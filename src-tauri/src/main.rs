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

mod media;

use std::borrow::Cow;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use redshift_core::app::{App, CustomPreset, EqSettings, PlayStat, Settings, TraySettings};
use redshift_core::dsp::eq::{BAND_COUNT, BAND_FREQS, PRESETS};
use redshift_core::dsp::spectrum::{SpectrumConfig, VizFrame};
use redshift_core::engine::output::{list_output_devices, DeviceInfo};
use redshift_core::engine::{PlayerConfig, PlayerState, QueueView, ReplayGainMode};
use redshift_core::library::{Album, Artist, ScanReport};
use redshift_core::lyrics::Lyrics;
use redshift_core::meta::TrackMeta;
use redshift_core::fonts::FontEntry;
use redshift_core::playlists::{ImportReport, Playlist, PlaylistId, PlaylistSummary, RelinkReport};
use redshift_core::queue::{EntryId, RepeatMode};
use redshift_core::util::percent_decode;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent};
use tauri::{AppHandle, DragDropEvent, Emitter, Manager, RunEvent, State, WebviewUrl, WebviewWindowBuilder, WindowEvent};

struct AppState(Arc<App>);

type CmdResult<T> = Result<T, String>;

fn err(e: impl std::fmt::Display) -> String {
    e.to_string()
}

// ---- transport and queue ----

#[tauri::command(async)]
fn set_queue(state: State<AppState>, paths: Vec<PathBuf>, start: usize, autoplay: bool) {
    state.0.player.set_queue(paths, start, autoplay);
}
#[tauri::command(async)]
fn play_entry(state: State<AppState>, id: EntryId) {
    state.0.player.play_entry(id);
}
#[tauri::command(async)]
fn pause(state: State<AppState>) {
    state.0.player.pause();
}
#[tauri::command(async)]
fn resume(state: State<AppState>) {
    state.0.player.resume();
}
#[tauri::command(async)]
fn toggle_pause(state: State<AppState>) {
    state.0.player.toggle_pause();
}
#[tauri::command(async)]
fn stop(state: State<AppState>) {
    state.0.player.stop();
}
#[tauri::command(async)]
fn next(state: State<AppState>) {
    state.0.player.next();
}
#[tauri::command(async)]
fn previous(state: State<AppState>) {
    state.0.player.previous();
}
#[tauri::command(async)]
fn seek(state: State<AppState>, secs: f64) {
    state.0.player.seek(secs);
}
#[tauri::command(async)]
fn queue_append(state: State<AppState>, paths: Vec<PathBuf>) {
    state.0.player.append(paths);
}
#[tauri::command(async)]
fn queue_play_next(state: State<AppState>, paths: Vec<PathBuf>) {
    state.0.player.play_next(paths);
}
#[tauri::command(async)]
fn queue_remove(state: State<AppState>, id: EntryId) {
    state.0.player.remove(id);
}
#[tauri::command(async)]
fn queue_move(state: State<AppState>, from: usize, to: usize) {
    state.0.player.move_entry(from, to);
}
#[tauri::command(async)]
fn queue_clear(state: State<AppState>) {
    state.0.player.clear_queue();
}
#[tauri::command(async)]
fn player_state(state: State<AppState>) -> PlayerState {
    state.0.player.state()
}
#[tauri::command(async)]
fn queue_view(state: State<AppState>) -> QueueView {
    state.0.player.queue()
}

// ---- sound settings (saved) ----

