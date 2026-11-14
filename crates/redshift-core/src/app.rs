//! The application layer: everything the window needs, behind plain Rust methods.
//!
//! [`App`] owns the player, the library, the playlists and the saved settings, and keeps them
//! consistent: changing the volume updates the player *and* what gets saved, a restart puts it all
//! back, and a rescan can't run twice at once. The Tauri shell only turns these methods into
//! commands, so everything of substance is covered by tests here without a GUI.
//!
//! Lock order, wherever more than one is held: library, then playlists, then settings.

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::dsp::eq::{EqParams, BAND_COUNT};
use crate::dsp::spectrum::SpectrumConfig;
use crate::engine::player::MAX_PREAMP_DB;
use crate::engine::{Player, PlayerConfig, PlayerEvent, ReplayGainMode};
use crate::error::{Error, Result};
use crate::library::{write_json_atomic, Album, Artist, Library, ScanProgress, ScanReport};
use crate::lyrics::{self, Lyrics};
use crate::meta::{self, Cover, TrackMeta};
use crate::playlists::{ImportReport, Playlist, PlaylistId, PlaylistStore, PlaylistSummary, RelinkReport};
use crate::queue::RepeatMode;
use crate::thumb;
use crate::resolve::Resolver;
use crate::history::{Heard, History};
use crate::insights::{self, Mix, MixInput, Report};
use crate::stats::{now_secs, PlayCounter, Stats};
use crate::waveform::{self, Overview};

/// How many waveform overviews are kept for replays.
const WAVEFORM_CACHE_SIZE: usize = 48;
/// How many covers (or "no cover" answers) are kept. The window asks for the playing track's
/// cover several times, and a library view asks for one per album; parsing a file's tags each
/// time is slow for large FLAC files.
const COVER_CACHE_SIZE: usize = 8;
/// How many bytes of small covers (list and grid tiles) are kept.
const THUMB_CACHE_BYTES: usize = 24 * 1024 * 1024;

pub const SETTINGS_FILE: &str = "settings.json";
pub const LIBRARY_FILE: &str = "library.json";
pub const PLAYLISTS_FILE: &str = "playlists.json";
pub const STATS_FILE: &str = "stats.json";
pub const HISTORY_FILE: &str = "history.json";
/// The listening history is written at most this often while playing (and when the app closes).
const HISTORY_SAVE_SECS: u64 = 60;
pub const SESSION_FILE: &str = "session.json";
/// The longest queue that is remembered between runs.
const SESSION_MAX_TRACKS: usize = 20_000;
/// How long the volume fades out before a sleep timer pauses the music.
const SLEEP_FADE_SECS: f64 = 10.0;
/// A song counts as finished for "end of this song" when this much of it is left; pausing then
/// lets the pause ramp finish inside the last moment of the song.
const SLEEP_END_MARGIN_SECS: f64 = 0.3;
/// How long after pausing the volume is put back, so the pause is not heard as a jump.
const SLEEP_RESTORE_DELAY: Duration = Duration::from_millis(600);
/// How far playback has to move before the saved position is rewritten.
const SESSION_POSITION_STEP_SECS: f64 = 15.0;

/// The front end can keep this much of its own state (theme, layout...) in the settings file.
const MAX_CUSTOM_PRESETS: usize = 50;
const MAX_PRESET_NAME_CHARS: usize = 40;
const MAX_UI_STATE_BYTES: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct EqSettings {
    pub enabled: bool,
    pub preamp_db: f32,
    pub gains_db: [f32; BAND_COUNT],
}

impl Default for EqSettings {
    fn default() -> Self {
        Self { enabled: true, preamp_db: 0.0, gains_db: [0.0; BAND_COUNT] }
    }
}

impl EqSettings {
    fn from_params(p: &EqParams) -> Self {
        Self { enabled: p.enabled(), preamp_db: p.preamp_db(), gains_db: p.gains_db() }
    }
}

/// An equaliser setting the person saved under their own name.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CustomPreset {
    pub name: String,
    pub preamp_db: f32,
    pub gains_db: [f32; BAND_COUNT],
}

/// What the window does instead of closing or sitting on the task bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct TraySettings {
    /// Minimising hides the window in the tray and the music keeps playing.
    pub minimize_to_tray: bool,
    /// The close button hides to the tray too; "Quit" in the tray menu really exits.
    pub close_to_tray: bool,
}

/// What a sleep timer is waiting for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum SleepKind {
    /// Until this moment, fading out over the last few seconds.
    At(Instant),
    /// Until the song playing now ends.
    EndOfSong(Option<crate::queue::EntryId>),
    /// Paused already; the volume goes back at this moment.
    Restoring(Instant),
}

/// For the window: whether a sleep timer is running and how long is left.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct SleepStatus {
    pub active: bool,
    /// `"time"` or `"song"` while active.
    pub kind: Option<&'static str>,
    /// Seconds until the music stops, for a timer set in minutes.
    pub remaining_secs: Option<f64>,
    /// The volume is on its way down.
    pub fading: bool,
}

/// The queue and place in it that are remembered between runs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Session {
    pub paths: Vec<PathBuf>,
    pub current: usize,
    pub position_secs: f64,
}

/// What's remembered between runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Volume slider position, 0..1.
    pub volume: f32,
    pub muted: bool,
    pub shuffle: bool,
    pub repeat: RepeatMode,
    pub crossfade_secs: f32,
    pub replaygain: ReplayGainMode,
    pub replaygain_preamp_db: f32,
    pub eq: EqSettings,
    /// Output device id; `None` is the system default.
    pub device: Option<String>,
    pub viz: SpectrumConfig,
    pub custom_eq_presets: Vec<CustomPreset>,
    pub tray: TraySettings,
    /// Start where you left off: the queue and the position in the song, paused.
    pub restore_session: bool,
    /// Look for new, changed and removed songs in the library folders now and then, and update the
    /// library when there are some.
    pub watch_folders: bool,
    /// Keep a log of what you listen to, for the Stats screen and the mixes.
    pub record_history: bool,
    /// How much of a song has to be heard to count as a play, in percent (1 to 100).
    pub count_percent: u8,
    /// Owned by the front end (theme, layout, customisation); stored as is.
    pub ui: Value,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            volume: 1.0,
            muted: false,
            shuffle: false,
            repeat: RepeatMode::Off,
            crossfade_secs: 0.0,
            replaygain: ReplayGainMode::Off,
            replaygain_preamp_db: 0.0,
            eq: EqSettings::default(),
            device: None,
            viz: SpectrumConfig::default(),
            custom_eq_presets: Vec::new(),
            tray: TraySettings::default(),
            restore_session: true,
            watch_folders: true,
            record_history: true,
            count_percent: 50,
            ui: Value::Null,
        }
    }
}

/// `x` if it's a usable number, else `fallback`.
fn finite_or(x: f32, fallback: f32) -> f32 {
    if x.is_finite() {
        x
    } else {
        fallback
    }
}

impl Settings {
    /// Brings every value into range, so a hand-edited or damaged file can't put the player in a
    /// state the UI can't produce.
    fn sanitized(mut self) -> Self {
        let d = Settings::default();
        self.volume = finite_or(self.volume, d.volume).clamp(0.0, 1.0);
        self.crossfade_secs = finite_or(self.crossfade_secs, 0.0).clamp(0.0, 12.0);
        self.replaygain_preamp_db = finite_or(self.replaygain_preamp_db, 0.0).clamp(-MAX_PREAMP_DB, MAX_PREAMP_DB);
        self.eq.preamp_db = finite_or(self.eq.preamp_db, 0.0);
        for g in &mut self.eq.gains_db {
            *g = finite_or(*g, 0.0);
        }
        self.custom_eq_presets.truncate(MAX_CUSTOM_PRESETS);
        for p in &mut self.custom_eq_presets {
            p.preamp_db = finite_or(p.preamp_db, 0.0);
            for g in &mut p.gains_db {
                *g = finite_or(*g, 0.0);
            }
        }
        let v = self.viz;
        let dv = SpectrumConfig::default();
        self.viz = SpectrumConfig {
            bars: v.bars,
            min_hz: finite_or(v.min_hz, dv.min_hz),
            max_hz: finite_or(v.max_hz, dv.max_hz),
            floor_db: finite_or(v.floor_db, dv.floor_db),
            ceil_db: finite_or(v.ceil_db, dv.ceil_db),
            fall_per_sec: finite_or(v.fall_per_sec, dv.fall_per_sec).max(0.0),
            peak_hold_secs: finite_or(v.peak_hold_secs, dv.peak_hold_secs).max(0.0),
        };
        self
    }
}

/// Reads the settings file. Missing means defaults; a file that can't be parsed is set aside as
/// `.bad` (so the person's choices aren't silently overwritten) and defaults are used.
fn load_settings(path: &Path) -> Settings {
    match fs::read(path) {
        Ok(bytes) => match serde_json::from_slice::<Settings>(&bytes) {
            Ok(s) => s.sanitized(),
            Err(_) => {
                let mut bad = path.as_os_str().to_owned();
                bad.push(".bad");
                let _ = fs::rename(path, PathBuf::from(bad));
                Settings::default()
            }
        },
        Err(_) => Settings::default(),
    }
}

/// Clears the "scan running" flag when dropped, including on panic.
struct ScanGuard<'a>(&'a AtomicBool);
impl Drop for ScanGuard<'_> {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
    }
}

/// Everything the song info dialog shows about one song.
#[derive(Debug, Clone, Serialize)]
pub struct SongInfo {
    pub meta: TrackMeta,
    pub in_library: bool,
    pub favourite: bool,
    /// Times listened to past the play threshold.
    pub plays: u32,
    /// Seconds since the epoch of the last start; 0 if never played.
    pub last_played: u64,
    /// Total seconds spent listening to it, from the history log.
    pub listened_secs: f64,
    /// When the history log first heard it (to within 15 minutes).
    pub first_heard: Option<u64>,
    /// "file" (an .lrc or .txt next to the song), "embedded" (in the tags) or "none".
    pub lyrics: &'static str,
}

/// How often and how recently a track was played.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlayStat {
    pub path: PathBuf,
    pub plays: u32,
    pub last_played: u64,
}

