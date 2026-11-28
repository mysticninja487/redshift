//! The Internet Archive as a source: searching it, opening an item, playing its files, and what the
//! rest of the app needs to treat those songs like any other song (tags, covers).
//!
//! A file in an item rides through the queue, playlists, history and stats as the path
//! `archive://<identifier>/<file>` (see [`crate::util::archive_song_path`]).

use std::collections::{HashMap, VecDeque};

use super::*;
use crate::net::archive::{ArchiveItem, ArchiveQuery, ArchiveSearch, ArchiveTrack};
use crate::util::{archive_cover_id, archive_song_parts};

const SONGS_FILE: &str = "archive-songs.json";
/// The first page of the last few searches, shown at once the next time (the Archive can be slow).
const BROWSE_FILE: &str = "archive-browse.json";
const BROWSE_MAX: usize = 8;
const COVER_FILES_MAX: usize = 500;
/// How many songs' details are remembered, so a queue restored at launch can be shown before the
/// Archive is asked.
const SONG_CACHE_MAX: usize = 3000;
const COVER_CACHE_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ArchiveState {
    /// By queue path.
    songs: HashMap<String, ArchiveTrack>,
    order: VecDeque<String>,
    dirty: bool,
    /// Covers by item identifier; `None` remembers "there is none".
    covers: VecDeque<(String, Option<Arc<Cover>>)>,
    /// First pages of recent searches, oldest first.
    browse: VecDeque<(String, ArchiveSearch)>,
}

impl ArchiveState {
    pub(super) fn load(data_dir: &Path) -> Self {
        let list: Vec<ArchiveTrack> = fs::read(data_dir.join(SONGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut st = ArchiveState::default();
        let browse: Vec<(String, ArchiveSearch)> = fs::read(data_dir.join(BROWSE_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        st.browse = browse.into_iter().collect();
        for t in list {
            st.insert(t);
        }
        st.dirty = false;
        st
    }

    fn insert(&mut self, t: ArchiveTrack) {
        if t.path.is_empty() {
            return;
        }
        if self.songs.insert(t.path.clone(), t.clone()).is_none() {
            self.order.push_back(t.path);
            while self.order.len() > SONG_CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.songs.remove(&old);
                }
            }
        }
        self.dirty = true;
    }
}

/// What identifies a search for the purpose of showing its first page again.
fn browse_key(q: &ArchiveQuery) -> String {
    format!("{}|{}|{}|{}", q.text.trim().to_lowercase(), q.lossless_only, q.collection, q.sort)
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG") {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else {
        "image/jpeg"
    }
}

/// Keeps the folder of remembered pictures from growing: the oldest go first.
fn prune_cover_files(dir: &Path) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = rd.flatten().filter_map(|e| Some((e.metadata().ok()?.modified().ok()?, e.path()))).collect();
    if files.len() <= COVER_FILES_MAX {
        return;
    }
    files.sort();
    for (_, p) in files.iter().take(files.len() - (COVER_FILES_MAX - 100)) {
        let _ = fs::remove_file(p);
    }
}

impl App {
    fn archive_gate(&self) -> Result<()> {
        let o = self.online();
        if !o.enabled || !o.archive {
            return Err(Error::Offline);
        }
        Ok(())
    }

    fn remember_archive(&self, tracks: &[ArchiveTrack]) {
        let mut st = self.archive.lock();
        for t in tracks {
            st.insert(t.clone());
        }
    }

    /// Looks for audio items on the Internet Archive.
    pub fn archive_search(&self, q: &ArchiveQuery) -> Result<ArchiveSearch> {
        self.archive_gate()?;
        let found = self.net.archive_search(q)?;
        if q.page <= 1 && !found.hits.is_empty() {
            let snapshot = {
                let mut st = self.archive.lock();
                let key = browse_key(q);
                st.browse.retain(|(k, _)| *k != key);
                st.browse.push_back((key, found.clone()));
                while st.browse.len() > BROWSE_MAX {
                    st.browse.pop_front();
                }
                st.browse.iter().cloned().collect::<Vec<_>>()
            };
            let _ = write_json_atomic(&self.data_dir.join(BROWSE_FILE), &snapshot);
        }
        Ok(found)
    }

    /// The first page this search had last time, if any, whatever its age. It is shown straight
    /// away while the real answer is fetched.
    pub fn archive_cached(&self, q: &ArchiveQuery) -> Option<ArchiveSearch> {
        let key = browse_key(q);
        self.archive.lock().browse.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
    }

    /// An item and its songs. With `lossless_only`, songs that only exist as lossy files are left out.
    pub fn archive_item(&self, identifier: &str, lossless_only: bool) -> Result<ArchiveItem> {
        self.archive_gate()?;
        let item = self.net.archive_item(identifier, lossless_only)?;
        self.remember_archive(&item.tracks);
        Ok(item)
    }