#[tauri::command(async)]
fn get_settings(state: State<AppState>) -> Settings {
    state.0.settings()
}
#[tauri::command(async)]
fn set_volume(state: State<AppState>, volume: f32) {
    state.0.set_volume(volume);
}
#[tauri::command(async)]
fn set_muted(state: State<AppState>, muted: bool) {
    state.0.set_muted(muted);
}
#[tauri::command(async)]
fn set_shuffle(state: State<AppState>, on: bool) {
    state.0.set_shuffle(on);
}
#[tauri::command(async)]
fn set_repeat(state: State<AppState>, mode: RepeatMode) {
    state.0.set_repeat(mode);
}
#[tauri::command(async)]
fn set_crossfade_secs(state: State<AppState>, secs: f32) {
    state.0.set_crossfade_secs(secs);
}
#[tauri::command(async)]
fn set_replaygain(state: State<AppState>, mode: ReplayGainMode, preamp_db: f32) {
    state.0.set_replaygain(mode, preamp_db);
}
#[tauri::command(async)]
fn output_devices() -> Vec<DeviceInfo> {
    list_output_devices()
}
#[tauri::command(async)]
fn set_output_device(state: State<AppState>, id: Option<String>) {
    state.0.set_device(id);
}

// ---- equaliser ----

#[derive(serde::Serialize)]
struct EqInfo {
    settings: EqSettings,
    band_freqs_hz: [f32; BAND_COUNT],
    presets: Vec<(&'static str, [f32; BAND_COUNT])>,
    /// Presets the person saved, in the order they were saved.
    custom_presets: Vec<CustomPreset>,
}

#[tauri::command(async)]
fn eq_info(state: State<AppState>) -> EqInfo {
    let settings = state.0.settings();
    EqInfo { settings: settings.eq, band_freqs_hz: BAND_FREQS, presets: PRESETS.to_vec(), custom_presets: settings.custom_eq_presets }
}
#[tauri::command(async)]
fn set_eq_enabled(state: State<AppState>, on: bool) {
    state.0.set_eq_enabled(on);
}
#[tauri::command(async)]
fn set_eq_preamp_db(state: State<AppState>, db: f32) {
    state.0.set_eq_preamp_db(db);
}
#[tauri::command(async)]
fn set_eq_band_db(state: State<AppState>, band: usize, db: f32) {
    state.0.set_eq_band_db(band, db);
}
#[tauri::command(async)]
fn set_eq_gains_db(state: State<AppState>, gains: [f32; BAND_COUNT]) {
    state.0.set_eq_gains_db(gains);
}
#[tauri::command(async)]
fn apply_eq_preset(state: State<AppState>, name: String) -> bool {
    state.0.apply_eq_preset(&name)
}
#[tauri::command(async)]
fn save_eq_preset(state: State<AppState>, name: String) -> CmdResult<String> {
    state.0.save_eq_preset(&name).map_err(err)
}
#[tauri::command(async)]
fn delete_eq_preset(state: State<AppState>, name: String) -> bool {
    state.0.delete_eq_preset(&name)
}
#[tauri::command(async)]
fn eq_curve(state: State<AppState>, points: usize) -> Vec<[f32; 2]> {
    state.0.eq_curve(points)
}

// ---- visualisation ----

#[tauri::command(async)]
fn viz_frame(state: State<AppState>) -> VizFrame {
    state.0.player.viz()
}
#[tauri::command(async)]
fn configure_viz(state: State<AppState>, config: SpectrumConfig) {
    state.0.configure_viz(config);
}

// ---- the UI's own saved state (theme, layout, customisation) ----

#[tauri::command(async)]
fn ui_state(state: State<AppState>) -> serde_json::Value {
    state.0.ui_state()
}
#[tauri::command(async)]
fn set_ui_state(app: AppHandle, state: State<AppState>, value: serde_json::Value) -> CmdResult<()> {
    state.0.set_ui_state(value.clone()).map_err(err)?;
    // The main and mini windows each keep their own copy of the settings; this lets the other one catch up.
    let _ = app.emit("ui-state-changed", value);
    Ok(())
}

// ---- library ----

#[tauri::command(async)]
fn library_folders(state: State<AppState>) -> Vec<PathBuf> {
    state.0.library_folders()
}
#[tauri::command(async)]
fn add_library_folder(state: State<AppState>, folder: PathBuf) -> CmdResult<bool> {
    state.0.add_library_folder(&folder).map_err(err)
}
#[tauri::command(async)]
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
#[tauri::command(async)]
fn is_scanning(state: State<AppState>) -> bool {
    state.0.is_scanning()
}
#[tauri::command(async)]
fn library_tracks(state: State<AppState>) -> Vec<TrackMeta> {
    state.0.tracks()
}
#[tauri::command(async)]
fn search_library(state: State<AppState>, query: String) -> Vec<TrackMeta> {
    state.0.search(&query)
}
#[tauri::command(async)]
fn library_albums(state: State<AppState>) -> Vec<Album> {
    state.0.albums()
}
#[tauri::command(async)]
fn library_artists(state: State<AppState>) -> Vec<Artist> {
    state.0.artists()
}
#[tauri::command(async)]
fn recently_added(state: State<AppState>, limit: usize) -> Vec<TrackMeta> {
    state.0.recently_added(limit)
}
#[tauri::command(async)]
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
#[tauri::command(async)]
fn lyrics(state: State<AppState>, path: PathBuf) -> Option<Lyrics> {
    state.0.lyrics(&path)
}

// ---- playlists ----

#[tauri::command(async)]
fn playlists(state: State<AppState>) -> Vec<PlaylistSummary> {
    state.0.playlists()
}
#[tauri::command(async)]
fn playlist(state: State<AppState>, id: PlaylistId) -> Option<Playlist> {
    state.0.playlist(id)
}
#[tauri::command(async)]
fn create_playlist(state: State<AppState>, name: String, tracks: Vec<PathBuf>) -> CmdResult<PlaylistId> {
    state.0.create_playlist(&name, tracks).map_err(err)
}
#[tauri::command(async)]
fn rename_playlist(state: State<AppState>, id: PlaylistId, name: String) -> CmdResult<()> {
    state.0.rename_playlist(id, &name).map_err(err)
}
#[tauri::command(async)]
fn delete_playlist(state: State<AppState>, id: PlaylistId) -> CmdResult<()> {
    state.0.delete_playlist(id).map_err(err)
}
#[tauri::command(async)]
fn move_playlist(state: State<AppState>, id: PlaylistId, to: usize) -> CmdResult<()> {
    state.0.move_playlist(id, to).map_err(err)
}
#[tauri::command(async)]
fn add_to_playlist(state: State<AppState>, id: PlaylistId, tracks: Vec<PathBuf>, position: Option<usize>) -> CmdResult<()> {
    state.0.add_to_playlist(id, &tracks, position).map_err(err)
}
#[tauri::command(async)]
fn remove_from_playlist(state: State<AppState>, id: PlaylistId, positions: Vec<usize>) -> CmdResult<usize> {
    state.0.remove_from_playlist(id, &positions).map_err(err)
}
#[tauri::command(async)]
fn move_in_playlist(state: State<AppState>, id: PlaylistId, from: usize, to: usize) -> CmdResult<()> {
    state.0.move_in_playlist(id, from, to).map_err(err)
}
#[tauri::command]
async fn import_m3u(state: State<'_, AppState>, file: PathBuf) -> CmdResult<ImportReport> {
    // Matching entries against the library touches the disk, so keep it off the UI thread.
    let core = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || core.import_m3u(&file)).await.map_err(err)?.map_err(err)
}
#[tauri::command]
async fn relink_playlist(state: State<'_, AppState>, id: PlaylistId) -> CmdResult<RelinkReport> {
    let core = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || core.relink_playlist(id)).await.map_err(err)?.map_err(err)
}
#[tauri::command]
async fn paths_exist(state: State<'_, AppState>, paths: Vec<PathBuf>) -> CmdResult<Vec<bool>> {
    let core = state.0.clone();
    tauri::async_runtime::spawn_blocking(move || core.paths_exist(&paths)).await.map_err(err)
}
#[tauri::command(async)]
fn export_m3u(state: State<AppState>, id: PlaylistId, dest: PathBuf) -> CmdResult<()> {
    state.0.export_m3u(id, &dest).map_err(err)
}
#[tauri::command(async)]
fn play_playlist(state: State<AppState>, id: PlaylistId, start: usize) -> CmdResult<()> {
    state.0.play_playlist(id, start).map_err(err)
}