pub struct App {
    /// Transport, queue, seeking, visualiser and so on are used straight from the player; the
    /// methods below are for the things that also need saving.
    pub player: Player,
    data_dir: PathBuf,
    settings: Mutex<Settings>,
    settings_dirty: AtomicBool,
    library: Mutex<Library>,
    playlists: Mutex<PlaylistStore>,
    stats: Mutex<Stats>,
    stats_dirty: AtomicBool,
    history: Mutex<History>,
    history_dirty: AtomicBool,
    history_saved: Mutex<Instant>,
    /// Decides when a play counts, and when [`poll_playback`](App::poll_playback) last ran.
    counter: Mutex<(PlayCounter, Option<Instant>)>,
    scanning: AtomicBool,
    /// Recent waveform overviews, keyed by file, size, modified time and bucket count.
    waveforms: Mutex<VecDeque<(WaveformKey, Arc<Overview>)>>,
    /// Recent covers, keyed by file, size and modified time. `None` remembers a file with no cover.
    covers: Mutex<VecDeque<(CoverKey, Option<Arc<Cover>>)>>,
    /// Small covers for lists and grids, by file, modified time and size, kept up to a byte budget.
    thumbs: Mutex<VecDeque<(CoverKey, u32, Option<Arc<Cover>>)>>,
    /// What the session file holds: the queue's fingerprint and the saved position; `None` before the first save.
    session_saved: Mutex<Option<(u64, f64)>>,
    sleep: Mutex<Option<SleepKind>>,
}

type CoverKey = (PathBuf, u64, u64);

type WaveformKey = (PathBuf, u64, u64, usize);

impl App {
    /// Opens everything under `data_dir` (created if needed) and applies the saved settings to the
    /// player. If the saved output device is gone, the system default is used instead.
    pub fn new(data_dir: impl Into<PathBuf>, mut player_config: PlayerConfig) -> Result<(App, Receiver<PlayerEvent>)> {
        let data_dir = data_dir.into();
        fs::create_dir_all(&data_dir)?;
        let mut settings = load_settings(&data_dir.join(SETTINGS_FILE));

        let saved_device = settings.device.clone();
        if player_config.device.is_none() {
            player_config.device = saved_device.clone();
        }
        let (player, events) = match Player::new(player_config.clone()) {
            Ok(p) => p,
            Err(_) if player_config.device.is_some() => {
                player_config.device = None;
                settings.device = None;
                Player::new(player_config)?
            }
            Err(e) => return Err(e),
        };

        player.set_volume(settings.volume);
        player.set_muted(settings.muted);
        player.set_shuffle(settings.shuffle);
        player.set_repeat(settings.repeat);
        player.set_crossfade_secs(settings.crossfade_secs);
        player.set_replaygain(settings.replaygain, settings.replaygain_preamp_db);
        player.configure_viz(settings.viz);
        let eq = player.eq();
        eq.set_enabled(settings.eq.enabled);
        eq.set_preamp_db(settings.eq.preamp_db);
        eq.set_gains_db(&settings.eq.gains_db);

        let app = App {
            library: Mutex::new(Library::open(data_dir.join(LIBRARY_FILE))?),
            playlists: Mutex::new(PlaylistStore::open(data_dir.join(PLAYLISTS_FILE))?),
            stats: Mutex::new(Stats::open(data_dir.join(STATS_FILE))?),
            stats_dirty: AtomicBool::new(false),
            history: Mutex::new(History::open(data_dir.join(HISTORY_FILE))?),
            history_dirty: AtomicBool::new(false),
            history_saved: Mutex::new(Instant::now()),
            counter: Mutex::new((PlayCounter::default(), None)),
            player,
            settings_dirty: AtomicBool::new(settings.device != saved_device),
            settings: Mutex::new(settings),
            data_dir,
            scanning: AtomicBool::new(false),
            waveforms: Mutex::new(VecDeque::new()),
            covers: Mutex::new(VecDeque::new()),
            thumbs: Mutex::new(VecDeque::new()),
            session_saved: Mutex::new(None),
            sleep: Mutex::new(None),
        };
        Ok((app, events))
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    // ---- settings ----

    pub fn settings(&self) -> Settings {
        self.settings.lock().clone()
    }

    fn update(&self, f: impl FnOnce(&mut Settings)) {
        f(&mut self.settings.lock());
        self.settings_dirty.store(true, Ordering::Release);
    }

    /// Writes the settings if anything changed since the last write. Cheap to call often (the
    /// shell does so on a timer and on exit); setters only mark the settings as changed so that
    /// dragging a slider doesn't write a file per mouse move.
    pub fn flush(&self) -> Result<()> {
        self.save_session(false)?;
        self.save_history(false)?;
        if self.stats_dirty.swap(false, Ordering::AcqRel) {
            if let Err(e) = self.stats.lock().save() {
                self.stats_dirty.store(true, Ordering::Release);
                return Err(e);
            }
        }
        if self.settings_dirty.swap(false, Ordering::AcqRel) {
            let snapshot = self.settings();
            if let Err(e) = write_json_atomic(&self.data_dir.join(SETTINGS_FILE), &snapshot) {
                self.settings_dirty.store(true, Ordering::Release);
                return Err(e);
            }
        }
        Ok(())
    }

    // ---- sleep timer ----

    /// Stops the music after `minutes` (fading out at the end), or at the end of the song playing
    /// now when `minutes` is `None`. `Some(0)` cancels. A new timer replaces the old one.
    pub fn set_sleep_timer(&self, minutes: Option<u32>) {
        self.set_sleep_timer_in(minutes.map(|m| Duration::from_secs(u64::from(m.min(24 * 60)) * 60)));
    }

    fn set_sleep_timer_in(&self, after: Option<Duration>) {
        self.put_volume_back();
        *self.sleep.lock() = match after {
            Some(d) if d.is_zero() => None,
            Some(d) => Some(SleepKind::At(Instant::now() + d)),
            None => self.player.current_track().map(|c| SleepKind::EndOfSong(Some(c.entry_id))),
        };
    }

    /// Whether a timer is running or finishing, i.e. whether [`tick_sleep_timer`](App::tick_sleep_timer) has work.
    pub fn sleep_timer_running(&self) -> bool {
        self.sleep.lock().is_some()
    }

    pub fn cancel_sleep_timer(&self) {
        self.set_sleep_timer_in(Some(Duration::ZERO));
    }

    pub fn sleep_status(&self) -> SleepStatus {
        let now = Instant::now();
        match *self.sleep.lock() {
            Some(SleepKind::At(t)) => {
                let left = t.saturating_duration_since(now).as_secs_f64();
                SleepStatus { active: true, kind: Some("time"), remaining_secs: Some(left), fading: left < SLEEP_FADE_SECS }
            }
            Some(SleepKind::EndOfSong(_)) => SleepStatus { active: true, kind: Some("song"), remaining_secs: None, fading: false },
            Some(SleepKind::Restoring(_)) | None => SleepStatus { active: false, kind: None, remaining_secs: None, fading: false },
        }
    }

    /// The volume the person set, in case a fade moved the player's away from it.
    fn put_volume_back(&self) {
        let v = self.settings.lock().volume;
        self.player.set_volume(v);
    }

    /// Advances the sleep timer. Call a few times a second (the shell does, every 100 ms) so the
    /// fade is smooth. Cheap when no timer is running.
    pub fn tick_sleep_timer(&self) {
        let now = Instant::now();
        let mut guard = self.sleep.lock();
        let Some(kind) = *guard else { return };
        match kind {
            SleepKind::At(deadline) => {
                let left = deadline.saturating_duration_since(now).as_secs_f64();
                if left <= 0.0 {
                    self.player.pause();
                    *guard = Some(SleepKind::Restoring(now + SLEEP_RESTORE_DELAY));
                } else if left < SLEEP_FADE_SECS {
                    let x = (left / SLEEP_FADE_SECS) as f32;
                    // Eases off rather than dropping in a straight line.
                    self.player.set_volume(self.settings.lock().volume * x * x * (3.0 - 2.0 * x));
                }
            }
            SleepKind::EndOfSong(was) => {
                let Some(cur) = self.player.current_track() else {
                    *guard = None;
                    return;
                };
                if was != Some(cur.entry_id) {
                    // Skipped to another song since: the timer follows to the one playing now.
                    *guard = Some(SleepKind::EndOfSong(Some(cur.entry_id)));
                    return;
                }
                let left = cur.duration_secs.map(|d| d - self.player.position_secs());
                if self.player.status() == crate::engine::Status::Playing && left.is_some_and(|l| l <= SLEEP_END_MARGIN_SECS) {
                    self.player.pause();
                    *guard = None;
                }
            }
            SleepKind::Restoring(at) => {
                if now >= at {
                    *guard = None;
                    drop(guard);
                    self.put_volume_back();
                }
            }
        }
    }

    // ---- remembering the queue ----

    /// Writes the queue and position if they changed (the position only when it has moved a fair
    /// way, or with `force` when the app is closing). A long queue is only rewritten when it
    /// changes, not as the song plays.
    pub fn save_session(&self, force: bool) -> Result<()> {
        if !self.settings.lock().restore_session {
            return Ok(());
        }
        let stamp = self.player.queue_stamp();
        let pos = if self.player.status() == crate::engine::Status::Stopped { 0.0 } else { self.player.position_secs() };
        let mut saved = self.session_saved.lock();
        let step = if force { 0.5 } else { SESSION_POSITION_STEP_SECS };
        if saved.is_some_and(|(s, p)| s == stamp && (pos - p).abs() < step) {
            return Ok(());
        }
        let (mut paths, cur) = self.player.queue_paths();
        let mut current = cur.unwrap_or(0);
        if paths.len() > SESSION_MAX_TRACKS {
            let from = current.saturating_sub(SESSION_MAX_TRACKS / 2).min(paths.len() - SESSION_MAX_TRACKS);
            paths = paths[from..from + SESSION_MAX_TRACKS].to_vec();
            current -= from;
        }
        write_json_atomic(&self.data_dir.join(SESSION_FILE), &Session { paths, current, position_secs: pos })?;
        *saved = Some((stamp, pos));
        Ok(())
    }

    /// Puts the remembered queue back, paused at the remembered position. Files that have gone
    /// are left out. Returns whether anything was restored. Does nothing if the setting is off.
    pub fn restore_session(&self) -> bool {
        if !self.settings.lock().restore_session {
            return false;
        }
        let Ok(bytes) = fs::read(self.data_dir.join(SESSION_FILE)) else { return false };
        let Ok(session) = serde_json::from_slice::<Session>(&bytes) else { return false };
        let before = session.current.min(session.paths.len().saturating_sub(1));
        let mut paths = Vec::new();
        let mut start = None;
        for (i, p) in session.paths.iter().enumerate() {
            if !p.is_file() {
                continue;
            }
            if start.is_none() && i >= before {
                start = Some(paths.len());
            }
            paths.push(p.clone());
        }
        let Some(start) = start.or(if paths.is_empty() { None } else { Some(paths.len() - 1) }) else { return false };
        // Only the position of the very song that was playing still means anything.
        let secs = if session.paths.get(before) == paths.get(start) && session.position_secs.is_finite() { session.position_secs } else { 0.0 };
        self.player.restore_queue(paths, start, secs);
        true
    }

    pub fn set_restore_session(&self, on: bool) {
        self.update(|s| s.restore_session = on);
        if !on {
            let _ = fs::remove_file(self.data_dir.join(SESSION_FILE));
        } else {
            *self.session_saved.lock() = None;
        }
    }

    pub fn set_volume(&self, v: f32) {
        let v = finite_or(v, 1.0).clamp(0.0, 1.0);
        self.player.set_volume(v);
        self.update(|s| s.volume = v);
    }
    pub fn set_muted(&self, m: bool) {
        self.player.set_muted(m);
        self.update(|s| s.muted = m);
    }
    pub fn set_shuffle(&self, on: bool) {
        self.player.set_shuffle(on);
        self.update(|s| s.shuffle = on);
    }
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.player.set_repeat(mode);
        self.update(|s| s.repeat = mode);
    }
    pub fn set_crossfade_secs(&self, secs: f32) {
        let secs = finite_or(secs, 0.0).clamp(0.0, 12.0);
        self.player.set_crossfade_secs(secs);
        self.update(|s| s.crossfade_secs = secs);
    }
    pub fn set_replaygain(&self, mode: ReplayGainMode, preamp_db: f32) {
        self.player.set_replaygain(mode, preamp_db);
        let (mode, preamp_db) = self.player.replaygain();
        self.update(|s| {
            s.replaygain = mode;
            s.replaygain_preamp_db = preamp_db;
        });
    }
    pub fn set_device(&self, id: Option<String>) {
        self.player.set_device(id.clone());
        self.update(|s| s.device = id);
    }
    pub fn configure_viz(&self, cfg: SpectrumConfig) {
        let cfg = Settings { viz: cfg, ..Settings::default() }.sanitized().viz;
        self.player.configure_viz(cfg);
        // The analyser clamps the bar count; store what it actually uses.
        let applied = self.player.viz_config();
        self.update(|s| s.viz = applied);
    }

