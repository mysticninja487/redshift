//! The player: owns the queue and the output, and runs the control thread that connects them.
//!
//! The audio thread (mixer) only plays what it's given. This thread decides *what* to give it:
//! it opens tracks, keeps the next one preloaded so transitions are gapless or crossfaded, reacts
//! to the mixer's events (a track started, the queue ran dry) and to UI requests, and publishes a
//! snapshot of the queue for the UI to read.
//!
//! Everything the UI calls on [`Player`] is cheap and non-blocking: either an atomic write
//! (volume, EQ) or a message to this thread.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{after, bounded, select, unbounded, Receiver, Sender};
use parking_lot::{Mutex, RwLock};
use serde::{Deserialize, Serialize};

use super::mixer::{Command, Event, Mixer, MixerPorts, Shared, Track};
use super::output::{start_output, OutputHandle, OutputKind};
use super::source::{open_source, wait_ready, SourceInfo};
use crate::dsp::eq::EqParams;
use crate::dsp::spectrum::{Analyzer, SpectrumConfig, VizFrame, VizTap};
use crate::error::Result;
use crate::meta::{self, TrackMeta};
use crate::queue::{EntryId, Queue, QueueEntry, RepeatMode};

/// Largest ReplayGain pre-amp either way, in dB.
pub const MAX_PREAMP_DB: f32 = 12.0;

/// A track must have this much audio buffered before it is handed to the mixer.
const PRELOAD_SECS: f64 = 0.25;
/// Pressing "previous" more than this far into a track restarts it instead of going back.
const RESTART_THRESHOLD_SECS: f64 = 3.0;

#[derive(Clone)]
pub struct OfflineConfig {
    pub sample_rate: u32,
    pub sink: Arc<Mutex<Vec<f32>>>,
    /// How many times faster than real time to run.
    pub speed: f32,
}

#[derive(Clone, Default)]
pub struct PlayerConfig {
    /// Output device id; `None` is the system default.
    pub device: Option<String>,
    /// Set to run without audio hardware (tests).
    pub offline: Option<OfflineConfig>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Stopped,
    Playing,
    Paused,
}

/// Which ReplayGain tag, if any, evens out loudness between tracks.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReplayGainMode {
    /// Play files as they are.
    #[default]
    Off,
    /// Even out individual tracks (shuffle, mixed playlists).
    Track,
    /// Keep an album's own dynamics between its tracks.
    Album,
}

impl ReplayGainMode {
    fn to_u8(self) -> u8 {
        self as u8
    }
    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Track,
            2 => Self::Album,
            _ => Self::Off,
        }
    }
}

/// The ReplayGain settings, readable from any thread.
struct ReplayGainSettings {
    mode: AtomicU8,
    preamp_db: AtomicU32,
}

impl ReplayGainSettings {
    fn new() -> Self {
        Self { mode: AtomicU8::new(ReplayGainMode::Off.to_u8()), preamp_db: AtomicU32::new(0f32.to_bits()) }
    }
    fn get(&self) -> (ReplayGainMode, f32) {
        (ReplayGainMode::from_u8(self.mode.load(Ordering::Relaxed)), f32::from_bits(self.preamp_db.load(Ordering::Relaxed)))
    }
    fn set(&self, mode: ReplayGainMode, preamp_db: f32) {
        self.mode.store(mode.to_u8(), Ordering::Relaxed);
        self.preamp_db.store(preamp_db.to_bits(), Ordering::Relaxed);
    }
}

/// The linear gain to play a track at. The tag for the chosen mode wins, falling back to the other
/// one; with neither the track plays unchanged (the pre-amp is for tagged tracks only, so untagged
/// files don't jump in level relative to them). The track's own peak limits the result, so
/// normalising up can't clip it.
pub fn resolve_gain(mode: ReplayGainMode, preamp_db: f32, m: &TrackMeta) -> f32 {
    let db = match mode {
        ReplayGainMode::Off => return 1.0,
        ReplayGainMode::Track => m.replaygain_track_db.or(m.replaygain_album_db),
        ReplayGainMode::Album => m.replaygain_album_db.or(m.replaygain_track_db),
    };
    match db {
        Some(db) => meta::replaygain_linear(db + preamp_db, m.replaygain_track_peak),
        None => 1.0,
    }
}

/// What the UI is told about, as it happens.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum PlayerEvent {
    /// A different entry is now playing (or nothing is, if `entry_id` is `None`).
    TrackChanged { entry_id: Option<EntryId> },
    /// The queue's contents, order, shuffle or repeat mode changed.
    QueueChanged,
    /// Playback reached the end of the queue.
    Ended,
    Error { message: String },
    /// The audio device (re)opened.
    DeviceChanged { name: String, sample_rate: u32 },
}