// ---- favourites and play history ----

#[tauri::command(async)]
fn favourite_paths(state: State<AppState>) -> Vec<PathBuf> {
    state.0.favourite_paths()
}
#[tauri::command(async)]
fn favourites(state: State<AppState>) -> Vec<TrackMeta> {
    state.0.favourites()
}
#[tauri::command(async)]
fn set_favourite(state: State<AppState>, paths: Vec<PathBuf>, on: bool) -> usize {
    state.0.set_favourite(&paths, on)
}
#[tauri::command(async)]
fn play_stats(state: State<AppState>) -> Vec<PlayStat> {
    state.0.play_stats()
}
#[tauri::command(async)]
fn reset_play_stats(state: State<AppState>, path: Option<PathBuf>) {
    state.0.reset_play_stats(path.as_deref());
}

// ---- fonts ----

#[tauri::command(async)]
fn fonts(state: State<AppState>) -> Vec<FontEntry> {
    state.0.fonts()
}
#[tauri::command(async)]
fn import_font(state: State<AppState>, path: PathBuf) -> CmdResult<FontEntry> {
    state.0.import_font(&path).map_err(err)
}
#[tauri::command(async)]
fn remove_font(state: State<AppState>, file: String) -> bool {
    state.0.remove_font(&file)
}