    fn eq_changed(&self) {
        let eq = EqSettings::from_params(self.player.eq());
        self.update(|s| s.eq = eq);
    }
    pub fn set_eq_enabled(&self, on: bool) {
        self.player.eq().set_enabled(on);
        self.eq_changed();
    }
    pub fn set_eq_preamp_db(&self, db: f32) {
        self.player.eq().set_preamp_db(finite_or(db, 0.0));
        self.eq_changed();
    }
    pub fn set_eq_band_db(&self, band: usize, db: f32) {
        self.player.eq().set_band_db(band, finite_or(db, 0.0));
        self.eq_changed();
    }
    pub fn set_eq_gains_db(&self, gains: [f32; BAND_COUNT]) {
        self.player.eq().set_gains_db(&gains.map(|g| finite_or(g, 0.0)));
        self.eq_changed();
    }
    /// The EQ's response as `[frequency_hz, gain_db]` points, log-spaced from 20 Hz to 20 kHz, for
    /// drawing the curve. Uses the same filter maths as the audio thread.
    pub fn eq_curve(&self, points: usize) -> Vec<[f32; 2]> {
        let n = points.clamp(2, 1024);
        let eq = self.player.eq();
        let (gains, preamp) = (eq.gains_db(), eq.preamp_db());
        let rate = self.player.state().sample_rate as f32;
        (0..n)
            .map(|i| {
                let f = 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32);
                [f, crate::dsp::eq::response_db(&gains, preamp, f, rate)]
            })
            .collect()
    }
    /// Applies a built-in preset or one the person saved. Returns false if there's no preset with
    /// that name. Built-in presets leave the pre-amp alone; saved ones restore theirs.
    pub fn apply_eq_preset(&self, name: &str) -> bool {
        let custom = self.settings.lock().custom_eq_presets.iter().find(|p| p.name.eq_ignore_ascii_case(name.trim())).cloned();
        if let Some(p) = custom {
            self.player.eq().set_preamp_db(p.preamp_db);
            self.player.eq().set_gains_db(&p.gains_db);
            self.eq_changed();
            return true;
        }
        let ok = self.player.eq().apply_preset(name);
        if ok {
            self.eq_changed();
        }
        ok
    }
    /// Saves the current equaliser curve and pre-amp under `name`, replacing a saved preset of the
    /// same name. Names of built-in presets are taken. Returns the name as stored.
    pub fn save_eq_preset(&self, name: &str) -> Result<String> {
        let name: String = name.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(MAX_PRESET_NAME_CHARS).collect();
        if name.is_empty() {
            return Err(Error::Other("give the preset a name".into()));
        }
        if crate::dsp::eq::PRESETS.iter().any(|(n, _)| n.eq_ignore_ascii_case(&name)) || name.eq_ignore_ascii_case("custom") {
            return Err(Error::Other(format!("\"{name}\" is a built-in preset name; pick another")));
        }
        let eq = EqSettings::from_params(self.player.eq());
        let mut full = false;
        self.update(|s| {
            let preset = CustomPreset { name: name.clone(), preamp_db: eq.preamp_db, gains_db: eq.gains_db };
            let existing = s.custom_eq_presets.iter().position(|p| p.name.eq_ignore_ascii_case(&name));
            match existing {
                Some(i) => s.custom_eq_presets[i] = preset,
                None if s.custom_eq_presets.len() >= MAX_CUSTOM_PRESETS => full = true,
                None => s.custom_eq_presets.push(preset),
            }
        });
        if full {
            return Err(Error::Other(format!("at most {MAX_CUSTOM_PRESETS} saved presets; delete one first")));
        }
        Ok(name)
    }
    /// Returns false if there was no saved preset with that name.
    pub fn delete_eq_preset(&self, name: &str) -> bool {
        let mut removed = false;
        self.update(|s| {
            let before = s.custom_eq_presets.len();
            s.custom_eq_presets.retain(|p| !p.name.eq_ignore_ascii_case(name.trim()));
            removed = s.custom_eq_presets.len() != before;
        });
        removed
    }

    // ---- tray ----

    pub fn set_tray(&self, tray: TraySettings) {
        self.update(|s| s.tray = tray);
    }

    /// The front end's own saved state (theme, layout...); `null` until it saves something.
    pub fn ui_state(&self) -> Value {
        self.settings.lock().ui.clone()
    }
    pub fn set_ui_state(&self, state: Value) -> Result<()> {
        let size = serde_json::to_vec(&state).map_err(|e| Error::Other(e.to_string()))?.len();
        if size > MAX_UI_STATE_BYTES {
            return Err(Error::Other(format!("UI state is too large ({size} bytes; the limit is {MAX_UI_STATE_BYTES})")));
        }
        self.update(|s| s.ui = state);
        Ok(())
    }

    // ---- library ----

    pub fn library_folders(&self) -> Vec<PathBuf> {
        self.library.lock().folders().to_vec()
    }

    /// Adds a folder to watch. Returns false if it was already there. Call
    /// [`scan_library`](Self::scan_library) afterwards to pick its tracks up.
    pub fn add_library_folder(&self, folder: &Path) -> Result<bool> {
        if !folder.is_dir() {
            return Err(Error::Other(format!("{} is not a folder", folder.display())));
        }
        let mut lib = self.library.lock();
        let added = lib.add_folder(folder);
        if added {
            lib.save()?;
        }
        Ok(added)
    }

    pub fn remove_library_folder(&self, folder: &Path) -> Result<bool> {
        let mut lib = self.library.lock();
        let removed = lib.remove_folder(folder);
        if removed {
            lib.save()?;
        }
        Ok(removed)
    }

    /// Rescans the library folders. Only one scan runs at a time; a second call while one is
    /// running is an error rather than a queue. The library isn't locked while files are read, so
    /// browsing stays responsive during a scan.
    pub fn scan_library(&self, progress: &mut dyn FnMut(ScanProgress)) -> Result<ScanReport> {
        if self.scanning.swap(true, Ordering::AcqRel) {
            return Err(Error::Other("a library scan is already running".into()));
        }
        let _guard = ScanGuard(&self.scanning);
        let (folders, known) = self.library.lock().scan_inputs();
        let result = crate::library::scan(&folders, &known, progress);
        let report = result.report.clone();
        let mut lib = self.library.lock();
        lib.apply_scan(result);
        lib.save()?;
        Ok(report)
    }

    /// If watching is on, nothing else is scanning and the folders changed since the last scan,
    /// scans them and returns what the scan found. `Ok(None)` means nothing was done.
    pub fn auto_rescan(&self, progress: &mut dyn FnMut(ScanProgress)) -> Result<Option<ScanReport>> {
        if !self.settings.lock().watch_folders || self.is_scanning() {
            return Ok(None);
        }
        let (folders, known) = self.library.lock().scan_inputs();
        if folders.is_empty() || !crate::library::has_changes(&folders, &known) {
            return Ok(None);
        }
        match self.scan_library(progress) {
            Ok(report) => Ok(Some(report)),
            // Someone else started a scan between the check and here: theirs covers it.
            Err(_) if self.is_scanning() => Ok(None),
            Err(e) => Err(e),
        }
    }

    pub fn set_watch_folders(&self, on: bool) {
        self.update(|s| s.watch_folders = on);
    }

    pub fn is_scanning(&self) -> bool {
        self.scanning.load(Ordering::Acquire)
    }

    pub fn track_count(&self) -> usize {
        self.library.lock().len()
    }
    pub fn tracks(&self) -> Vec<TrackMeta> {
        self.library.lock().tracks().into_iter().cloned().collect()
    }
    pub fn search(&self, query: &str) -> Vec<TrackMeta> {
        self.library.lock().search(query).into_iter().cloned().collect()
    }
    pub fn albums(&self) -> Vec<Album> {
        self.library.lock().albums()
    }
    pub fn artists(&self) -> Vec<Artist> {
        self.library.lock().artists()
    }
    pub fn recently_added(&self, limit: usize) -> Vec<TrackMeta> {
        self.library.lock().recently_added(limit).into_iter().cloned().collect()
    }
    /// Tags for any file: from the library if it's there, else read from disk.
    pub fn track_meta(&self, path: &Path) -> Result<TrackMeta> {
        if let Some(m) = self.library.lock().get(path) {
            return Ok(m.clone());
        }
        meta::read_meta(path)
    }

    // ---- editing tags ----

    /// The tag values the editor starts from.
    pub fn tag_form(&self, path: &Path) -> Result<crate::tags::TagForm> {
        crate::tags::read_form(path)
    }

    /// Writes tags, cover art and embedded lyrics into a file, then refreshes what the library
    /// remembers about it. Returns the track's new tags.
    pub fn write_tags(&self, path: &Path, edit: &crate::tags::TagEdit) -> Result<TrackMeta> {
        crate::tags::write_tags(path, edit)?;
        let fresh = meta::read_meta(path)?;
        let mut lib = self.library.lock();
        if lib.update_track(fresh.clone()) {
            lib.save()?;
        }
        Ok(fresh)
    }

    /// Saves lyrics as an `.lrc` or `.txt` file next to the song.
    pub fn save_lyrics_file(&self, audio: &Path, text: &str, synced: bool) -> Result<PathBuf> {
        crate::tags::write_sidecar_lyrics(audio, text, synced)
    }

    // ---- imported fonts ----

    fn fonts_dir(&self) -> PathBuf {
        self.data_dir.join("fonts")
    }
    pub fn fonts(&self) -> Vec<crate::fonts::FontEntry> {
        crate::fonts::list(&self.fonts_dir())
    }
    pub fn import_font(&self, src: &Path) -> Result<crate::fonts::FontEntry> {
        crate::fonts::import(&self.fonts_dir(), src)
    }
    /// Plays what the user opened the player with: files, folders or playlists (see [`crate::open::expand`]).
    /// Returns how many songs were queued; nothing changes if there were none.
    pub fn open_paths(&self, inputs: &[PathBuf]) -> usize {
        let songs = crate::open::expand(inputs);
        let n = songs.len();
        if n > 0 {
            self.player.set_queue(songs, 0, true);
        }
        n
    }

    pub fn remove_font(&self, file: &str) -> bool {
        crate::fonts::remove(&self.fonts_dir(), file)
    }
    pub fn font_bytes(&self, file: &str) -> Option<(Vec<u8>, &'static str)> {
        crate::fonts::read(&self.fonts_dir(), file)
    }

    // ---- play history and favourites ----

    fn with_stats<T>(&self, f: impl FnOnce(&mut Stats) -> T) -> T {
        let out = f(&mut self.stats.lock());
        self.stats_dirty.store(true, Ordering::Release);
        out
    }

    /// Call about once a second. Notices when a track starts and when it's been heard for long
    /// enough to count as a play. Cheap: it doesn't copy the queue.
    pub fn poll_playback(&self) {
        let now = Instant::now();
        let (current, playing, pos) = {
            let st = self.player.status();
            (self.player.current_track(), st == crate::engine::Status::Playing, self.player.position_secs())
        };
        let (tick, dt) = {
            let mut guard = self.counter.lock();
            let dt = guard.1.map_or(0.0, |t| now.duration_since(t).as_secs_f64());
            guard.1 = Some(now);
            (guard.0.tick(current.as_ref(), playing, pos, dt, f64::from(self.settings.lock().count_percent.clamp(1, 100)) / 100.0), dt)
        };
        if let Some(cur) = &current {
            if playing && self.settings.lock().record_history {
                self.history.lock().add(&cur.path, now_secs(), dt.clamp(0.0, 5.0), u16::from(tick.counted));
                self.history_dirty.store(true, Ordering::Release);
            }
        }
        if let Some(cur) = current {
            if tick.started || tick.counted {
                self.with_stats(|s| {
                    if tick.started {
                        s.record_start(&cur.path, now_secs());
                    }
                    if tick.counted {
                        s.record_play(&cur.path);
                    }
                });
            }
        }
    }

    // ---- listening history, stats and mixes ----

    /// Writes the listening history if it changed: at most once a minute, or at once with `force`.
    pub fn save_history(&self, force: bool) -> Result<()> {
        if !self.history_dirty.load(Ordering::Acquire) {
            return Ok(());
        }
        {
            let mut last = self.history_saved.lock();
            if !force && last.elapsed() < Duration::from_secs(HISTORY_SAVE_SECS) {
                return Ok(());
            }
            *last = Instant::now();
        }
        self.history_dirty.store(false, Ordering::Release);
        if let Err(e) = self.history.lock().save() {
            self.history_dirty.store(true, Ordering::Release);
            return Err(e);
        }
        Ok(())
    }
    /// How much of a song has to be heard to count as a play (percent, held between 1 and 100).
    pub fn set_count_percent(&self, percent: u8) {
        self.update(|s| s.count_percent = percent.clamp(1, 100));
    }
    pub fn set_record_history(&self, on: bool) {
        self.update(|s| s.record_history = on);
    }
    /// Forgets the whole listening log (play counts and favourites stay).
    pub fn clear_history(&self) -> Result<()> {
        self.history.lock().clear();
        self.history_dirty.store(true, Ordering::Release);
        self.save_history(true)
    }
    /// How much was heard in each of the ranges, given as `(from, to)` seconds since the epoch, `to` excluded.
    pub fn listening_totals(&self, ranges: &[(u64, u64)]) -> Vec<Heard> {
        let h = self.history.lock();
        ranges.iter().take(400).map(|&(a, b)| h.total(a, b)).collect()
    }
    /// Days in a row with a played song, in the person's own time zone.
    pub fn listening_streak(&self, tz_offset_min: i32) -> insights::Streak {
        let tz = tz_offset_min.clamp(-14 * 60, 14 * 60);
        let today = (now_secs() as i64 + tz as i64 * 60).div_euclid(86_400);
        insights::streak(&self.history.lock().play_days(tz), today)
    }
    /// The report for one range; `tz_offset_min` is the person's clock offset from UTC in minutes.
    pub fn listening_report(&self, from: u64, to: u64, tz_offset_min: i32) -> Report {
        let lib = self.library.lock();
        let tracks = lib.tracks();
        let by: std::collections::HashMap<&Path, &TrackMeta> = tracks.iter().map(|t| (t.path.as_path(), *t)).collect();
        insights::report(&self.history.lock(), &by, from, to, tz_offset_min.clamp(-14 * 60, 14 * 60))
    }
    /// Mixes made from the history, the play counts and the favourites, for today.
    pub fn mixes(&self) -> Vec<Mix> {
        let favourites: std::collections::HashSet<PathBuf> = self.stats.lock().favourites().into_iter().collect();
        let stats = self.stats.lock().all().clone();
        let lib = self.library.lock();
        let tracks = lib.tracks();
        insights::mixes(&MixInput { history: &self.history.lock(), lib: &tracks, stats: &stats, favourites: &favourites, now: now_secs() })
    }

    /// Play counts and last-played times of every track that has been played.
    pub fn play_stats(&self) -> Vec<PlayStat> {
        let mut v: Vec<PlayStat> = self
            .stats
            .lock()
            .all()
            .iter()
            .map(|(p, s)| PlayStat { path: p.clone(), plays: s.plays, last_played: s.last_played })
            .collect();
        v.sort_by(|a, b| a.path.cmp(&b.path));
        v
    }
    /// Everything known about one song, for its info dialog: tags, audio and file details, and how
    /// it has been listened to.
    pub fn song_info(&self, path: &Path) -> Result<SongInfo> {
        let in_library = self.library.lock().get(path).is_some();
        let meta = self.track_meta(path)?;
        let stat = self.stats.lock().get(path);
        let heard = self.history.lock().of_track(path);
        let lyrics = if lyrics::sidecar_path(path).is_some() {
            "file"
        } else if meta.has_embedded_lyrics {
            "embedded"
        } else {
            "none"
        };
        Ok(SongInfo {
            favourite: self.stats.lock().is_favourite(path),
            plays: stat.plays,
            last_played: stat.last_played,
            listened_secs: heard.secs,
            first_heard: heard.first,
            lyrics,
            in_library,
            meta,
        })
    }

    /// Forgets the play history of one track, or of everything if `path` is `None`.
    pub fn reset_play_stats(&self, path: Option<&Path>) {
        self.with_stats(|s| s.reset(path));
    }

    /// Favourite files, most recently favourited first (including ones no longer in the library).
    pub fn favourite_paths(&self) -> Vec<PathBuf> {
        self.stats.lock().favourites()
    }
    /// Favourites that are in the library, most recently favourited first.
    pub fn favourites(&self) -> Vec<TrackMeta> {
        let paths = self.favourite_paths();
        let lib = self.library.lock();
        paths.iter().filter_map(|p| lib.get(p).cloned()).collect()
    }
    /// Marks or unmarks files as favourites. Returns how many actually changed.
    pub fn set_favourite(&self, paths: &[PathBuf], on: bool) -> usize {
        let at = now_secs();
        self.with_stats(|s| paths.iter().filter(|p| s.set_favourite(p, on, at)).count())
    }

    // ---- per-track extras ----

    /// The loudness outline of a track for a waveform seek bar. Decodes the whole file the first
    /// time (run this off the UI thread) and remembers the result for replays.
    pub fn waveform(&self, path: &Path, buckets: usize) -> Result<Arc<Overview>> {
        if !meta::is_audio_file(path) {
            return Err(Error::Unsupported(format!("{} is not an audio file", path.display())));
        }
        let m = fs::metadata(path)?;
        let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
        let key: WaveformKey = (path.to_path_buf(), m.len(), mtime, buckets.clamp(8, 4096));
        if let Some((_, o)) = self.waveforms.lock().iter().find(|(k, _)| *k == key) {
            return Ok(o.clone());
        }
        let o = Arc::new(waveform::overview(path, buckets)?);
        let mut cache = self.waveforms.lock();
        cache.push_back((key, o.clone()));
        while cache.len() > WAVEFORM_CACHE_SIZE {
            cache.pop_front();
        }
        Ok(o)
    }

    /// Lyrics for an audio file. Anything that isn't an audio file gets `None`: these are called
    /// with paths from the UI, and shouldn't be a way to read arbitrary files.
    pub fn lyrics(&self, path: &Path) -> Option<Lyrics> {
        meta::is_audio_file(path).then(|| lyrics::find_lyrics(path)).flatten()
    }
    /// Cover art for an audio file (same restriction as [`lyrics`](Self::lyrics)).
    pub fn cover(&self, path: &Path) -> Option<Arc<Cover>> {
        if !meta::is_audio_file(path) {
            return None;
        }
        let key = fs::metadata(path).ok().map(|m| {
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            (path.to_path_buf(), m.len(), mtime)
        });
        if let Some(key) = &key {
            if let Some((_, hit)) = self.covers.lock().iter().find(|(k, _)| k == key) {
                return hit.clone();
            }
        }
        // Read outside the lock: a big file can take a while and other covers shouldn't wait.
        let found = meta::read_cover(path).map(Arc::new);
        if let Some(key) = key {
            let mut cache = self.covers.lock();
            cache.push_back((key, found.clone()));
            while cache.len() > COVER_CACHE_SIZE {
                cache.pop_front();
            }
        }
        found
    }

    /// Cover art no bigger than `max_px` on its longer side: lists and grids ask for these instead of
    /// the full picture. A cover that is already small enough is returned as it is.
    pub fn cover_sized(&self, path: &Path, max_px: u32) -> Option<Arc<Cover>> {
        if !meta::is_audio_file(path) {
            return None;
        }
        let key = fs::metadata(path).ok().map(|m| {
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            (path.to_path_buf(), m.len(), mtime)
        });
        if let Some(key) = &key {
            if let Some((_, _, hit)) = self.thumbs.lock().iter().find(|(k, px, _)| k == key && *px == max_px) {
                return hit.clone();
            }
        }
        // Read and shrink outside the lock: decoding a big picture takes a moment.
        let found = meta::read_cover(path).map(|full| match thumb::shrink(&full, max_px) {
            Some(small) => Arc::new(small),
            None => Arc::new(full),
        });
        if let Some(key) = key {
            let mut cache = self.thumbs.lock();
            cache.push_back((key, max_px, found.clone()));
            let mut bytes: usize = cache.iter().map(|(_, _, c)| c.as_ref().map_or(0, |c| c.data.len())).sum();
            while bytes > THUMB_CACHE_BYTES && cache.len() > 1 {
                if let Some((_, _, old)) = cache.pop_front() {
                    bytes -= old.map_or(0, |c| c.data.len());
                }
            }
        }
        found
    }

    // ---- playlists (each change is saved straight away) ----

    fn with_playlists<T>(&self, f: impl FnOnce(&mut PlaylistStore) -> Result<T>) -> Result<T> {
        let mut store = self.playlists.lock();
        let out = f(&mut store)?;
        store.save()?;
        Ok(out)
    }

    pub fn playlists(&self) -> Vec<PlaylistSummary> {
        self.playlists.lock().list()
    }
    pub fn playlist(&self, id: PlaylistId) -> Option<Playlist> {
        self.playlists.lock().get(id).cloned()
    }
    pub fn create_playlist(&self, name: &str, tracks: Vec<PathBuf>) -> Result<PlaylistId> {
        self.with_playlists(|s| Ok(s.create_with(name, tracks)))
    }
    pub fn rename_playlist(&self, id: PlaylistId, name: &str) -> Result<()> {
        self.with_playlists(|s| s.rename(id, name))
    }
    pub fn delete_playlist(&self, id: PlaylistId) -> Result<()> {
        self.with_playlists(|s| s.delete(id))
    }
    pub fn move_playlist(&self, id: PlaylistId, to: usize) -> Result<()> {
        self.with_playlists(|s| s.move_playlist(id, to))
    }
    pub fn add_to_playlist(&self, id: PlaylistId, tracks: &[PathBuf], position: Option<usize>) -> Result<()> {
        self.with_playlists(|s| s.add_tracks(id, tracks, position))
    }
    pub fn remove_from_playlist(&self, id: PlaylistId, positions: &[usize]) -> Result<usize> {
        self.with_playlists(|s| s.remove_at(id, positions))
    }
    pub fn move_in_playlist(&self, id: PlaylistId, from: usize, to: usize) -> Result<()> {
        self.with_playlists(|s| s.move_track(id, from, to))
    }
    fn resolver(&self) -> Resolver {
        Resolver::new(self.library.lock().tracks())
    }
    /// Imports an M3U/M3U8 file. Entries whose files aren't where the playlist says are matched
    /// to library files by name and tags; see [`ImportReport`] for what was found.
    pub fn import_m3u(&self, file: &Path) -> Result<ImportReport> {
        let resolver = self.resolver();
        self.with_playlists(|s| s.import_m3u(file, &|p, label| resolver.resolve(p, label)))
    }
    /// Retries the entries of a playlist that point at missing files, e.g. after adding a folder.
    pub fn relink_playlist(&self, id: PlaylistId) -> Result<RelinkReport> {
        let resolver = self.resolver();
        self.with_playlists(|s| s.relink(id, &|p, label| resolver.resolve(p, label)))
    }
    /// Which of these files exist right now, in the same order.
    pub fn paths_exist(&self, paths: &[PathBuf]) -> Vec<bool> {
        paths.iter().map(|p| fs::metadata(p).is_ok()).collect()
    }
    /// Writes a playlist as M3U8, with durations and "Artist - Title" for tracks in the library.
    pub fn export_m3u(&self, id: PlaylistId, dest: &Path) -> Result<()> {
        let lib = self.library.lock();
        let store = self.playlists.lock();
        store.export_m3u(id, dest, &|p| lib.get(p).map(|m| (m.duration_secs, format!("{} - {}", m.artist, m.title))))
    }
    /// Replaces the queue with a playlist and starts playing it from `start`.
    pub fn play_playlist(&self, id: PlaylistId, start: usize) -> Result<()> {
        let tracks = self.playlist(id).ok_or_else(|| Error::Other(format!("no playlist with id {id}")))?.tracks;
        if tracks.is_empty() {
            return Err(Error::Other("the playlist is empty".into()));
        }
        // Entries whose files are gone are left out rather than failing one by one; `start` keeps
        // pointing at the same track, or the next playable one if that track is missing.
        let exists = self.paths_exist(&tracks);
        let playable = exists.iter().filter(|&&e| e).count();
        if playable == 0 {
            return Err(Error::Other(format!(
                "none of the {} tracks in this playlist could be found on this PC. Try Relink after adding the folder that holds your music",
                tracks.len()
            )));
        }
        let start = start.min(tracks.len() - 1);
        let new_start = exists[..start].iter().filter(|&&e| e).count().min(playable - 1);
        let tracks: Vec<PathBuf> = tracks.into_iter().zip(exists).filter_map(|(t, e)| e.then_some(t)).collect();
        self.player.set_queue(tracks, new_start, true);
        Ok(())
    }
}

