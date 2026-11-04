//! Redshift's window: a thin layer that exposes `redshift_core::app::App` to the web UI.
//!
//! Every command here is a one-line call into the core, which is where the behaviour (and the
//! tests) live. Nothing in this file decides anything about playback, the library or settings.
//!
//! How the UI hears about things:
//! - event `player-event`: a [`PlayerEvent`] (track changed, queue changed, ended, error, device)
//! - event `scan-progress`: a [`ScanProgress`] while a library scan runs
//! - commands `player_state` and `viz_frame` are polled by the UI (position, meters, visualisers)
//! - cover art is an image URL: `cover://localhost/<encodeURIComponent(audio file path)>`
//!   (on Windows: `http://cover.localhost/<...>`); load it with `crossOrigin="anonymous"` to read pixels

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use redshift_core::app::{App, EqSettings, Settings};
use redshift_core::dsp::eq::{BAND_COUNT, BAND_FREQS, PRESETS};
use redshift_core::dsp::spectrum::{SpectrumConfig, VizFrame};
use redshift_core::engine::output::{list_output_devices, DeviceInfo};
use redshift_core::engine::{PlayerConfig, PlayerState, QueueView, ReplayGainMode};
use redshift_core::library::{Album, Artist, ScanReport};
use redshift_core::lyrics::Lyrics;
use redshift_core::meta::TrackMeta;
use redshift_core::playlists::{Playlist, PlaylistId, PlaylistSummary};
use redshift_core::queue::{EntryId, RepeatMode};
use redshift_core::util::percent_decode;
use tauri::{Emitter, Manager, RunEvent, State};

struct AppState(Arc<App>);

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---- transport and queue ----

#[tauri::command]
fn set_queue(state: State<AppState>, paths: Vec<PathBuf>, start: usize, autoplay: bool) {
    state.0.player.set_queue(paths, start, autoplay);
}
#[tauri::command]
fn play_entry(state: State<AppState>, id: EntryId) {
    state.0.player.play_entry(id);
}
#[tauri::command]
fn pause(state: State<AppState>) {
    state.0.player.pause();
}
#[tauri::command]
fn resume(state: State<AppState>) {
    state.0.player.resume();
}
#[tauri::command]
fn toggle_pause(state: State<AppState>) {
    state.0.player.toggle_pause();
}
#[tauri::command]
fn stop(state: State<AppState>) {
    state.0.player.stop();
}
#[tauri::command]
fn next(state: State<AppState>) {
    state.0.player.next();
}
#[tauri::command]
fn previous(state: State<AppState>) {
    state.0.player.previous();
}
#[tauri::command]
fn seek(state: State<AppState>, secs: f64) {
    state.0.player.seek(secs);
}
#[tauri::command]
fn queue_append(state: State<AppState>, paths: Vec<PathBuf>) {
    state.0.player.append(paths);
}
#[tauri::command]
fn queue_play_next(state: State<AppState>, paths: Vec<PathBuf>) {
    state.0.player.play_next(paths);
}
#[tauri::command]
fn queue_remove(state: State<AppState>, id: EntryId) {
    state.0.player.remove(id);
}
#[tauri::command]
fn queue_move(state: State<AppState>, from: usize, to: usize) {
    state.0.player.move_entry(from, to);
}
#[tauri::command]
fn queue_clear(state: State<AppState>) {
    state.0.player.clear_queue();
}
#[tauri::command]
fn player_state(state: State<AppState>) -> PlayerState {
    state.0.player.state()
}
#[tauri::command]
fn queue_view(state: State<AppState>) -> QueueView {
    state.0.player.queue()
}

// ---- sound settings (saved) ----

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    state.0.settings()
}
#[tauri::command]
fn set_volume(state: State<AppState>, volume: f32) {
    state.0.set_volume(volume);
}
#[tauri::command]
fn set_muted(state: State<AppState>, muted: bool) {
    state.0.set_muted(muted);
}
#[tauri::command]
fn set_shuffle(state: State<AppState>, on: bool) {
    state.0.set_shuffle(on);
}
#[tauri::command]
fn set_repeat(state: State<AppState>, mode: RepeatMode) {
    state.0.set_repeat(mode);
}
#[tauri::command]
fn set_crossfade_secs(state: State<AppState>, secs: f32) {
    state.0.set_crossfade_secs(secs);
}
#[tauri::command]
fn set_replaygain(state: State<AppState>, mode: ReplayGainMode, preamp_db: f32) {
    state.0.set_replaygain(mode, preamp_db);
}
#[tauri::command]
fn output_devices() -> Vec<DeviceInfo> {
    list_output_devices()
}
#[tauri::command]
fn set_output_device(state: State<AppState>, id: Option<String>) {
    state.0.set_device(id);
}

// ---- equaliser ----