    /// Plays files from the Archive (replacing the queue), starting at `start`. `paths` are the
    /// `path` of each [`ArchiveTrack`].
    pub fn archive_play(&self, paths: &[String], start: usize) -> Result<()> {
        self.archive_gate()?;
        let paths: Vec<PathBuf> = paths.iter().map(PathBuf::from).filter(|p| archive_song_parts(p).is_some()).collect();
        if paths.is_empty() {
            return Ok(());
        }
        self.play_queue(paths.clone(), start.min(paths.len() - 1), true);
        Ok(())
    }

    /// The details of a song from the Archive: what was seen before, else the item is asked for.
    fn archive_song(&self, path: &Path) -> Option<ArchiveTrack> {
        let key = path.to_string_lossy().trim().to_string();
        if let Some(t) = self.archive.lock().songs.get(&key) {
            return Some(t.clone());
        }
        let (identifier, _) = archive_song_parts(path)?;
        self.archive_gate().ok()?;
        let item = self.net.archive_item(&identifier, false).ok()?;
        self.remember_archive(&item.tracks);
        item.tracks.into_iter().find(|t| t.path == key)
    }

    /// Tags for a song from the Archive. Never fails: a song the Archive cannot be asked about is
    /// shown by its file name.
    pub(super) fn archive_meta(&self, path: &Path) -> TrackMeta {
        let Some(t) = self.archive_song(path) else {
            let file = archive_song_parts(path).map(|(_, f)| f).unwrap_or_default();
            let title = file.rsplit('/').next().unwrap_or("").rsplit_once('.').map(|(s, _)| s.to_string()).filter(|s| !s.is_empty()).unwrap_or_else(|| "Song from the Internet Archive".into());
            return TrackMeta { path: path.to_path_buf(), title, ..TrackMeta::default() };
        };
        let artist = if t.artist.is_empty() { crate::meta::UNKNOWN_ARTIST.to_string() } else { t.artist.clone() };
        TrackMeta {
            path: path.to_path_buf(),
            title: t.title.clone(),
            album: if t.album.is_empty() { crate::meta::UNKNOWN_ALBUM.to_string() } else { t.album.clone() },
            album_artist: artist.clone(),
            artist,
            year: t.year.and_then(|y| u16::try_from(y).ok()),
            track_no: t.track,
            duration_secs: t.duration_secs,
            sample_rate: t.sample_rate,
            bit_depth: t.bit_depth.and_then(|b| u8::try_from(b).ok()),
            bitrate_kbps: t.bitrate_kbps,
            format: t.suffix.clone(),
            size: t.size,
            ..TrackMeta::default()
        }
    }

    /// The picture of the item a song (or an `archive-cover://` path) belongs to.
    pub(super) fn archive_cover_of(&self, path: &Path) -> Option<Arc<Cover>> {
        let identifier = archive_cover_id(path).or_else(|| archive_song_parts(path).map(|(id, _)| id))?;
        if let Some((_, hit)) = self.archive.lock().covers.iter().find(|(c, _)| *c == identifier) {
            return hit.clone();
        }
        // A picture fetched before is on disk: no network, and it works offline.
        let file = self.net.cache_dir().join("archive-covers").join(format!("{identifier}.img"));
        let from_disk = fs::read(&file).ok().filter(|b| !b.is_empty());
        let (data, remember_missing) = match from_disk {
            Some(data) => (Some(data), false),
            None => {
                self.archive_gate().ok()?;
                // Fetched outside the lock: it is a network request.
                match self.net.archive_cover(&identifier) {
                    Ok(b) if !b.is_empty() => {
                        if let Some(dir) = file.parent() {
                            let _ = fs::create_dir_all(dir);
                            if fs::write(&file, &b).is_ok() {
                                prune_cover_files(dir);
                            }
                        }
                        (Some(b), false)
                    }
                    // Only "this item has no picture" is worth remembering; a slow or failed
                    // request should be tried again the next time it is asked for.
                    Err(e) => (None, e.to_string().contains("no picture")),
                    Ok(_) => (None, true),
                }
            }
        };
        let found = data.map(|data| Arc::new(Cover { mime: sniff_mime(&data).to_string(), data }));
        if found.is_none() && !remember_missing {
            return None;
        }
        let mut st = self.archive.lock();
        st.covers.push_back((identifier, found.clone()));
        let mut bytes: usize = st.covers.iter().map(|(_, c)| c.as_ref().map_or(0, |c| c.data.len())).sum();
        while bytes > COVER_CACHE_BYTES && st.covers.len() > 1 {
            if let Some((_, old)) = st.covers.pop_front() {
                bytes -= old.map_or(0, |c| c.data.len());
            }
        }
        found
    }