// ---- opening files, and the mini player ----

/// Plays what the player was opened with (files, folders or playlists) and tells the UI.
fn open_files(app: &AppHandle, inputs: &[PathBuf]) {
    let Some(state) = app.try_state::<AppState>() else { return };
    let n = state.0.open_paths(inputs);
    if n > 0 {
        let _ = app.emit("files-opened", n);
    }
}

/// Paths from a command line: everything that isn't a flag, so `Redshift.exe song.flac` and
/// "Open with Redshift" both work.
fn paths_from_args(args: impl IntoIterator<Item = String>) -> Vec<PathBuf> {
    args.into_iter().filter(|a| !a.starts_with('-')).map(PathBuf::from).collect()
}

const MINI_LABEL: &str = "mini";

/// The mini player's two layouts, in logical pixels: the size it opens at and how far it may be
/// resized by its corner. The compact strip stays a strip; the expanded card can grow into a small player.
struct MiniLayout { open: (f64, f64), min: (f64, f64), max: (f64, f64) }
const MINI_COMPACT: MiniLayout = MiniLayout { open: (404.0, 108.0), min: (320.0, 92.0), max: (720.0, 180.0) };
const MINI_EXPANDED: MiniLayout = MiniLayout { open: (720.0, 252.0), min: (520.0, 220.0), max: (1100.0, 700.0) };

fn mini_layout(expanded: bool) -> &'static MiniLayout { if expanded { &MINI_EXPANDED } else { &MINI_COMPACT } }

/// Applies a layout's limits to the mini window and sets its size: `size` if given (kept inside the
/// limits), otherwise the layout's own default.
fn fit_mini(mini: &tauri::WebviewWindow, expanded: bool, size: Option<(f64, f64)>) -> CmdResult<()> {
    let l = mini_layout(expanded);
    let (w, h) = size.unwrap_or(l.open);
    let (w, h) = (w.clamp(l.min.0, l.max.0), h.clamp(l.min.1, l.max.1));
    // Widen the limits first so the new size is never refused by the old ones.
    mini.set_min_size(Some(tauri::LogicalSize::new(l.min.0.min(MINI_COMPACT.min.0), l.min.1.min(MINI_COMPACT.min.1)))).map_err(err)?;
    mini.set_max_size(Some(tauri::LogicalSize::new(l.max.0.max(MINI_EXPANDED.max.0), l.max.1.max(MINI_EXPANDED.max.1)))).map_err(err)?;
    mini.set_size(tauri::LogicalSize::new(w, h)).map_err(err)?;
    mini.set_min_size(Some(tauri::LogicalSize::new(l.min.0, l.min.1))).map_err(err)?;
    mini.set_max_size(Some(tauri::LogicalSize::new(l.max.0, l.max.1))).map_err(err)?;
    Ok(())
}