#[derive(serde::Serialize)]
struct EqInfo {
    settings: EqSettings,
    band_freqs_hz: [f32; BAND_COUNT],
    presets: Vec<(&'static str, [f32; BAND_COUNT])>,
}

#[tauri::command]
fn eq_info(state: State<AppState>) -> EqInfo {
    EqInfo { settings: state.0.settings().eq, band_freqs_hz: BAND_FREQS, presets: PRESETS.to_vec() }
}
#[tauri::command]
fn set_eq_enabled(state: State<AppState>, on: bool) {
    state.0.set_eq_enabled(on);
}
#[tauri::command]
fn set_eq_preamp_db(state: State<AppState>, db: f32) {
    state.0.set_eq_preamp_db(db);
}
#[tauri::command]
fn set_eq_band_db(state: State<AppState>, band: usize, db: f32) {
    state.0.set_eq_band_db(band, db);
}
#[tauri::command]
fn set_eq_gains_db(state: State<AppState>, gains: [f32; BAND_COUNT]) {
    state.0.set_eq_gains_db(gains);
}
#[tauri::command]
fn apply_eq_preset(state: State<AppState>, name: String) -> bool {
    state.0.apply_eq_preset(&name)
}
#[tauri::command]
fn eq_curve(state: State<AppState>, points: usize) -> Vec<[f32; 2]> {
    state.0.eq_curve(points)
}

// ---- visualisation ----

#[tauri::command]
fn viz_frame(state: State<AppState>) -> VizFrame {
    state.0.player.viz()
}
#[tauri::command]
fn configure_viz(state: State<AppState>, config: SpectrumConfig) {
    state.0.configure_viz(config);
}

// ---- the UI's own saved state (theme, layout, customisation) ----

#[tauri::command]
fn ui_state(state: State<AppState>) -> serde_json::Value {
    state.0.ui_state()
}
#[tauri::command]
fn set_ui_state(state: State<AppState>, value: serde_json::Value) -> CmdResult<()> {
    state.0.set_ui_state(value).map_err(err)
}

// ---- library ----

#[tauri::command]
fn library_folders(state: State<AppState>) -> Vec<PathBuf> {
    state.0.library_folders()
}
#[tauri::command]
fn add_library_folder(state: State<AppState>, folder: PathBuf) -> CmdResult<bool> {
    state.0.add_library_folder(&folder).map_err(err)
}
#[tauri::command]
fn remove_library_folder(state: State<AppState>, folder: PathBuf) -> CmdResult<bool> {
    state.0.remove_library_folder(&folder).map_err(err)
}

/// Rescans the library folders on a worker thread, sending `scan-progress` events (at most about
/// twenty a second) until it finishes.
#[tauri::command]
async fn scan_library(app: tauri::AppHandle, state: State<'_, AppState>) -> CmdResult<ScanReport> {
    let core = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let mut last = Instant::now().checked_sub(Duration::from_secs(1)).unwrap_or_else(Instant::now);
        core.scan_library(&mut |p| {
            if p.done >= p.total || last.elapsed() >= Duration::from_millis(50) {
                last = Instant::now();
                let _ = app.emit("scan-progress", &p);
            }
        })
    })
    .await
    .map_err(err)?
    .map_err(err)
}
#[tauri::command]
fn is_scanning(state: State<AppState>) -> bool {
    state.0.is_scanning()
}
#[tauri::command]
fn library_tracks(state: State<AppState>) -> Vec<TrackMeta> {
    state.0.tracks()
}
#[tauri::command]
fn search_library(state: State<AppState>, query: String) -> Vec<TrackMeta> {
    state.0.search(&query)
}
#[tauri::command]
fn library_albums(state: State<AppState>) -> Vec<Album> {
    state.0.albums()
}
#[tauri::command]
fn library_artists(state: State<AppState>) -> Vec<Artist> {
    state.0.artists()
}
#[tauri::command]
fn recently_added(state: State<AppState>, limit: usize) -> Vec<TrackMeta> {
    state.0.recently_added(limit)
}
#[tauri::command]
fn track_meta(state: State<AppState>, path: PathBuf) -> CmdResult<TrackMeta> {
    state.0.track_meta(&path).map_err(err)
}
/// The track's loudness outline, for waveform seek bars. Decodes the file on a worker thread the
/// first time; later calls for the same file are instant.
#[tauri::command]
async fn waveform(state: State<'_, AppState>, path: PathBuf, buckets: usize) -> CmdResult<redshift_core::waveform::Overview> {
    let core = state.0.clone();
    let overview = tauri::async_runtime::spawn_blocking(move || core.waveform(&path, buckets)).await.map_err(err)?.map_err(err)?;
    Ok((*overview).clone())
}
#[tauri::command]
fn lyrics(state: State<AppState>, path: PathBuf) -> Option<Lyrics> {
    state.0.lyrics(&path)
}

// ---- playlists ----