#[derive(Debug, Clone, Serialize)]
pub struct CurrentTrack {
    pub entry_id: EntryId,
    pub path: PathBuf,
    pub codec: &'static str,
    pub source_rate: u32,
    pub source_channels: usize,
    pub bits_per_sample: Option<u32>,
    pub duration_secs: Option<f64>,
}

/// A copy of the queue state, for the UI. `entries` are in the order they will play (the list order
/// when shuffle is off), so what the UI shows is what is coming.
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueView {
    pub entries: Vec<QueueEntry>,
    pub current_entry: Option<EntryId>,
    pub current: Option<CurrentTrack>,
    pub shuffle: bool,
    pub repeat: RepeatMode,
}

#[derive(Debug, Clone, Serialize)]
pub struct PlayerState {
    pub status: Status,
    pub position_secs: f64,
    pub volume: f32,
    pub muted: bool,
    pub crossfade_secs: f32,
    pub replaygain: ReplayGainMode,
    pub replaygain_preamp_db: f32,
    pub device_name: String,
    pub sample_rate: u32,
}

enum Msg {
    SetQueue { paths: Vec<PathBuf>, start: usize, autoplay: bool },
    /// Puts a remembered queue back: the current track loaded and paused at `secs`.
    Restore { paths: Vec<PathBuf>, start: usize, secs: f64 },
    Append(Vec<PathBuf>),
    InsertNext(Vec<PathBuf>),
    Remove(EntryId),
    Move { from: usize, to: usize },
    ClearQueue,
    PlayEntry(EntryId),
    Next,
    Previous,
    Pause,
    Resume,
    Stop,
    Seek(f64),
    SetShuffle(bool),
    SetRepeat(RepeatMode),
    SetDevice(Option<String>),
    /// The ReplayGain settings changed: reload what's loaded so the new gain applies now.
    ReplayGainChanged,
    Shutdown,
}