impl Drop for App {
    fn drop(&mut self) {
        let _ = self.save_history(true);
        let _ = self.flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineConfig;
    use std::sync::Arc;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    fn offline() -> PlayerConfig {
        PlayerConfig { device: None, offline: Some(OfflineConfig { sample_rate: 48_000, sink: Arc::new(Mutex::new(Vec::new())), speed: 4.0 }) }
    }

    fn open(dir: &Path) -> App {
        App::new(dir, offline()).unwrap().0
    }

    fn music() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        fs::copy(fixture("tagged.flac"), d.path().join("a.flac")).unwrap();
        fs::copy(fixture("tagged.mp3"), d.path().join("b.mp3")).unwrap();
        fs::copy(fixture("tagged.opus"), d.path().join("c.opus")).unwrap();
        d
    }

    fn wait_for(what: &str, mut ok: impl FnMut() -> bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while !ok() {
            assert!(std::time::Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn song_info_gathers_tags_plays_favourite_and_lyrics() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        let a = m.path().join("a.flac");

        let info = app.song_info(&a).unwrap();
        assert!(info.in_library && !info.favourite);
        assert_eq!((info.plays, info.last_played, info.first_heard), (0, 0, None));
        assert_eq!(info.lyrics, if info.meta.has_embedded_lyrics { "embedded" } else { "none" });
        assert_eq!(info.meta.format, "flac");

        app.set_favourite(&[a.clone()], true);
        app.history.lock().add(&a, now_secs(), 42.0, 1);
        fs::write(m.path().join("a.lrc"), "[00:01.00]hi").unwrap();
        let info = app.song_info(&a).unwrap();
        assert!(info.favourite);
        assert_eq!(info.listened_secs, 42.0);
        assert!(info.first_heard.is_some());
        assert_eq!(info.lyrics, "file");

        // A file outside the library still gets its details from disk.
        let outside = tempfile::tempdir().unwrap();
        fs::copy(fixture("tagged.mp3"), outside.path().join("x.mp3")).unwrap();
        let info = app.song_info(&outside.path().join("x.mp3")).unwrap();
        assert!(!info.in_library);
        assert!(app.song_info(&outside.path().join("missing.mp3")).is_err());
    }

    #[test]
    fn folders_are_rescanned_by_themselves_when_something_changes() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        let n = app.track_count();
        assert!(n >= 3);
        assert!(app.auto_rescan(&mut |_| {}).unwrap().is_none(), "nothing changed, nothing scanned");

        let extra = m.path().join("sub");
        fs::create_dir_all(&extra).unwrap();
        fs::copy(m.path().join("a.flac"), extra.join("new song.flac")).unwrap();
        let r = app.auto_rescan(&mut |_| {}).unwrap().expect("a new file is picked up");
        assert_eq!((r.added, r.removed), (1, 0));
        assert_eq!(app.track_count(), n + 1);
        assert!(app.auto_rescan(&mut |_| {}).unwrap().is_none(), "and then it is up to date again");

        fs::remove_file(m.path().join("b.mp3")).unwrap();
        let r = app.auto_rescan(&mut |_| {}).unwrap().expect("a removed file is noticed");
        assert_eq!(r.removed, 1);
        assert_eq!(app.track_count(), n);

        // A file changed in place (its size differs) counts too.
        let c = m.path().join("c.opus");
        let mut bytes = fs::read(&c).unwrap();
        bytes.extend_from_slice(&[0; 16]);
        fs::write(&c, bytes).unwrap();
        assert!(app.auto_rescan(&mut |_| {}).unwrap().is_some());
    }

    #[test]
    fn watching_can_be_switched_off() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        fs::copy(m.path().join("a.flac"), m.path().join("later.flac")).unwrap();
        app.set_watch_folders(false);
        assert!(app.auto_rescan(&mut |_| {}).unwrap().is_none());
        app.set_watch_folders(true);
        assert!(app.auto_rescan(&mut |_| {}).unwrap().is_some());
        // No folders at all: nothing to look at.
        let empty = open(tempfile::tempdir().unwrap().path());
        assert!(empty.auto_rescan(&mut |_| {}).unwrap().is_none());
    }