    /// Writes the remembered song details if they changed.
    pub(super) fn save_archive_songs(&self) -> Result<()> {
        let snapshot = {
            let mut st = self.archive.lock();
            if !st.dirty {
                return Ok(());
            }
            st.dirty = false;
            st.order.iter().filter_map(|p| st.songs.get(p).cloned()).collect::<Vec<_>>()
        };
        if let Err(e) = write_json_atomic(&self.data_dir.join(SONGS_FILE), &snapshot) {
            self.archive.lock().dirty = true;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineConfig;
    use crate::net::fakearchive;
    use crate::util::archive_song_path;

    fn open(dir: &Path) -> App {
        let cfg = PlayerConfig { device: None, offline: Some(OfflineConfig { sample_rate: 48_000, sink: Arc::new(Mutex::new(Vec::new())), speed: 4.0 }), ..Default::default() };
        App::new(dir, cfg).unwrap().0
    }

    fn online(app: &App, archive: bool) {
        app.set_online(OnlineSettings { enabled: true, archive, ..OnlineSettings::default() });
    }

    #[test]
    fn the_archive_needs_both_online_switches() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        let q = ArchiveQuery::default();
        assert!(matches!(app.archive_search(&q), Err(Error::Offline)), "main switch off");
        online(&app, false);
        assert!(matches!(app.archive_search(&q), Err(Error::Offline)), "archive switch off");
        assert!(matches!(app.archive_item("demo-live", true), Err(Error::Offline)));
        assert!(matches!(app.archive_play(&["archive://demo-live/a.flac".into()], 0), Err(Error::Offline)));
        // Offline, a song is still shown by its file name and a cover is simply missing.
        let p = archive_song_path("demo-live", "Disc 2/02 Some Song.flac");
        assert_eq!(app.track_meta(&p).unwrap().title, "02 Some Song");
        assert!(app.cover(&p).is_none());
    }

    #[test]
    fn items_are_remembered_and_songs_show_as_normal_tracks() {
        let f = fakearchive::start(vec![0; 10], true);
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.net.set_archive_base_for_tests(&f.base);
        online(&app, true);
        let found = app.archive_search(&ArchiveQuery { text: "hall".into(), ..Default::default() }).unwrap();
        assert_eq!(found.hits[0].identifier, "demo-live");
        let item = app.archive_item("demo-live", true).unwrap();
        let p = PathBuf::from(&item.tracks[0].path);
        assert_eq!(p, archive_song_path("demo-live", "d1t01.flac"));
        let m = app.track_meta(&p).unwrap();
        assert_eq!((m.title.as_str(), m.artist.as_str(), m.album.as_str(), m.format.as_str(), m.year, m.duration_secs), ("Opening", "The Band", "Live at the Hall", "flac", Some(1977), 12.5));
        assert_eq!(app.paths_exist(&[p.clone(), PathBuf::from("/no/such.flac")]), vec![true, false]);
        // The cover comes once, then from memory.
        assert_eq!(app.cover_sized(&p, 300).expect("a cover").mime, "image/png");
        let before = f.hits.load(Ordering::Relaxed);
        assert!(app.cover(&p).is_some() && app.cover(&crate::util::archive_cover_path("demo-live")).is_some());
        assert_eq!(f.hits.load(Ordering::Relaxed), before);
    }

    #[test]
    fn a_song_not_seen_yet_is_looked_up_and_remembered_across_restarts() {
        let f = fakearchive::start(vec![0; 10], true);
        let d = tempfile::tempdir().unwrap();
        let p = archive_song_path("demo-live", "d1t03.mp3");
        {
            let app = open(d.path());
            app.net.set_archive_base_for_tests(&f.base);
            online(&app, true);
            let m = app.track_meta(&p).unwrap();
            assert_eq!((m.title.as_str(), m.format.as_str(), m.bitrate_kbps), ("d1t03", "mp3", None));
            app.flush().unwrap();
        }
        // Offline after a restart: what was seen is still shown.
        let app = open(d.path());
        assert_eq!(app.track_meta(&p).unwrap().title, "d1t03");
        assert_eq!(app.track_meta(&p).unwrap().album, "Live at the Hall");
    }

    #[test]
    fn archive_songs_come_back_with_the_queue_and_only_real_paths_are_played() {
        let f = fakearchive::start(fakesub_wav(), true);
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.net.set_archive_base_for_tests(&f.base);
        online(&app, true);
        let p = archive_song_path("demo-live", "d1t01.flac");
        app.with_stats(|s| s.record_play(&p));
        assert_eq!(app.play_stats().len(), 1);
        app.archive_play(&["/etc/passwd".into(), "subsonic://5".into()], 0).unwrap();
        assert!(app.player.queue_paths().0.is_empty(), "paths that are not the Archive's are ignored");
        app.archive_play(&[p.to_string_lossy().into_owned()], 0).unwrap();
        // The player thread takes the new queue a moment later.
        let start = std::time::Instant::now();
        while app.player.queue_paths().0 != vec![p.clone()] && start.elapsed() < std::time::Duration::from_secs(3) {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(app.player.queue_paths().0, vec![p]);
    }

    fn fakesub_wav() -> Vec<u8> {
        crate::net::fakesub::wav(2, 44100)
    }
}