/// The handle the application holds. Cheap to share by reference.
pub struct Player {
    tx: Sender<Msg>,
    shared: Arc<Shared>,
    eq: Arc<EqParams>,
    analyzer: Arc<Mutex<Analyzer>>,
    view: Arc<RwLock<QueueView>>,
    sample_rate: Arc<AtomicU32>,
    device_name: Arc<RwLock<String>>,
    crossfade_secs: Arc<AtomicU32>,
    replaygain: Arc<ReplayGainSettings>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl Player {
    /// Opens the output and starts the control thread. Events arrive on the returned receiver.
    pub fn new(config: PlayerConfig) -> Result<(Player, Receiver<PlayerEvent>)> {
        let shared = Shared::new();
        let eq = EqParams::new();
        let tap = VizTap::new();
        let (events_tx, events_rx) = unbounded();
        let (tx, msg_rx) = unbounded();
        let (err_tx, err_rx) = unbounded();

        let wiring = open_output(&config, &shared, &eq, &tap)?;
        let sample_rate = Arc::new(AtomicU32::new(wiring.out.sample_rate));
        let device_name = Arc::new(RwLock::new(wiring.out.device_name.clone()));
        let analyzer = Arc::new(Mutex::new(Analyzer::new(tap.clone(), wiring.out.sample_rate, SpectrumConfig::default())));
        let view = Arc::new(RwLock::new(QueueView::default()));
        let crossfade_secs = Arc::new(AtomicU32::new(0f32.to_bits()));
        let replaygain = Arc::new(ReplayGainSettings::new());
        let _ = events_tx.send(PlayerEvent::DeviceChanged { name: wiring.out.device_name.clone(), sample_rate: wiring.out.sample_rate });

        let core = Core {
            config,
            queue: Queue::new(),
            shared: shared.clone(),
            eq: eq.clone(),
            tap,
            analyzer: analyzer.clone(),
            view: view.clone(),
            sample_rate: sample_rate.clone(),
            device_name: device_name.clone(),
            crossfade_secs: crossfade_secs.clone(),
            replaygain: replaygain.clone(),
            out: wiring.out,
            cmd: wiring.cmd,
            mixer_events: wiring.events,
            retired: wiring.retired,
            msg_rx,
            decode_errors_tx: err_tx,
            decode_errors_rx: err_rx,
            events_tx,
            loads: HashMap::new(),
            infos: HashMap::new(),
            next_load_id: 1,
            current_load: None,
            next_load: None,
            failed_next: None,
            last_device_recovery: None,
        };
        let thread = thread::Builder::new()
            .name("redshift-control".into())
            .spawn(move || core.run())
            .map_err(crate::error::Error::Io)?;

        let player = Player {
            tx,
            shared,
            eq,
            analyzer,
            view,
            sample_rate,
            device_name,
            crossfade_secs,
            replaygain,
            thread: Mutex::new(Some(thread)),
        };
        Ok((player, events_rx))
    }

    fn send(&self, m: Msg) {
        let _ = self.tx.send(m);
    }

    // ---- transport ----

    /// Replaces the queue and (optionally) starts playing `paths[start]`.
    pub fn set_queue(&self, paths: Vec<PathBuf>, start: usize, autoplay: bool) {
        self.send(Msg::SetQueue { paths, start, autoplay });
    }
    /// Brings back a queue from an earlier run: `paths[start]` is loaded, paused, at `secs`.
    pub fn restore_queue(&self, paths: Vec<PathBuf>, start: usize, secs: f64) {
        self.send(Msg::Restore { paths, start, secs });
    }
    /// Cheap fingerprint of the queue (its length, order and current entry) for noticing changes
    /// without copying it.
    pub fn queue_stamp(&self) -> u64 {
        let v = self.view.read();
        let mut h = (v.entries.len() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ v.current_entry.unwrap_or(0);
        for (i, e) in v.entries.iter().enumerate() {
            h = h.rotate_left(5) ^ e.id.wrapping_mul(i as u64 + 1);
        }
        h
    }
    /// The paths in play order and the index of the current one, for saving the queue.
    pub fn queue_paths(&self) -> (Vec<PathBuf>, Option<usize>) {
        let v = self.view.read();
        let cur = v.current_entry.and_then(|id| v.entries.iter().position(|e| e.id == id));
        (v.entries.iter().map(|e| e.path.clone()).collect(), cur)
    }
    pub fn play_entry(&self, id: EntryId) {
        self.send(Msg::PlayEntry(id));
    }
    pub fn pause(&self) {
        self.send(Msg::Pause);
    }
    pub fn resume(&self) {
        self.send(Msg::Resume);
    }
    pub fn toggle_pause(&self) {
        match self.status() {
            Status::Playing => self.pause(),
            _ => self.resume(),
        }
    }
    pub fn stop(&self) {
        self.send(Msg::Stop);
    }
    pub fn next(&self) {
        self.send(Msg::Next);
    }
    pub fn previous(&self) {
        self.send(Msg::Previous);
    }
    pub fn seek(&self, secs: f64) {
        self.send(Msg::Seek(secs));
    }

    // ---- queue ----

    pub fn append(&self, paths: Vec<PathBuf>) {
        self.send(Msg::Append(paths));
    }
    pub fn play_next(&self, paths: Vec<PathBuf>) {
        self.send(Msg::InsertNext(paths));
    }
    pub fn remove(&self, id: EntryId) {
        self.send(Msg::Remove(id));
    }
    pub fn move_entry(&self, from: usize, to: usize) {
        self.send(Msg::Move { from, to });
    }
    pub fn clear_queue(&self) {
        self.send(Msg::ClearQueue);
    }
    pub fn set_shuffle(&self, on: bool) {
        self.send(Msg::SetShuffle(on));
    }
    pub fn set_repeat(&self, mode: RepeatMode) {
        self.send(Msg::SetRepeat(mode));
    }
    pub fn set_device(&self, id: Option<String>) {
        self.send(Msg::SetDevice(id));
    }

    // ---- direct controls (no round trip) ----

    /// Volume on a 0..1 slider, applied as amplitude squared so the slider feels even.
    pub fn set_volume(&self, v: f32) {
        let v = v.clamp(0.0, 1.0);
        self.shared.set_volume(v * v);
    }
    pub fn volume(&self) -> f32 {
        self.shared.volume().sqrt()
    }
    pub fn set_muted(&self, m: bool) {
        self.shared.set_muted(m);
    }
    pub fn eq(&self) -> &Arc<EqParams> {
        &self.eq
    }
    /// Crossfade length in seconds; 0 turns it off (tracks still join gaplessly).
    pub fn set_crossfade_secs(&self, secs: f32) {
        let secs = secs.clamp(0.0, 12.0);
        self.crossfade_secs.store(secs.to_bits(), Ordering::Relaxed);
        self.shared.set_crossfade_frames((secs * self.sample_rate.load(Ordering::Relaxed) as f32) as u32);
    }
    pub fn crossfade_secs(&self) -> f32 {
        f32::from_bits(self.crossfade_secs.load(Ordering::Relaxed))
    }
    /// Loudness normalisation from ReplayGain tags. `preamp_db` (clamped to ±12) is added to the
    /// tagged gain. Takes effect immediately, including on what's playing and the preloaded next
    /// track, with a very short fade.
    pub fn set_replaygain(&self, mode: ReplayGainMode, preamp_db: f32) {
        let preamp_db = if preamp_db.is_finite() { preamp_db.clamp(-MAX_PREAMP_DB, MAX_PREAMP_DB) } else { 0.0 };
        if self.replaygain.get() != (mode, preamp_db) {
            self.replaygain.set(mode, preamp_db);
            self.send(Msg::ReplayGainChanged);
        }
    }
    pub fn replaygain(&self) -> (ReplayGainMode, f32) {
        self.replaygain.get()
    }

    // ---- reading state ----

    pub fn status(&self) -> Status {
        if self.shared.current_id() == 0 {
            Status::Stopped
        } else if self.shared.playing() {
            Status::Playing
        } else {
            Status::Paused
        }
    }

    pub fn position_secs(&self) -> f64 {
        self.shared.position_frames() as f64 / self.sample_rate.load(Ordering::Relaxed) as f64
    }

    pub fn state(&self) -> PlayerState {
        PlayerState {
            status: self.status(),
            position_secs: self.position_secs(),
            volume: self.volume(),
            muted: self.shared.muted(),
            crossfade_secs: self.crossfade_secs(),
            replaygain: self.replaygain().0,
            replaygain_preamp_db: self.replaygain().1,
            device_name: self.device_name.read().clone(),
            sample_rate: self.sample_rate.load(Ordering::Relaxed),
        }
    }

    pub fn queue(&self) -> QueueView {
        self.view.read().clone()
    }

    /// The playing (or paused) track, without copying the whole queue.
    pub fn current_track(&self) -> Option<CurrentTrack> {
        self.view.read().current.clone()
    }

    /// Fresh visualiser data. Call at the display's frame rate.
    pub fn viz(&self) -> VizFrame {
        self.analyzer.lock().snapshot()
    }
    pub fn configure_viz(&self, cfg: SpectrumConfig) {
        self.analyzer.lock().configure(cfg);
    }
    pub fn viz_config(&self) -> SpectrumConfig {
        self.analyzer.lock().config()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(t) = self.thread.lock().take() {
            let _ = t.join();
        }
    }
}

// ------------------------------------------------------------------------------------------------

struct Wiring {
    out: OutputHandle,
    cmd: Sender<Command>,
    events: Receiver<Event>,
    retired: Receiver<Track>,
}

fn open_output(config: &PlayerConfig, shared: &Arc<Shared>, eq: &Arc<EqParams>, tap: &VizTap) -> Result<Wiring> {
    let (cmd, commands) = bounded(256);
    let (events_tx, events) = bounded(256);
    let (retired_tx, retired) = bounded(128);
    let kind = match &config.offline {
        Some(o) => OutputKind::Offline { sample_rate: o.sample_rate, sink: o.sink.clone(), speed: o.speed },
        None => OutputKind::Device(config.device.clone()),
    };
    let (shared, eq, tap) = (shared.clone(), eq.clone(), tap.clone());
    let out = start_output(kind, move |rate| {
        Mixer::new(rate, shared, eq, tap, MixerPorts { commands, events: events_tx, retired: retired_tx })
    })?;
    Ok(Wiring { out, cmd, events, retired })
}

struct Core {
    config: PlayerConfig,
    queue: Queue,
    shared: Arc<Shared>,
    eq: Arc<EqParams>,
    tap: VizTap,
    analyzer: Arc<Mutex<Analyzer>>,
    view: Arc<RwLock<QueueView>>,
    sample_rate: Arc<AtomicU32>,
    device_name: Arc<RwLock<String>>,
    crossfade_secs: Arc<AtomicU32>,
    replaygain: Arc<ReplayGainSettings>,

    out: OutputHandle,
    cmd: Sender<Command>,
    mixer_events: Receiver<Event>,
    retired: Receiver<Track>,
    msg_rx: Receiver<Msg>,
    decode_errors_tx: Sender<(u64, String)>,
    decode_errors_rx: Receiver<(u64, String)>,
    events_tx: Sender<PlayerEvent>,

    /// Which queue entry each loaded track (by load id) came from.
    loads: HashMap<u64, EntryId>,
    infos: HashMap<u64, SourceInfo>,
    next_load_id: u64,
    /// Load id of what the mixer is playing.
    current_load: Option<u64>,
    /// The preloaded follow-up, as (load id, entry).
    next_load: Option<(u64, EntryId)>,
    /// An entry that failed to preload, so we don't retry it on every event.
    failed_next: Option<EntryId>,
    last_device_recovery: Option<Instant>,
}

impl Core {
    fn run(mut self) {
        loop {
            let (msg_rx, ev_rx, err_rx) = (self.msg_rx.clone(), self.mixer_events.clone(), self.decode_errors_rx.clone());
            select! {
                recv(msg_rx) -> m => match m {
                    Ok(Msg::Shutdown) | Err(_) => break,
                    Ok(m) => self.handle(m),
                },
                recv(ev_rx) -> e => if let Ok(e) = e { self.on_mixer_event(e) },
                recv(err_rx) -> e => if let Ok((load_id, message)) = e { self.on_decode_error(load_id, message) },
                recv(after(Duration::from_millis(100))) -> _ => self.on_tick(),
            }
            self.collect_retired();
        }
        let _ = self.cmd.try_send(Command::Stop);
    }

    fn emit(&self, e: PlayerEvent) {
        let _ = self.events_tx.send(e);
    }

    fn send(&self, c: Command) {
        if self.cmd.try_send(c).is_err() {
            self.emit(PlayerEvent::Error { message: "audio engine is not accepting commands".into() });
        }
    }

    fn rate(&self) -> u32 {
        self.out.sample_rate
    }

    fn is_playing(&self) -> bool {
        self.shared.playing()
    }

    // ---- loading ----

    /// The gain to play `path` at under the current ReplayGain settings. Reads the file's tags
    /// only when normalising is on; a file whose tags can't be read just plays unchanged.
    fn gain_for(&self, path: &Path) -> f32 {
        let (mode, preamp_db) = self.replaygain.get();
        if mode == ReplayGainMode::Off {
            return 1.0;
        }
        meta::read_meta(path).map_or(1.0, |m| resolve_gain(mode, preamp_db, &m))
    }

    fn load(&mut self, entry: &QueueEntry, start_secs: f64) -> Result<Track> {
        let load_id = self.next_load_id;
        self.next_load_id += 1;
        let rate = self.rate();
        let (source, info) = open_source(&entry.path, rate, start_secs, load_id, self.decode_errors_tx.clone())?;
        wait_ready(&source, (rate as f64 * PRELOAD_SECS) as usize, Duration::from_secs(2));
        self.loads.insert(load_id, entry.id);
        let track = Track {
            id: load_id,
            source: Box::new(source),
            total_frames: info.total_frames,
            start_frame: info.start_frame,
            gain: self.gain_for(&entry.path),
        };
        self.infos.insert(load_id, info);
        Ok(track)
    }

    /// Starts the queue's current entry. With `skip_on_error`, unplayable entries are reported and
    /// passed over. Returns whether something is now playing.
    fn start_current(&mut self, mut start_secs: f64, fade_ms: u32, autoplay: bool, skip_on_error: bool, announce: bool) -> bool {
        let fade_frames = (self.rate() as u64 * fade_ms as u64 / 1000) as u32;
        let mut attempts = 0;
        loop {
            let Some(entry) = self.queue.current().cloned() else {
                self.halt();
                return false;
            };
            match self.load(&entry, start_secs) {
                Ok(track) => {
                    self.current_load = Some(track.id);
                    self.failed_next = None;
                    self.send(Command::Start { track, fade_frames });
                    if autoplay {
                        self.send(Command::Play);
                    }
                    // The mixer keeps its preloaded next track across a Start; this only swaps it
                    // if the queue now says something else should follow.
                    self.refresh_next();
                    self.publish();
                    if announce {
                        self.emit(PlayerEvent::TrackChanged { entry_id: Some(entry.id) });
                    }
                    return true;
                }
                Err(e) => {
                    self.emit(PlayerEvent::Error { message: format!("{}: {e}", file_name(&entry.path)) });
                    attempts += 1;
                    if !skip_on_error || attempts >= self.queue.len() || self.queue.advance().is_none() {
                        self.halt();
                        return false;
                    }
                    start_secs = 0.0;
                }
            }
        }
    }

    /// Stops playback; the queue keeps its place.
    fn halt(&mut self) {
        self.send(Command::Stop);
        self.current_load = None;
        self.next_load = None;
        self.publish();
        self.emit(PlayerEvent::TrackChanged { entry_id: None });
    }

    /// Makes sure the mixer's "next" slot holds whatever the queue says plays after the current
    /// entry, loading it if needed.
    fn refresh_next(&mut self) {
        let desired = self.queue.peek_next().cloned();
        match (&desired, &self.next_load) {
            (None, None) => return,
            (Some(d), Some((_, loaded))) if d.id == *loaded => return,
            (Some(d), _) if self.failed_next == Some(d.id) => return,
            _ => {}
        }
        match desired {
            None => {
                self.send(Command::SetNext(None));
                self.next_load = None;
            }
            Some(entry) => match self.load(&entry, 0.0) {
                Ok(track) => {
                    self.next_load = Some((track.id, entry.id));
                    self.send(Command::SetNext(Some(track)));
                }
                Err(e) => {
                    self.emit(PlayerEvent::Error { message: format!("{}: {e}", file_name(&entry.path)) });
                    self.failed_next = Some(entry.id);
                    self.send(Command::SetNext(None));
                    self.next_load = None;
                }
            },
        }
    }

    fn publish(&mut self) {
        let current = self.current_load.and_then(|id| {
            let entry = self.queue.current()?;
            let info = self.infos.get(&id)?;
            let rate = self.rate() as f64;
            Some(CurrentTrack {
                entry_id: entry.id,
                path: entry.path.clone(),
                codec: info.codec,
                source_rate: info.source_rate,
                source_channels: info.source_channels,
                bits_per_sample: info.bits_per_sample,
                duration_secs: info.total_frames.map(|f| f as f64 / rate),
            })
        });
        *self.view.write() = QueueView {
            entries: self.queue.play_order().cloned().collect(),
            current_entry: self.queue.current().map(|e| e.id),
            current,
            shuffle: self.queue.shuffle(),
            repeat: self.queue.repeat(),
        };
    }

    fn queue_changed(&mut self) {
        self.failed_next = None;
        self.refresh_next();
        self.publish();
        self.emit(PlayerEvent::QueueChanged);
    }

    // ---- requests from the UI ----

    fn handle(&mut self, msg: Msg) {
        match msg {
            Msg::Shutdown => {}
            Msg::SetQueue { paths, start, autoplay } => {
                self.queue.set(paths, start);
                self.emit(PlayerEvent::QueueChanged);
                if autoplay {
                    self.start_current(0.0, 10, true, true, true);
                } else {
                    self.halt();
                }
            }
            Msg::Restore { paths, start, secs } => {
                self.queue.set(paths, start);
                self.emit(PlayerEvent::QueueChanged);
                // Loaded but not playing; an unplayable file is reported and the next one is tried.
                self.start_current(secs.max(0.0), 10, false, true, true);
            }
            Msg::Append(paths) => {
                self.queue.append(paths);
                self.queue_changed();
            }
            Msg::InsertNext(paths) => {
                self.queue.insert_next(paths);
                self.queue_changed();
            }
            Msg::Move { from, to } => {
                self.queue.move_in_play_order(from, to);
                self.queue_changed();
            }
            Msg::ClearQueue => {
                self.queue.clear();
                self.halt();
                self.emit(PlayerEvent::QueueChanged);
            }
            Msg::Remove(id) => {
                let was_current = self.queue.current().is_some_and(|e| e.id == id);
                let playing = self.is_playing();
                self.queue.remove(id);
                if was_current {
                    if self.queue.current().is_some() && playing {
                        self.start_current(0.0, 10, true, true, true);
                    } else {
                        self.halt();
                    }
                    self.emit(PlayerEvent::QueueChanged);
                } else {
                    self.queue_changed();
                }
            }
            Msg::PlayEntry(id) => {
                if self.queue.jump_to(id) {
                    self.start_current(0.0, 10, true, true, true);
                }
            }
            Msg::Next => {
                if self.queue.skip_next().is_some() {
                    self.start_current(0.0, 10, true, true, true);
                }
            }
            Msg::Previous => {
                let pos = self.shared.position_frames() as f64 / self.rate() as f64;
                let at_very_start = self.queue.current_index() == Some(0) && self.queue.repeat() != RepeatMode::All;
                if pos > RESTART_THRESHOLD_SECS || at_very_start {
                    self.start_current(0.0, 10, self.is_playing(), true, true);
                } else if self.queue.previous().is_some() {
                    self.start_current(0.0, 10, true, true, true);
                }
            }
            Msg::Pause => self.send(Command::Pause),
            Msg::Resume => {
                if self.shared.current_id() == 0 && self.queue.current().is_some() {
                    // Stopped (the queue ended, or was never started): play the current entry.
                    self.start_current(0.0, 0, true, true, true);
                } else {
                    self.send(Command::Play);
                }
            }
            Msg::Stop => self.halt(),
            Msg::Seek(secs) => {
                if self.current_load.is_none() {
                    return;
                }
                let mut secs = secs.max(0.0);
                if let Some(d) = self.current_duration() {
                    secs = secs.min((d - 0.05).max(0.0));
                }
                // Reopen at the target and cross over to it with a very short fade. Keeps the
                // paused/playing state as it is.
                let playing = self.is_playing();
                self.start_current(secs, 10, false, false, false);
                if playing {
                    self.send(Command::Play);
                }
            }
            Msg::SetShuffle(on) => {
                self.queue.set_shuffle(on);
                self.queue_changed();
            }
            Msg::SetRepeat(mode) => {
                self.queue.set_repeat(mode);
                self.queue_changed();
            }
            Msg::SetDevice(id) => {
                self.config.device = id;
                self.rebuild_output();
            }
            Msg::ReplayGainChanged => {
                if self.current_load.is_none() {
                    return; // nothing loaded yet; the next load picks the new setting up
                }
                // Gain is fixed when a track is loaded, so reopen the current one where it is, the
                // way a seek does, and drop the preloaded follow-up so it loads again with the new
                // gain too.
                let pos = self.shared.position_frames() as f64 / self.rate() as f64;
                let playing = self.is_playing();
                self.next_load = None;
                self.failed_next = None;
                self.start_current(pos, 10, false, false, false);
                if playing {
                    self.send(Command::Play);
                }
            }
        }
    }

    fn current_duration(&self) -> Option<f64> {
        let info = self.infos.get(&self.current_load?)?;
        info.total_frames.map(|f| f as f64 / self.rate() as f64)
    }

    // ---- events from the audio side ----

    fn on_mixer_event(&mut self, e: Event) {
        match e {
            Event::TrackStarted { id } => {
                if Some(id) == self.current_load {
                    return; // our own Start
                }
                // Load ids only ever go up. An older one than the current is news about a track the
                // mixer has since been told to leave (a natural join that raced with the user
                // pressing next, say); acting on it would drag the queue backwards.
                if self.current_load.is_some_and(|cur| id < cur) {
                    return;
                }
                // The mixer moved on by itself: gapless join or crossfade into the preloaded track.
                let Some(&entry_id) = self.loads.get(&id) else { return };
                self.queue.jump_to(entry_id);
                self.current_load = Some(id);
                if self.next_load.is_some_and(|(n, _)| n == id) {
                    self.next_load = None;
                }
                self.publish();
                self.emit(PlayerEvent::TrackChanged { entry_id: Some(entry_id) });
            }
            Event::NeedNext => self.refresh_next(),
            Event::PlaybackEnded => {
                self.current_load = None;
                self.next_load = None;
                // Normally the next track was preloaded and we never get here with one pending.
                // If it failed to load, try to carry on from the queue.
                if self.queue.peek_next().is_some() && self.queue.advance().is_some() {
                    self.start_current(0.0, 0, true, true, true);
                } else {
                    self.publish();
                    self.emit(PlayerEvent::Ended);
                    self.emit(PlayerEvent::TrackChanged { entry_id: None });
                }
            }
        }
    }

    fn on_decode_error(&mut self, load_id: u64, message: String) {
        let name = self
            .loads
            .get(&load_id)
            .and_then(|id| self.queue.entries().iter().find(|e| e.id == *id))
            .map_or_else(|| "track".to_string(), |e| file_name(&e.path));
        self.emit(PlayerEvent::Error { message: format!("{name}: playback stopped early: {message}") });
    }

    fn on_tick(&mut self) {
        let problems: Vec<String> = self.out.errors.try_iter().collect();
        for p in problems {
            self.emit(PlayerEvent::Error { message: format!("audio device: {p}") });
            let lost = p.contains("DeviceNotAvailable");
            let cooled_down = self.last_device_recovery.is_none_or(|t| t.elapsed() > Duration::from_secs(3));
            if lost && cooled_down {
                // The device vanished (unplugged headphones, say): carry on with the system default.
                self.last_device_recovery = Some(Instant::now());
                self.config.device = None;
                self.rebuild_output();
            }
        }
    }

    /// Drops tracks the mixer has finished with, here rather than on the audio thread.
    fn collect_retired(&mut self) {
        while let Ok(track) = self.retired.try_recv() {
            if Some(track.id) != self.current_load && self.next_load.map(|n| n.0) != Some(track.id) {
                self.loads.remove(&track.id);
                self.infos.remove(&track.id);
            }
            drop(track);
        }
    }

    /// Reopens the output (new device, or recovery) and resumes where playback was.
    fn rebuild_output(&mut self) {
        let was_playing = self.is_playing();
        let position = self.shared.position_frames() as f64 / self.rate() as f64;
        let had_current = self.current_load.is_some();

        // Release the old device first: some drivers won't open a second stream alongside it.
        let _ = self.cmd.try_send(Command::Stop);
        let wiring = match self.try_open() {
            Ok(w) => w,
            Err(e) => {
                self.emit(PlayerEvent::Error { message: format!("could not open audio device: {e}") });
                return;
            }
        };
        self.out = wiring.out;
        self.cmd = wiring.cmd;
        self.mixer_events = wiring.events;
        self.retired = wiring.retired;
        self.loads.clear();
        self.infos.clear();
        self.current_load = None;
        self.next_load = None;
        self.failed_next = None;

        let rate = self.out.sample_rate;
        self.sample_rate.store(rate, Ordering::Relaxed);
        *self.device_name.write() = self.out.device_name.clone();
        let viz_config = self.analyzer.lock().config();
        *self.analyzer.lock() = Analyzer::new(self.tap.clone(), rate, viz_config);
        // Crossfade length is stored in seconds; the mixer wants frames at the new rate.
        let secs = f32::from_bits(self.crossfade_secs.load(Ordering::Relaxed));
        self.shared.set_crossfade_frames((secs * rate as f32) as u32);
        self.emit(PlayerEvent::DeviceChanged { name: self.out.device_name.clone(), sample_rate: rate });

        if had_current {
            self.start_current(position, 0, was_playing, false, false);
        }
    }

    fn try_open(&self) -> Result<Wiring> {
        open_output(&self.config, &self.shared, &self.eq, &self.tap)
    }
}

fn file_name(p: &Path) -> String {
    p.file_name().map_or_else(|| p.display().to_string(), |n| n.to_string_lossy().into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(x: f32) -> f32 {
        10f32.powf(x / 20.0)
    }

    fn tagged(track: Option<f32>, album: Option<f32>, peak: Option<f32>) -> TrackMeta {
        TrackMeta { replaygain_track_db: track, replaygain_album_db: album, replaygain_track_peak: peak, ..Default::default() }
    }

    #[test]
    fn off_never_changes_the_level() {
        assert_eq!(resolve_gain(ReplayGainMode::Off, 6.0, &tagged(Some(-6.5), Some(-5.0), None)), 1.0);
    }

    #[test]
    fn track_and_album_modes_pick_their_own_tag() {
        let m = tagged(Some(-6.5), Some(-5.25), Some(0.9));
        assert!((resolve_gain(ReplayGainMode::Track, 0.0, &m) - db(-6.5)).abs() < 1e-6);
        assert!((resolve_gain(ReplayGainMode::Album, 0.0, &m) - db(-5.25)).abs() < 1e-6);
    }

    #[test]
    fn a_missing_tag_falls_back_to_the_other_one() {
        assert!((resolve_gain(ReplayGainMode::Album, 0.0, &tagged(Some(-4.0), None, None)) - db(-4.0)).abs() < 1e-6);
        assert!((resolve_gain(ReplayGainMode::Track, 0.0, &tagged(None, Some(-3.0), None)) - db(-3.0)).abs() < 1e-6);
    }

    #[test]
    fn untagged_tracks_are_left_alone_even_with_a_preamp() {
        let m = tagged(None, None, None);
        assert_eq!(resolve_gain(ReplayGainMode::Track, 6.0, &m), 1.0);
        assert_eq!(resolve_gain(ReplayGainMode::Album, -6.0, &m), 1.0);
    }

    #[test]
    fn preamp_adds_to_the_tagged_gain() {
        let m = tagged(Some(-6.5), None, Some(0.9));
        assert!((resolve_gain(ReplayGainMode::Track, 6.0, &m) - db(-0.5)).abs() < 1e-6);
        assert!((resolve_gain(ReplayGainMode::Track, -3.0, &m) - db(-9.5)).abs() < 1e-6);
    }

    #[test]
    fn the_tracks_own_peak_stops_boosting_from_clipping() {
        let m = tagged(Some(-6.5), None, Some(0.9));
        // -6.5 + 10 = +3.5 dB would push a 0.9 peak to 1.35; it's held at exactly full scale.
        let g = resolve_gain(ReplayGainMode::Track, 10.0, &m);
        assert!((g - 1.0 / 0.9).abs() < 1e-6, "{g}");
        assert!((g * 0.9 - 1.0).abs() < 1e-6);
        // Without a peak tag there's nothing to limit by.
        let g = resolve_gain(ReplayGainMode::Track, 10.0, &tagged(Some(-6.5), None, None));
        assert!((g - db(3.5)).abs() < 1e-6);
    }

    #[test]
    fn mode_round_trips_through_its_byte_form() {
        for m in [ReplayGainMode::Off, ReplayGainMode::Track, ReplayGainMode::Album] {
            assert_eq!(ReplayGainMode::from_u8(m.to_u8()), m);
        }
        assert_eq!(ReplayGainMode::from_u8(200), ReplayGainMode::Off, "garbage means off");
    }
}