/// Switches the mini player between its compact and expanded layout, at `width` by `height` (what the
/// person last chose for that layout) or the default size.
#[tauri::command(async)]
fn mini_size(app: AppHandle, expanded: bool, width: Option<f64>, height: Option<f64>) -> CmdResult<()> {
    if let Some(mini) = app.get_webview_window(MINI_LABEL) {
        fit_mini(&mini, expanded, width.zip(height))?;
    }
    Ok(())
}

/// Keeps the mini player above other windows, or lets it sit among them.
#[tauri::command(async)]
fn mini_on_top(app: AppHandle, on: bool) -> CmdResult<()> {
    if let Some(mini) = app.get_webview_window(MINI_LABEL) {
        mini.set_always_on_top(on).map_err(err)?;
    }
    Ok(())
}

/// Whether the mini player starts above other windows: the saved setting, on unless switched off.
fn mini_starts_on_top(app: &AppHandle) -> bool {
    app.try_state::<AppState>().and_then(|s| s.0.ui_state().pointer("/mini/onTop").and_then(serde_json::Value::as_bool)).unwrap_or(true)
}

/// Opens the small player, tucking the main window away (or closes it and brings the main window
/// back). Returns whether the mini player is now showing.
#[tauri::command(async)]
fn toggle_mini_player(app: AppHandle) -> CmdResult<bool> {
    if let Some(mini) = app.get_webview_window(MINI_LABEL) {
        let _ = mini.close();
        show_main_window(&app);
        return Ok(false);
    }
    let mini = WebviewWindowBuilder::new(&app, MINI_LABEL, WebviewUrl::App("index.html?mini=1".into()))
        .title("Redshift mini player")
        .inner_size(MINI_COMPACT.open.0, MINI_COMPACT.open.1)
        .min_inner_size(MINI_COMPACT.min.0, MINI_COMPACT.min.1)
        .max_inner_size(MINI_COMPACT.max.0, MINI_COMPACT.max.1)
        .resizable(true)
        .maximizable(false)
        .decorations(false)
        // Transparent and shadowless: the page draws its own rounded card, so the system adds no border.
        .transparent(true)
        .shadow(false)
        .always_on_top(mini_starts_on_top(&app))
        .build()
        .map_err(err)?;
    let _ = mini.set_focus();
    if let Some(main) = app.get_webview_window("main") {
        let _ = main.minimize();
    }
    Ok(true)
}

// ---- tray ----

const TRAY_ID: &str = "redshift";

fn show_main_window(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
    }
}

/// Adds or removes the tray icon so it's there exactly when one of the tray options is on.
fn sync_tray(app: &AppHandle, tray: TraySettings) -> tauri::Result<()> {
    let wanted = tray.minimize_to_tray || tray.close_to_tray;
    if !wanted {
        let _ = app.remove_tray_by_id(TRAY_ID);
        return Ok(());
    }
    if app.tray_by_id(TRAY_ID).is_some() {
        return Ok(());
    }
    let menu = Menu::with_items(
        app,
        &[
            &MenuItem::with_id(app, "show", "Show Redshift", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "toggle", "Play / Pause", true, None::<&str>)?,
            &MenuItem::with_id(app, "next", "Next", true, None::<&str>)?,
            &MenuItem::with_id(app, "previous", "Previous", true, None::<&str>)?,
            &PredefinedMenuItem::separator(app)?,
            &MenuItem::with_id(app, "quit", "Quit", true, None::<&str>)?,
        ],
    )?;
    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .tooltip("Redshift")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            let Some(state) = app.try_state::<AppState>() else { return };
            match event.id().as_ref() {
                "show" => show_main_window(app),
                "toggle" => state.0.player.toggle_pause(),
                "next" => state.0.player.next(),
                "previous" => state.0.player.previous(),
                "quit" => app.exit(0),
                _ => {}
            }
        })
        .on_tray_icon_event(|icon, event| {
            if let TrayIconEvent::Click { button: MouseButton::Left, button_state: MouseButtonState::Up, .. } = event {
                show_main_window(icon.app_handle());
            }
        });
    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }
    builder.build(app)?;
    Ok(())
}