    #[test]
    fn a_sleep_timer_fades_the_volume_then_pauses_and_puts_it_back() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_volume(0.8);
        app.player.set_queue(vec![m.path().join("a.flac"), m.path().join("b.mp3")], 0, true);
        wait_for("playback", || app.player.status() == crate::engine::Status::Playing);
        assert!(!app.sleep_status().active);
        app.set_sleep_timer_in(Some(Duration::from_secs(4)));
        let st = app.sleep_status();
        assert!(st.active && st.kind == Some("time") && st.fading && st.remaining_secs.unwrap() <= 4.0, "{st:?}");
        app.tick_sleep_timer();
        assert!(app.player.volume() < 0.5, "four seconds from the end the volume is already coming down: {}", app.player.volume());
        // Time passes: move the deadline instead of waiting for it.
        *app.sleep.lock() = Some(SleepKind::At(Instant::now() - Duration::from_millis(1)));
        app.tick_sleep_timer();
        wait_for("pause", || app.player.status() == crate::engine::Status::Paused);
        assert!(!app.sleep_status().active);
        *app.sleep.lock() = Some(SleepKind::Restoring(Instant::now() - Duration::from_millis(1)));
        app.tick_sleep_timer();
        assert!((app.player.volume() - 0.8).abs() < 1e-4, "volume back to what was set: {}", app.player.volume());
        assert!(app.sleep.lock().is_none());
    }

    #[test]
    fn cancelling_a_sleep_timer_puts_the_volume_back() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_volume(0.9);
        app.player.set_queue(vec![m.path().join("a.flac")], 0, true);
        wait_for("playback", || app.player.status() == crate::engine::Status::Playing);
        app.set_sleep_timer(Some(1));
        *app.sleep.lock() = Some(SleepKind::At(Instant::now() + Duration::from_secs(3)));
        app.tick_sleep_timer();
        assert!(app.player.volume() < 0.9);
        app.cancel_sleep_timer();
        assert!((app.player.volume() - 0.9).abs() < 1e-4);
        assert!(!app.sleep_status().active);
        app.set_sleep_timer(Some(0));
        assert!(!app.sleep_status().active, "zero minutes means off");
    }

    #[test]
    fn end_of_song_pauses_before_the_next_one_starts() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        let (a, b) = (m.path().join("a.flac"), m.path().join("b.mp3"));
        app.player.set_queue(vec![a.clone(), b], 0, true);
        wait_for("playback", || app.player.status() == crate::engine::Status::Playing);
        app.set_sleep_timer(None);
        assert_eq!(app.sleep_status().kind, Some("song"));
        wait_for("the song to end and the music to pause", || {
            app.tick_sleep_timer();
            app.player.status() == crate::engine::Status::Paused
        });
        assert_eq!(app.player.current_track().unwrap().path, a, "stayed on the first song");
        assert!(!app.sleep_status().active);
        assert!((app.player.volume() - 1.0).abs() < 1e-4, "no fade for end-of-song");
    }

    #[test]
    fn end_of_song_with_nothing_playing_sets_no_timer() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_sleep_timer(None);
        assert!(!app.sleep_status().active);
    }

    #[test]
    fn the_queue_and_position_come_back_after_a_restart() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let (a, b, c) = (m.path().join("a.flac"), m.path().join("b.mp3"), m.path().join("c.opus"));
        {
            let app = open(d.path());
            app.player.set_queue(vec![a.clone(), b.clone(), c.clone()], 1, true);
            wait_for("playback", || app.player.status() == crate::engine::Status::Playing);
            app.player.pause();
            app.player.seek(0.6);
            wait_for("seek", || (app.player.position_secs() - 0.6).abs() < 0.2);
            app.save_session(true).unwrap();
        }
        let app = open(d.path());
        assert!(app.restore_session());
        wait_for("restored queue", || app.player.queue().entries.len() == 3 && app.player.current_track().is_some());
        let q = app.player.queue();
        assert_eq!(q.entries.iter().map(|e| e.path.clone()).collect::<Vec<_>>(), vec![a, b.clone(), c]);
        assert_eq!(app.player.current_track().unwrap().path, b);
        assert_eq!(app.player.status(), crate::engine::Status::Paused, "restored paused, not playing");
        assert!((app.player.position_secs() - 0.6).abs() < 0.2, "position {}", app.player.position_secs());
    }

    #[test]
    fn songs_that_are_gone_are_left_out_and_the_next_one_takes_over() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let (a, b, c) = (m.path().join("a.flac"), m.path().join("b.mp3"), m.path().join("c.opus"));
        write_json_atomic(&d.path().join(SESSION_FILE), &Session { paths: vec![a.clone(), b.clone(), c.clone()], current: 1, position_secs: 1.0 }).unwrap();
        fs::remove_file(&b).unwrap();
        let app = open(d.path());
        assert!(app.restore_session());
        wait_for("queue", || app.player.queue().entries.len() == 2 && app.player.current_track().is_some());
        assert_eq!(app.player.current_track().unwrap().path, c);
        assert!(app.player.position_secs() < 0.3, "the old position belonged to the song that is gone");
    }

    #[test]
    fn nothing_is_restored_when_off_or_empty_or_damaged() {
        let d = tempfile::tempdir().unwrap();
        assert!(!open(d.path()).restore_session(), "no session file");
        fs::write(d.path().join(SESSION_FILE), "{ not json").unwrap();
        assert!(!open(d.path()).restore_session());
        let m = music();
        write_json_atomic(&d.path().join(SESSION_FILE), &Session { paths: vec![m.path().join("a.flac")], current: 0, position_secs: 0.0 }).unwrap();
        let app = open(d.path());
        app.set_restore_session(false);
        assert!(!app.restore_session());
        assert!(!d.path().join(SESSION_FILE).exists(), "turning it off forgets the saved queue");
        app.save_session(true).unwrap();
        assert!(!d.path().join(SESSION_FILE).exists(), "and nothing is saved while it is off");
    }

    #[test]
    fn the_session_is_only_rewritten_when_something_changed() {
        let m = music();
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.player.set_queue(vec![m.path().join("a.flac"), m.path().join("b.mp3")], 0, false);
        wait_for("queue", || app.player.queue().entries.len() == 2);
        app.save_session(false).unwrap();
        let file = d.path().join(SESSION_FILE);
        let first = fs::metadata(&file).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(30));
        app.save_session(false).unwrap();
        assert_eq!(fs::metadata(&file).unwrap().modified().unwrap(), first, "unchanged: not written again");
        app.player.append(vec![m.path().join("c.opus")]);
        wait_for("append", || app.player.queue().entries.len() == 3);
        app.save_session(false).unwrap();
        let s: Session = serde_json::from_slice(&fs::read(&file).unwrap()).unwrap();
        assert_eq!(s.paths.len(), 3);
    }

    #[test]
    fn fresh_start_uses_defaults() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        assert_eq!(app.settings(), Settings::default());
        assert!((app.player.volume() - 1.0).abs() < 1e-6);
        assert!(app.playlists().is_empty() && app.tracks().is_empty());
    }

    #[test]
    fn settings_survive_a_restart_and_are_applied_to_the_player() {
        let d = tempfile::tempdir().unwrap();
        {
            let app = open(d.path());
            app.set_volume(0.4);
            app.set_muted(true);
            app.set_shuffle(true);
            app.set_repeat(RepeatMode::All);
            app.set_crossfade_secs(3.5);
            app.set_replaygain(ReplayGainMode::Album, 2.0);
            app.set_eq_enabled(false);
            app.set_eq_preamp_db(-2.0);
            app.set_eq_band_db(3, 4.5);
            app.configure_viz(SpectrumConfig { bars: 32, ..Default::default() });
            app.set_ui_state(serde_json::json!({"theme": {"accent": "#ff8800"}, "sidebar": true})).unwrap();
            app.flush().unwrap();
        }
        let app = open(d.path());
        let p = &app.player;
        assert!((p.volume() - 0.4).abs() < 1e-4);
        let st = p.state();
        assert!(st.muted);
        assert!((st.crossfade_secs - 3.5).abs() < 1e-6);
        assert_eq!((st.replaygain, st.replaygain_preamp_db), (ReplayGainMode::Album, 2.0));
        assert!(!p.eq().enabled());
        assert_eq!(p.eq().preamp_db(), -2.0);
        assert_eq!(p.eq().band_db(3), 4.5);
        assert_eq!(p.viz_config().bars, 32);
        let s = app.settings();
        assert!(s.shuffle && s.repeat == RepeatMode::All);
        assert_eq!(app.ui_state()["theme"]["accent"], "#ff8800");
        // Queue modes are applied by the control thread, which publishes the queue view when done.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while p.queue().repeat != RepeatMode::All || !p.queue().shuffle {
            assert!(std::time::Instant::now() < deadline, "saved shuffle/repeat never reached the queue: {:?}", p.queue());
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn dropping_the_app_saves_pending_changes() {
        let d = tempfile::tempdir().unwrap();
        {
            let app = open(d.path());
            app.set_volume(0.25);
        } // no explicit flush
        assert!((open(d.path()).player.volume() - 0.25).abs() < 1e-4);
    }

    #[test]
    fn setters_only_write_when_flushed() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.flush().unwrap();
        assert!(!d.path().join(SETTINGS_FILE).exists(), "nothing changed, nothing written");
        app.set_volume(0.5);
        assert!(!d.path().join(SETTINGS_FILE).exists(), "setting a value doesn't write by itself");
        app.flush().unwrap();
        assert!(d.path().join(SETTINGS_FILE).exists());
        fs::remove_file(d.path().join(SETTINGS_FILE)).unwrap();
        app.flush().unwrap();
        assert!(!d.path().join(SETTINGS_FILE).exists(), "already flushed, so not written again");
    }

    #[test]
    fn bad_numbers_are_ignored_not_stored() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_volume(f32::NAN);
        app.set_volume(7.0);
        assert_eq!(app.settings().volume, 1.0);
        app.set_crossfade_secs(f32::INFINITY);
        assert_eq!(app.settings().crossfade_secs, 0.0);
        app.set_crossfade_secs(99.0);
        assert_eq!(app.settings().crossfade_secs, 12.0);
        app.set_eq_band_db(0, f32::NAN);
        app.set_eq_band_db(1, 99.0);
        app.set_eq_band_db(500, 3.0); // no such band
        let g = app.settings().eq.gains_db;
        assert_eq!((g[0], g[1]), (0.0, 12.0));
        app.set_replaygain(ReplayGainMode::Track, 50.0);
        assert_eq!(app.settings().replaygain_preamp_db, 12.0);
        app.configure_viz(SpectrumConfig { bars: 100_000, min_hz: f32::NAN, ..Default::default() });
        let v = app.settings().viz;
        assert_eq!(v.bars, 256);
        assert!(v.min_hz.is_finite());
    }

    #[test]
    fn eq_presets_apply_and_are_remembered() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        assert!(app.apply_eq_preset("bass boost") || app.apply_eq_preset(crate::dsp::eq::PRESETS[1].0));
        assert!(app.settings().eq.gains_db.iter().any(|g| *g != 0.0));
        assert_eq!(app.settings().eq.gains_db, app.player.eq().gains_db());
        let before = app.settings();
        assert!(!app.apply_eq_preset("no such preset"));
        assert_eq!(app.settings(), before, "unknown preset changes nothing");
    }

    #[test]
    fn eq_curve_follows_the_bands_and_preamp() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        let flat = app.eq_curve(50);
        assert_eq!(flat.len(), 50);
        assert!((flat[0][0] - 20.0).abs() < 1e-3 && (flat[49][0] - 20_000.0).abs() < 1.0, "20 Hz to 20 kHz");
        assert!(flat.iter().all(|p| p[1].abs() < 0.01), "flat EQ is a flat line");
        app.set_eq_band_db(0, 9.0);
        let c = app.eq_curve(50);
        assert!(c[0][1] > 3.0, "boosting the lowest band lifts the bottom: {}", c[0][1]);
        assert!(c[49][1].abs() < 0.5, "and leaves the top alone: {}", c[49][1]);
        app.set_eq_preamp_db(-3.0);
        assert!((app.eq_curve(50)[49][1] - (c[49][1] - 3.0)).abs() < 0.05, "preamp shifts the whole curve");
        assert_eq!(app.eq_curve(0).len(), 2, "point count is kept sensible");
    }

    #[test]
    fn waveform_is_cached_per_file_and_refuses_non_audio() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        let p = fixture("tagged.flac");
        let a = app.waveform(&p, 64).unwrap();
        let b = app.waveform(&p, 64).unwrap();
        assert!(Arc::ptr_eq(&a, &b), "second call reuses the first result");
        assert_eq!(app.waveform(&p, 32).unwrap().peaks.len(), 32, "other sizes are separate entries");
        fs::write(d.path().join("x.txt"), "text").unwrap();
        assert!(app.waveform(&d.path().join("x.txt"), 64).is_err());
        for i in 0..(WAVEFORM_CACHE_SIZE + 5) {
            app.waveform(&p, 100 + i).unwrap();
        }
        assert!(app.waveforms.lock().len() <= WAVEFORM_CACHE_SIZE, "cache is bounded");
    }

    #[test]
    fn lyrics_and_cover_refuse_non_audio_paths() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        fs::write(d.path().join("secret.txt"), "[00:01.00]hello").unwrap();
        fs::write(d.path().join("cover.jpg"), [0xFF, 0xD8, 0xFF]).unwrap();
        assert!(app.lyrics(&d.path().join("secret.txt")).is_none());
        assert!(app.cover(&d.path().join("secret.txt")).is_none(), "even with a cover.jpg beside it");
    }

    #[test]
    fn a_damaged_or_out_of_range_settings_file_cannot_break_startup() {
        let d = tempfile::tempdir().unwrap();
        let path = d.path().join(SETTINGS_FILE);

        fs::write(&path, "{{ nope").unwrap();
        let app = open(d.path());
        assert_eq!(app.settings(), Settings::default());
        assert_eq!(fs::read_to_string(d.path().join("settings.json.bad")).unwrap(), "{{ nope");
        drop(app);

        fs::write(&path, r#"{"volume": 40, "crossfade_secs": -3, "replaygain_preamp_db": 900, "eq": {"gains_db": [1,2,3,4,5,6,7,8,9,10]}}"#).unwrap();
        let app = open(d.path());
        let s = app.settings();
        assert_eq!((s.volume, s.crossfade_secs, s.replaygain_preamp_db), (1.0, 0.0, 12.0));
        assert_eq!(s.eq.gains_db[9], 10.0, "partial files are fine; missing fields take defaults");
        assert!(s.eq.enabled);
    }

    #[test]
    fn oversized_ui_state_is_refused() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_ui_state(serde_json::json!({"ok": 1})).unwrap();
        let big = Value::String("x".repeat(MAX_UI_STATE_BYTES + 1));
        assert!(app.set_ui_state(big).is_err());
        assert_eq!(app.ui_state()["ok"], 1, "the old state is kept");
    }

    #[test]
    fn folders_scan_and_browse_and_persist() {
        let state = tempfile::tempdir().unwrap();
        let m = music();
        {
            let app = open(state.path());
            assert!(app.add_library_folder(m.path()).unwrap());
            assert!(!app.add_library_folder(m.path()).unwrap());
            assert!(app.add_library_folder(&m.path().join("nope")).is_err(), "must be an existing folder");
            let mut updates = 0;
            let report = app.scan_library(&mut |_| updates += 1).unwrap();
            assert_eq!((report.added, report.failed.len()), (3, 0));
            assert!(updates > 0 && !app.is_scanning());
            assert_eq!(app.track_count(), 3);
            assert_eq!(app.search("mp3 song").len(), 1);
            assert_eq!(app.albums().len(), 3);
            assert_eq!(app.artists().len(), 3);
            assert_eq!(app.recently_added(2).len(), 2);
            let second = app.scan_library(&mut |_| {}).unwrap();
            assert_eq!((second.added, second.unchanged), (0, 3));
        }
        let app = open(state.path());
        assert_eq!(app.library_folders(), vec![m.path().to_path_buf()]);
        assert_eq!(app.track_count(), 3, "the library was saved by the scan");
        assert!(app.remove_library_folder(m.path()).unwrap());
        assert_eq!(app.track_count(), 0);
    }

    #[test]
    fn only_one_scan_runs_at_a_time_and_the_flag_clears_afterwards() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        app.scanning.store(true, Ordering::Release);
        assert!(app.is_scanning());
        assert!(app.scan_library(&mut |_| {}).is_err());
        assert!(app.is_scanning(), "a refused scan doesn't clear the running scan's flag");
        app.scanning.store(false, Ordering::Release);
        app.scan_library(&mut |_| {}).unwrap();
        assert!(!app.is_scanning());
    }

    #[test]
    fn covers_are_remembered_and_the_cache_is_bounded() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        let a = app.cover(&fixture("tagged.flac")).expect("has a cover");
        let b = app.cover(&fixture("tagged.flac")).expect("still has one");
        assert!(Arc::ptr_eq(&a, &b), "the second ask is served from the cache");
        assert!(app.cover(&fixture("chirp_44k_stereo.flac")).is_none());
        assert!(app.cover(&fixture("chirp_44k_stereo.flac")).is_none(), "a miss is remembered too");
        assert!(app.covers.lock().len() <= COVER_CACHE_SIZE);
    }

    #[test]
    fn small_covers_are_cached_per_size_and_never_bigger_than_the_original() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        let full = app.cover(&fixture("tagged.flac")).expect("has a cover");
        let a = app.cover_sized(&fixture("tagged.flac"), 32).expect("has a cover");
        let b = app.cover_sized(&fixture("tagged.flac"), 32).expect("still has one");
        assert!(Arc::ptr_eq(&a, &b), "the second ask is served from the cache");
        assert!(a.data.len() <= full.data.len());
        assert!(app.cover_sized(&fixture("chirp_44k_stereo.flac"), 32).is_none());
        assert!(app.cover_sized(Path::new("notes.txt"), 32).is_none(), "only audio files");
    }

    #[test]
    fn track_meta_falls_back_to_reading_the_file() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        let m = app.track_meta(&fixture("tagged.flac")).unwrap();
        assert_eq!(m.title, "Tagged Song");
        assert!(app.track_meta(&fixture("does_not_exist.flac")).is_err());
    }

    #[test]
    fn lyrics_and_cover_come_from_the_files() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        let l = app.lyrics(&fixture("tagged.flac")).expect("embedded lyrics");
        assert!(l.synced && l.lines.len() == 2);
        assert!(app.cover(&fixture("tagged.flac")).is_some());
        assert!(app.lyrics(&fixture("chirp_44k_stereo.flac")).is_none());
        assert!(app.cover(&fixture("chirp_44k_stereo.flac")).is_none());
    }

    #[test]
    fn playlists_persist_every_change() {
        let state = tempfile::tempdir().unwrap();
        let (a, b) = (PathBuf::from("/m/a.flac"), PathBuf::from("/m/b.flac"));
        let id = {
            let app = open(state.path());
            let id = app.create_playlist("Mix", vec![a.clone()]).unwrap();
            app.add_to_playlist(id, &[b.clone(), a.clone()], None).unwrap();
            app.move_in_playlist(id, 2, 0).unwrap();
            app.rename_playlist(id, "Mix 2").unwrap();
            assert_eq!(app.remove_from_playlist(id, &[1]).unwrap(), 1);
            assert!(app.rename_playlist(999, "x").is_err());
            id
        }; // no flush needed: playlists save on every change
        let app = open(state.path());
        let p = app.playlist(id).unwrap();
        assert_eq!((p.name.as_str(), p.tracks), ("Mix 2", vec![a.clone(), b]));
        app.delete_playlist(id).unwrap();
        assert!(open(state.path()).playlists().is_empty());
    }

    #[test]
    fn export_includes_library_info_and_import_reads_it_back() {
        let state = tempfile::tempdir().unwrap();
        let m = music();
        let app = open(state.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        let id = app.create_playlist("Out", vec![m.path().join("a.flac"), m.path().join("b.mp3")]).unwrap();
        let dest = m.path().join("out.m3u8");
        app.export_m3u(id, &dest).unwrap();
        let text = fs::read_to_string(&dest).unwrap();
        assert!(text.contains("#EXTINF:1,The Testers - Tagged Song"), "{text}");
        assert!(text.contains("\na.flac\n") && text.contains("\nb.mp3\n"), "relative to the playlist file: {text}");
        let report = app.import_m3u(&dest).unwrap();
        assert_eq!((report.total, report.missing_count), (2, 0));
        let new_id = report.id;
        assert_eq!(app.playlist(new_id).unwrap().tracks, app.playlist(id).unwrap().tracks);
    }

    #[test]
    fn playing_a_playlist_fills_the_queue() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        let id = app.create_playlist("P", vec![fixture("tagged.flac"), fixture("tagged.opus")]).unwrap();
        app.play_playlist(id, 1).unwrap();
        // The queue view updates once the control thread has handled the message.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        while app.player.queue().entries.len() != 2 {
            assert!(std::time::Instant::now() < deadline, "queue never filled");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        assert_eq!(app.player.queue().entries[1].path, fixture("tagged.opus"));
        let empty = app.create_playlist("E", vec![]).unwrap();
        let gone = app.create_playlist("G", vec![PathBuf::from("/definitely/not/here.opus")]).unwrap();
        assert!(app.play_playlist(gone, 0).unwrap_err().to_string().contains("could be found"));
        assert!(app.play_playlist(empty, 0).is_err());
        assert!(app.play_playlist(12345, 0).is_err());
        app.player.stop();
    }

    #[test]
    fn saved_eq_presets_round_trip_and_reject_bad_names() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        app.set_eq_gains_db([1.0, 2.0, 3.0, 4.0, 5.0, 0.0, 0.0, 0.0, 0.0, -2.0]);
        app.set_eq_preamp_db(-3.0);
        assert_eq!(app.save_eq_preset("  My   mix ").unwrap(), "My mix");
        assert!(app.save_eq_preset("rock").is_err(), "built-in names are taken");
        assert!(app.save_eq_preset("   ").is_err());
        app.apply_eq_preset("Flat");
        app.set_eq_preamp_db(0.0);
        assert!(app.apply_eq_preset("my MIX"));
        assert_eq!(app.settings().eq.gains_db[4], 5.0);
        assert_eq!(app.settings().eq.preamp_db, -3.0);
        app.flush().unwrap();
        assert_eq!(open(state.path()).settings().custom_eq_presets.len(), 1);
        assert!(app.delete_eq_preset("My Mix"));
        assert!(!app.delete_eq_preset("My Mix"));
        assert!(!app.apply_eq_preset("My Mix"));
    }

    #[test]
    fn built_in_presets_are_within_range_and_uniquely_named() {
        let mut names = std::collections::HashSet::new();
        for (n, g) in crate::dsp::eq::PRESETS {
            assert!(names.insert(n.to_lowercase()), "duplicate {n}");
            assert!(g.iter().all(|x| x.abs() <= crate::dsp::eq::MAX_GAIN_DB), "{n}");
        }
        assert!(names.len() >= 20);
    }

    #[test]
    fn tray_settings_are_remembered() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        assert_eq!(app.settings().tray, TraySettings::default());
        app.set_tray(TraySettings { minimize_to_tray: true, close_to_tray: false });
        app.flush().unwrap();
        assert!(open(state.path()).settings().tray.minimize_to_tray);
    }

    #[test]
    fn favourites_persist_and_only_library_tracks_are_listed() {
        let state = tempfile::tempdir().unwrap();
        let m = music();
        let app = open(state.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        let (a, b) = (m.path().join("a.flac"), m.path().join("b.mp3"));
        assert_eq!(app.set_favourite(&[a.clone(), b.clone(), PathBuf::from("/gone.flac")], true), 3);
        assert_eq!(app.set_favourite(&[a.clone()], true), 0);
        assert_eq!(app.favourites().len(), 2);
        assert_eq!(app.favourite_paths().len(), 3);
        assert_eq!(app.set_favourite(&[b], false), 1);
        app.flush().unwrap();
        let again = open(state.path());
        assert!(again.favourite_paths().contains(&a));
        assert_eq!(again.favourite_paths().len(), 2);
    }

    #[test]
    fn playing_a_track_records_a_start_and_then_a_play() {
        let state = tempfile::tempdir().unwrap();
        // Real-time speed, because plays are counted in wall-clock time.
        let slow = PlayerConfig { device: None, offline: Some(OfflineConfig { sample_rate: 48_000, sink: Arc::new(Mutex::new(Vec::new())), speed: 1.0 }) };
        let app = App::new(state.path(), slow).unwrap().0;
        let f = fixture("chirp_44k_stereo.flac");
        app.player.set_queue(vec![f.clone()], 0, true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.poll_playback();
            if app.play_stats().iter().any(|s| s.path == f && s.last_played > 0) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "start never recorded");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        // The fixture is a few seconds long, so half of that counts as a play.
        loop {
            app.poll_playback();
            if app.play_stats().iter().any(|s| s.path == f && s.plays == 1) {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "play never counted");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        app.player.stop();
        app.flush().unwrap();
        assert_eq!(open(state.path()).play_stats()[0].plays, 1);
        app.reset_play_stats(None);
        assert!(app.play_stats().is_empty());
    }

    #[test]
    fn listening_is_logged_summed_and_can_be_switched_off_or_cleared() {
        let state = tempfile::tempdir().unwrap();
        let slow = PlayerConfig { device: None, offline: Some(OfflineConfig { sample_rate: 48_000, sink: Arc::new(Mutex::new(Vec::new())), speed: 1.0 }) };
        let app = App::new(state.path(), slow).unwrap().0;
        let f = fixture("chirp_44k_stereo.flac");
        app.player.set_queue(vec![f.clone()], 0, true);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            app.poll_playback();
            if app.listening_totals(&[(0, u64::MAX)])[0].plays == 1 {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "listening never logged");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let all = app.listening_totals(&[(0, u64::MAX), (0, 1)]);
        assert!(all[0].secs > 0.5, "{:?}", all[0]);
        assert_eq!(all[1].secs, 0.0, "nothing before 1970");
        // It reaches the disk when forced, and comes back after a restart.
        app.save_history(true).unwrap();
        assert!(state.path().join(HISTORY_FILE).exists());
        let again = open(state.path());
        assert_eq!(again.listening_totals(&[(0, u64::MAX)])[0].plays, 1);
        // Off: no more is logged.
        app.set_record_history(false);
        let before = app.listening_totals(&[(0, u64::MAX)])[0].secs;
        for _ in 0..15 {
            app.poll_playback();
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert_eq!(app.listening_totals(&[(0, u64::MAX)])[0].secs, before);
        app.player.stop();
        // Clearing leaves play counts alone.
        app.clear_history().unwrap();
        assert_eq!(app.listening_totals(&[(0, u64::MAX)])[0], Heard::default());
        assert!(!app.play_stats().is_empty());
        assert_eq!(open(state.path()).listening_totals(&[(0, u64::MAX)])[0], Heard::default());
    }

    #[test]
    fn mixes_and_reports_work_from_the_library() {
        let state = tempfile::tempdir().unwrap();
        let m = music();
        let app = open(state.path());
        app.add_library_folder(m.path()).unwrap();
        app.scan_library(&mut |_| {}).unwrap();
        // No history yet: nothing to report, and no mixes.
        let r = app.listening_report(0, u64::MAX, 0);
        assert_eq!(r.plays, 0);
        assert!(r.top_tracks.is_empty());
        assert!(app.mixes().is_empty());
    }

    #[test]
    fn the_share_needed_for_a_play_is_a_setting() {
        let state = tempfile::tempdir().unwrap();
        let app = open(state.path());
        assert_eq!(app.settings().count_percent, 50, "half is the default");
        app.set_count_percent(0);
        assert_eq!(app.settings().count_percent, 1, "held in range");
        app.set_count_percent(250);
        assert_eq!(app.settings().count_percent, 100);
        app.set_count_percent(80);
        drop(app);
        assert_eq!(open(state.path()).settings().count_percent, 80, "kept across a restart");
    }
}