#[tauri::command]
fn playlists(state: State<AppState>) -> Vec<PlaylistSummary> {
    state.0.playlists()
}
#[tauri::command]
fn playlist(state: State<AppState>, id: PlaylistId) -> Option<Playlist> {
    state.0.playlist(id)
}
#[tauri::command]
fn create_playlist(state: State<AppState>, name: String, tracks: Vec<PathBuf>) -> CmdResult<PlaylistId> {
    state.0.create_playlist(&name, tracks).map_err(err)
}
#[tauri::command]
fn rename_playlist(state: State<AppState>, id: PlaylistId, name: String) -> CmdResult<()> {
    state.0.rename_playlist(id, &name).map_err(err)
}
#[tauri::command]
fn delete_playlist(state: State<AppState>, id: PlaylistId) -> CmdResult<()> {
    state.0.delete_playlist(id).map_err(err)
}
#[tauri::command]
fn move_playlist(state: State<AppState>, id: PlaylistId, to: usize) -> CmdResult<()> {
    state.0.move_playlist(id, to).map_err(err)
}
#[tauri::command]
fn add_to_playlist(state: State<AppState>, id: PlaylistId, tracks: Vec<PathBuf>, position: Option<usize>) -> CmdResult<()> {
    state.0.add_to_playlist(id, &tracks, position).map_err(err)
}
#[tauri::command]
fn remove_from_playlist(state: State<AppState>, id: PlaylistId, positions: Vec<usize>) -> CmdResult<usize> {
    state.0.remove_from_playlist(id, &positions).map_err(err)
}
#[tauri::command]
fn move_in_playlist(state: State<AppState>, id: PlaylistId, from: usize, to: usize) -> CmdResult<()> {
    state.0.move_in_playlist(id, from, to).map_err(err)
}
#[tauri::command]
fn import_m3u(state: State<AppState>, file: PathBuf) -> CmdResult<(PlaylistId, usize)> {
    state.0.import_m3u(&file).map_err(err)
}
#[tauri::command]
fn export_m3u(state: State<AppState>, id: PlaylistId, dest: PathBuf) -> CmdResult<()> {
    state.0.export_m3u(id, &dest).map_err(err)
}
#[tauri::command]
fn play_playlist(state: State<AppState>, id: PlaylistId, start: usize) -> CmdResult<()> {
    state.0.play_playlist(id, start).map_err(err)
}

/// Serves cover art for `cover://localhost/<percent-encoded audio file path>`.
fn serve_cover(app: &tauri::AppHandle, uri_path: &str) -> tauri::http::Response<Cow<'static, [u8]>> {
    let path = percent_decode(uri_path.trim_start_matches('/'));
    let cover = app.try_state::<AppState>().and_then(|s| s.0.cover(Path::new(&path)));
    match cover {
        Some(c) => tauri::http::Response::builder()
            .header("Content-Type", c.mime)
            .header("Cache-Control", "max-age=3600")
            // The UI reads cover pixels (to theme itself from the artwork), which a canvas only
            // allows for images served with CORS enabled.
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(c.data)),
        None => tauri::http::Response::builder().status(404).body(Cow::Borrowed(&[][..])),
    }
    .expect("static headers are valid")
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .register_uri_scheme_protocol("cover", |ctx, request| serve_cover(ctx.app_handle(), request.uri().path()))
        .setup(|app| {
            let data_dir = app.path().app_data_dir()?;
            let (core, events) = App::new(data_dir, PlayerConfig::default())?;
            let core = Arc::new(core);

            // Player events go to the UI as they happen.
            let handle = app.handle().clone();
            std::thread::Builder::new().name("player-events".into()).spawn(move || {
                for event in events {
                    let _ = handle.emit("player-event", &event);
                }
            })?;
            // Settings changed by sliders are written in batches rather than once per mouse move.
            let saver = core.clone();
            std::thread::Builder::new().name("settings-saver".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_secs(2));
                let _ = saver.flush();
            })?;

            app.manage(AppState(core));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_queue, play_entry, pause, resume, toggle_pause, stop, next, previous, seek,
            queue_append, queue_play_next, queue_remove, queue_move, queue_clear, player_state, queue_view,
            get_settings, set_volume, set_muted, set_shuffle, set_repeat, set_crossfade_secs, set_replaygain,
            output_devices, set_output_device,
            eq_info, set_eq_enabled, set_eq_preamp_db, set_eq_band_db, set_eq_gains_db, apply_eq_preset, eq_curve,
            viz_frame, configure_viz, ui_state, set_ui_state,
            library_folders, add_library_folder, remove_library_folder, scan_library, is_scanning,
            library_tracks, search_library, library_albums, library_artists, recently_added, track_meta, lyrics, waveform,
            playlists, playlist, create_playlist, rename_playlist, delete_playlist, move_playlist,
            add_to_playlist, remove_from_playlist, move_in_playlist, import_m3u, export_m3u, play_playlist,
        ])
        .build(tauri::generate_context!())
        .expect("failed to build the Redshift window");

    app.run(|handle, event| {
        if let RunEvent::Exit = event {
            if let Some(state) = handle.try_state::<AppState>() {
                let _ = state.0.flush();
            }
        }
    });
}