#[tauri::command]
fn set_tray(app: AppHandle, state: State<AppState>, minimize_to_tray: bool, close_to_tray: bool) -> CmdResult<()> {
    let tray = TraySettings { minimize_to_tray, close_to_tray };
    state.0.set_tray(tray);
    sync_tray(&app, tray).map_err(err)
}

/// Serves cover art for `cover://localhost/<percent-encoded audio file path>`. `?s=<pixels>` asks for a
/// copy no bigger than that (lists and grids use it; the full picture is only for the big views).
fn serve_cover(app: &tauri::AppHandle, uri_path: &str, query: Option<&str>) -> tauri::http::Response<Cow<'static, [u8]>> {
    let path = percent_decode(uri_path.trim_start_matches('/'));
    let size = query
        .and_then(|q| q.split('&').find_map(|kv| kv.strip_prefix("s=")))
        .and_then(|v| v.parse::<u32>().ok())
        .map(|px| px.clamp(16, 2048));
    let cover = app.try_state::<AppState>().and_then(|s| match size {
        Some(px) => s.0.cover_sized(Path::new(&path), px),
        None => s.0.cover(Path::new(&path)),
    });
    match cover {
        Some(c) => tauri::http::Response::builder()
            .header("Content-Type", c.mime.clone())
            .header("Cache-Control", "max-age=3600")
            // The UI reads cover pixels (to theme itself from the artwork), which a canvas only
            // allows for images served with CORS enabled.
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(c.data.clone())),
        None => tauri::http::Response::builder().status(404).body(Cow::Borrowed(&[][..])),
    }
    .expect("static headers are valid")
}

/// Serves imported fonts for `font://localhost/<file name>`.
fn serve_font(app: &tauri::AppHandle, uri_path: &str) -> tauri::http::Response<Cow<'static, [u8]>> {
    let file = percent_decode(uri_path.trim_start_matches('/'));
    match app.try_state::<AppState>().and_then(|s| s.0.font_bytes(&file)) {
        Some((bytes, mime)) => tauri::http::Response::builder()
            .header("Content-Type", mime)
            .header("Cache-Control", "max-age=3600")
            .header("Access-Control-Allow-Origin", "*")
            .body(Cow::Owned(bytes)),
        None => tauri::http::Response::builder().status(404).body(Cow::Borrowed(&[][..])),
    }
    .expect("static headers are valid")
}

/// Answers cover and font requests on a few worker threads. Opening the library asks for dozens of
/// covers at once; a thread per request would flood the machine (and the disk) every time.
fn run_off_thread(job: impl FnOnce() + Send + 'static) {
    type Job = Box<dyn FnOnce() + Send>;
    static POOL: OnceLock<Sender<Job>> = OnceLock::new();
    const WORKERS: usize = 4;
    let tx = POOL.get_or_init(|| {
        let (tx, rx) = channel::<Job>();
        let rx: Arc<Mutex<Receiver<Job>>> = Arc::new(Mutex::new(rx));
        for n in 0..WORKERS {
            let rx = rx.clone();
            let _ = std::thread::Builder::new().name(format!("redshift-protocol-{n}")).spawn(move || loop {
                // Hold the lock only while waiting for the next job, never while running one.
                let next = rx.lock().map(|r| r.recv());
                match next {
                    Ok(Ok(job)) => { let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job)); }
                    _ => break,
                }
            });
        }
        tx
    });
    let _ = tx.send(Box::new(job));
}

