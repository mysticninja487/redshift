//! Windows media controls: the track, cover and play state in the volume flyout and on the lock
//! screen, and the keyboard's (or headset's) play, pause, next and previous keys.
//!
//! Everything here mirrors the player; the player stays the single source of truth. On other
//! systems this does nothing.

#[cfg(not(windows))]
pub fn start(_app: &tauri::AppHandle, _core: std::sync::Arc<redshift_core::app::App>) {}

#[cfg(windows)]
pub use imp::start;

#[cfg(windows)]
mod imp {
    use std::ffi::c_void;
    use std::path::{Path, PathBuf};
    use std::sync::Arc;
    use std::time::Duration;

    use redshift_core::app::App;
    use redshift_core::engine::Status;
    use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig, SeekDirection};
    use tauri::{AppHandle, Manager};

    /// `file:///C:/Users/me/AppData/.../cover.jpg`, which is what the media flyout can load.
    fn file_uri(path: &Path) -> String {
        let mut out = String::from("file:///");
        for b in path.to_string_lossy().replace('\\', "/").bytes() {
            match b {
                b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'/' | b':' | b'.' | b'_' | b'-' | b'~' => out.push(b as char),
                _ => out.push_str(&format!("%{b:02X}")),
            }
        }
        out
    }

    fn handle(core: &App, event: MediaControlEvent) {
        let p = &core.player;
        match event {
            MediaControlEvent::Play => p.resume(),
            MediaControlEvent::Pause => p.pause(),
            MediaControlEvent::Toggle => p.toggle_pause(),
            MediaControlEvent::Next => p.next(),
            MediaControlEvent::Previous => p.previous(),
            MediaControlEvent::Stop => p.stop(),
            MediaControlEvent::SetPosition(MediaPosition(d)) => p.seek(d.as_secs_f64()),
            MediaControlEvent::SeekBy(dir, d) => {
                let now = p.position_secs();
                p.seek(if dir == SeekDirection::Forward { now + d.as_secs_f64() } else { (now - d.as_secs_f64()).max(0.0) });
            }
            MediaControlEvent::Seek(dir) => {
                let now = p.position_secs();
                p.seek(if dir == SeekDirection::Forward { now + 5.0 } else { (now - 5.0).max(0.0) });
            }
            _ => {}
        }
    }

    /// Writes the track's cover where the media flyout can read it; two file names alternate so a
    /// new cover never has to replace one the system still has open.
    fn cover_file(core: &App, track: &Path, flip: &mut bool) -> Option<PathBuf> {
        let cover = core.cover(track)?;
        let ext = if cover.mime.contains("png") { "png" } else { "jpg" };
        *flip = !*flip;
        let file = std::env::temp_dir().join(format!("redshift-cover-{}.{ext}", u8::from(*flip)));
        std::fs::write(&file, &cover.data).ok()?;
        Some(file)
    }

    pub fn start(app: &AppHandle, core: Arc<App>) {
        let Some(hwnd) = app.get_webview_window("main").and_then(|w| w.hwnd().ok()) else { return };
        let hwnd = hwnd.0 as usize; // a raw handle can't cross threads as a pointer, but its number can
        let spawned = std::thread::Builder::new().name("media-controls".into()).spawn(move || {
            let config = PlatformConfig { dbus_name: "redshift", display_name: "Redshift", hwnd: Some(hwnd as *mut c_void) };
            let mut controls = match MediaControls::new(config) {
                Ok(c) => c,
                Err(e) => { eprintln!("media controls unavailable: {e}"); return; }
            };
            let handler_core = core.clone();
            if let Err(e) = controls.attach(move |event| handle(&handler_core, event)) {
                eprintln!("media keys unavailable: {e}");
                return;
            }
            let (mut shown, mut status, mut last_pos, mut flip) = (None::<redshift_core::queue::EntryId>, None::<Status>, -10.0_f64, false);
            loop {
                std::thread::sleep(Duration::from_millis(400));
                let current = core.player.current_track();
                let now_status = core.player.status();
                let pos = core.player.position_secs();
                let id = current.as_ref().map(|c| c.entry_id);
                if id != shown {
                    shown = id;
                    last_pos = -10.0;
                    match &current {
                        Some(c) => {
                            let meta = core.track_meta(&c.path).ok();
                            let cover = cover_file(&core, &c.path, &mut flip).map(|f| file_uri(&f));
                            let (title, artist, album) = match &meta {
                                Some(m) => (m.title.clone(), m.artist.clone(), m.album.clone()),
                                None => (c.path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(), String::new(), String::new()),
                            };
                            let _ = controls.set_metadata(MediaMetadata {
                                title: Some(&title),
                                artist: Some(&artist),
                                album: Some(&album),
                                cover_url: cover.as_deref(),
                                duration: c.duration_secs.map(Duration::from_secs_f64),
                            });
                        }
                        None => { let _ = controls.set_metadata(MediaMetadata::default()); }
                    }
                }
                // Tell the system when the state changes or the position jumps (a seek); in between
                // it moves the clock itself.
                let moved = (pos - last_pos - 0.4).abs() > 1.5;
                if status != Some(now_status) || moved {
                    status = Some(now_status);
                    last_pos = pos;
                    let progress = Some(MediaPosition(Duration::from_secs_f64(pos.max(0.0))));
                    let _ = controls.set_playback(match (now_status, current.is_some()) {
                        (Status::Playing, true) => MediaPlayback::Playing { progress },
                        (Status::Paused, true) => MediaPlayback::Paused { progress },
                        _ => MediaPlayback::Stopped,
                    });
                } else {
                    last_pos = pos;
                }
            }
        });
        if let Err(e) = spawned { eprintln!("media controls thread: {e}"); }
    }
}