fn main() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // These answer on a worker thread: reading a cover means parsing a whole audio file's tags,
        // which would otherwise freeze the window while it happens.
        .register_asynchronous_uri_scheme_protocol("cover", |ctx, request, responder| {
            let (app, path, query) = (ctx.app_handle().clone(), request.uri().path().to_string(), request.uri().query().map(str::to_string));
            run_off_thread(move || responder.respond(serve_cover(&app, &path, query.as_deref())));
        })
        .register_asynchronous_uri_scheme_protocol("font", |ctx, request, responder| {
            let (app, path) = (ctx.app_handle().clone(), request.uri().path().to_string());
            run_off_thread(move || responder.respond(serve_font(&app, &path)));
        })
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            // A second launch ("Open with Redshift", a double-click) hands its files to this one.
            open_files(app, &paths_from_args(argv.into_iter().skip(1)));
            show_main_window(app);
        }))
        .on_window_event(|window, event| {
            if window.label() != "main" {
                return;
            }
            let app = window.app_handle();
            // Files dropped on the window play; the UI shows a hint while they hover.
            if let WindowEvent::DragDrop(drag) = event {
                match drag {
                    DragDropEvent::Enter { .. } => { let _ = app.emit("drop-hover", true); }
                    DragDropEvent::Leave => { let _ = app.emit("drop-hover", false); }
                    DragDropEvent::Drop { paths, .. } => {
                        let _ = app.emit("drop-hover", false);
                        open_files(app, paths);
                    }
                    _ => {}
                }
                return;
            }
            let Some(state) = app.try_state::<AppState>() else { return };
            let tray = state.0.settings().tray;
            match event {
                WindowEvent::CloseRequested { api, .. } if tray.close_to_tray => {
                    api.prevent_close();
                    let _ = window.hide();
                }
                WindowEvent::CloseRequested { .. } => {
                    // Quitting for real: don't leave the mini player running on its own.
                    if let Some(mini) = app.get_webview_window(MINI_LABEL) {
                        let _ = mini.close();
                    }
                }
                WindowEvent::Resized(_) if tray.minimize_to_tray && window.is_minimized().unwrap_or(false) && app.get_webview_window(MINI_LABEL).is_none() => {
                    let _ = window.hide();
                }
                _ => {}
            }
        })
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

            // Counting plays needs a regular look at what the player is doing.
            let counter = core.clone();
            std::thread::Builder::new().name("play-counter".into()).spawn(move || loop {
                std::thread::sleep(Duration::from_millis(500));
                counter.poll_playback();
            })?;

            let tray = core.settings().tray;
            app.manage(AppState(core.clone()));
            sync_tray(app.handle(), tray)?;
            media::start(app.handle(), core);
            // Started with files ("Open with Redshift", or dragged onto the exe)?
            let opened = paths_from_args(std::env::args().skip(1));
            if !opened.is_empty() {
                open_files(app.handle(), &opened);
            }
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            set_queue, play_entry, pause, resume, toggle_pause, stop, next, previous, seek,
            queue_append, queue_play_next, queue_remove, queue_move, queue_clear, player_state, queue_view,
            get_settings, set_volume, set_muted, set_shuffle, set_repeat, set_crossfade_secs, set_replaygain,
            output_devices, set_output_device,
            eq_info, set_eq_enabled, set_eq_preamp_db, set_eq_band_db, set_eq_gains_db, apply_eq_preset, eq_curve,
            save_eq_preset, delete_eq_preset, set_tray, toggle_mini_player, mini_size, mini_on_top, favourite_paths, favourites, set_favourite, play_stats,
            reset_play_stats, fonts, import_font, remove_font, relink_playlist, paths_exist,
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
