//! The real-time heart of the player: turns a current track, a queued next track and a handful of
//! commands into buffers of stereo audio.
//!
//! Everything here runs on the audio thread, so it never blocks, allocates, or drops large objects
//! (finished tracks are sent back to the control thread instead). It knows nothing about devices or
//! files, only [`FrameSource`]s, which makes it fully testable with synthetic sources.
//!
//! Signal path: `sources -> crossfade -> EQ -> visualiser tap -> volume & pause ramp -> soft clip`.

use std::f32::consts::FRAC_PI_2;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;

use crossbeam_channel::{Receiver, Sender};

use crate::dsp::eq::{EqParams, Equalizer};
use crate::dsp::spectrum::VizTap;

/// Frames mixed per step. Crossfades and track changes are decided at this granularity (~5 ms).
pub const CHUNK: usize = 256;
/// Pausing and resuming fade over this many frames (~21 ms). A power of two, so the per-frame step
/// is exact in f32 and the ramp lands on precisely 0.0 and 1.0.
const PAUSE_RAMP_FRAMES: usize = 1024;

/// A pull-based stream of interleaved stereo f32 at the mixer's sample rate.
pub trait FrameSource: Send {
    /// Fills as much of `out` as is available right now and returns `(frames_written, ended)`.
    /// Fewer frames than requested with `ended == false` is an underrun (the producer is behind);
    /// the mixer plays silence for the gap and asks again next time.
    fn read(&mut self, out: &mut [f32]) -> (usize, bool);
}

pub struct Track {
    /// Identifies the track to the control thread. Must be non-zero.
    pub id: u64,
    pub source: Box<dyn FrameSource>,
    /// Length in frames at the mixer's rate, if known. Needed for crossfading out of this track.
    pub total_frames: Option<u64>,
    /// Where in the track the source starts (non-zero after a seek), so position reads correctly.
    pub start_frame: u64,
    /// Linear gain applied to this track (ReplayGain).
    pub gain: f32,
}

pub enum Command {
    Play,
    Pause,
    /// Drop everything and go quiet.
    Stop,
    /// Switch to `track` now. The old track fades out over `fade_frames` (0 = hard cut).
    Start { track: Track, fade_frames: u32 },
    /// Set (or clear) the track that follows the current one.
    SetNext(Option<Track>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// `id` is now the current track.
    TrackStarted { id: u64 },
    /// The queue slot after the current track is empty; send a `SetNext` if there is one to play.
    NeedNext,
    /// The current track ended and nothing followed it.
    PlaybackEnded,
}

/// State the UI can read at any time without asking the audio thread.
#[derive(Debug)]
pub struct Shared {
    volume: AtomicU32,
    muted: AtomicBool,
    crossfade_frames: AtomicU32,
    position_frames: AtomicU64,
    current_id: AtomicU64,
    playing: AtomicBool,
    underruns: AtomicU64,
}

impl Shared {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            volume: AtomicU32::new(1.0f32.to_bits()),
            muted: AtomicBool::new(false),
            crossfade_frames: AtomicU32::new(0),
            position_frames: AtomicU64::new(0),
            current_id: AtomicU64::new(0),
            playing: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
        })
    }
    pub fn volume(&self) -> f32 {
        f32::from_bits(self.volume.load(Ordering::Relaxed))
    }
    pub fn set_volume(&self, v: f32) {
        self.volume.store(v.clamp(0.0, 2.0).to_bits(), Ordering::Relaxed);
    }
    pub fn muted(&self) -> bool {
        self.muted.load(Ordering::Relaxed)
    }
    pub fn set_muted(&self, m: bool) {
        self.muted.store(m, Ordering::Relaxed);
    }
    pub fn crossfade_frames(&self) -> u32 {
        self.crossfade_frames.load(Ordering::Relaxed)
    }
    pub fn set_crossfade_frames(&self, f: u32) {
        self.crossfade_frames.store(f, Ordering::Relaxed);
    }
    /// Playback position within the current track, in frames at the mixer's rate.
    pub fn position_frames(&self) -> u64 {
        self.position_frames.load(Ordering::Relaxed)
    }
    /// Id of the current track, or 0 if there is none.
    pub fn current_id(&self) -> u64 {
        self.current_id.load(Ordering::Relaxed)
    }
    pub fn playing(&self) -> bool {
        self.playing.load(Ordering::Relaxed)
    }
    /// How many times a source has failed to deliver in time.
    pub fn underruns(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }
}

struct Active {
    track: Track,
    /// Frames read from the source so far.
    played: u64,
}

impl Active {
    fn new(track: Track) -> Self {
        Self { track, played: 0 }
    }
    fn position(&self) -> u64 {
        self.track.start_frame + self.played
    }
    fn remaining(&self) -> Option<u64> {
        self.track.total_frames.map(|t| t.saturating_sub(self.position()))
    }
}

struct Fade {
    outgoing: Active,
    done: u64,
    len: u64,
}

pub struct Mixer {
    sample_rate: u32,
    shared: Arc<Shared>,
    commands: Receiver<Command>,
    events: Sender<Event>,
    retired: Sender<Track>,

    current: Option<Active>,
    next: Option<Active>,
    fade: Option<Fade>,

    eq: Equalizer,
    tap: VizTap,

    playing: bool,
    /// 0..1, ramps towards 1 while playing and towards 0 while paused, to avoid clicks.
    play_gain: f32,
    /// The smoothed volume actually applied.
    volume: f32,

    in_buf: Vec<f32>,
    out_buf: Vec<f32>,
}

/// Everything a mixer needs from outside, and the other ends of its channels.
pub struct MixerPorts {
    pub commands: Receiver<Command>,
    pub events: Sender<Event>,
    pub retired: Sender<Track>,
}

impl Mixer {
    pub fn new(
        sample_rate: u32,
        shared: Arc<Shared>,
        eq: Arc<EqParams>,
        tap: VizTap,
        ports: MixerPorts,
    ) -> Self {
        Self {
            sample_rate,
            shared,
            commands: ports.commands,
            events: ports.events,
            retired: ports.retired,
            current: None,
            next: None,
            fade: None,
            eq: Equalizer::new(eq, sample_rate),
            tap,
            playing: false,
            play_gain: 0.0,
            volume: 1.0,
            in_buf: vec![0.0; CHUNK * 2],
            out_buf: vec![0.0; CHUNK * 2],
        }
    }

    /// Fills `out` (interleaved stereo) with the next stretch of audio.
    ///
    /// The whole signal path runs per chunk, not per callback: pausing is a ramp that must end on a
    /// chunk boundary, and a source must never be read for audio that is then thrown away.
    pub fn render(&mut self, out: &mut [f32]) {
        self.drain_commands();
        for chunk in out.chunks_mut(CHUNK * 2) {
            self.mix_chunk(chunk);
            self.eq.process(chunk);
            self.tap.write(chunk);
            self.apply_output_stage(chunk);
        }
        self.shared.position_frames.store(self.current.as_ref().map_or(0, Active::position), Ordering::Relaxed);
        self.shared.current_id.store(self.current.as_ref().map_or(0, |a| a.track.id), Ordering::Relaxed);
    }

    fn drain_commands(&mut self) {
        while let Ok(cmd) = self.commands.try_recv() {
            match cmd {
                Command::Play => {
                    self.playing = true;
                    self.shared.playing.store(true, Ordering::Relaxed);
                }
                Command::Pause => {
                    self.playing = false;
                    self.shared.playing.store(false, Ordering::Relaxed);
                }
                Command::Stop => {
                    self.playing = false;
                    self.shared.playing.store(false, Ordering::Relaxed);
                    let (cur, next) = (self.current.take(), self.next.take());
                    self.retire_active(cur);
                    self.retire_active(next);
                    if let Some(f) = self.fade.take() {
                        self.retire_active(Some(f.outgoing));
                    }
                    self.tap.clear();
                }
                Command::Start { track, fade_frames } => {
                    if let Some(f) = self.fade.take() {
                        self.retire_active(Some(f.outgoing));
                    }
                    let old = self.current.take();
                    match old {
                        Some(old) if fade_frames > 0 && self.play_gain > 0.0 => {
                            self.fade = Some(Fade { outgoing: old, done: 0, len: fade_frames as u64 });
                        }
                        old => self.retire_active(old),
                    }
                    self.begin(Active::new(track));
                }
                Command::SetNext(track) => {
                    let old = self.next.take();
                    self.retire_active(old);
                    self.next = track.map(Active::new);
                }
            }
        }
    }

    /// Makes `a` the current track.
    fn begin(&mut self, a: Active) {
        let id = a.track.id;
        self.current = Some(a);
        let _ = self.events.try_send(Event::TrackStarted { id });
        if self.next.is_none() {
            let _ = self.events.try_send(Event::NeedNext);
        }
    }

    fn promote_next(&mut self) {
        match self.next.take() {
            Some(next) => self.begin(next),
            None => {
                let _ = self.events.try_send(Event::PlaybackEnded);
            }
        }
    }

    /// Hands a finished track back to the control thread, so its buffers are freed there rather
    /// than here. If the channel is somehow full it is dropped in place, which is merely slower.
    fn retire_active(&mut self, a: Option<Active>) {
        if let Some(a) = a {
            let _ = self.retired.try_send(a.track);
        }
    }

    fn maybe_start_crossfade(&mut self) {
        if self.fade.is_some() {
            return;
        }
        let cf = self.shared.crossfade_frames() as u64;
        if cf == 0 {
            return;
        }
        let (Some(cur), Some(next)) = (&self.current, &self.next) else { return };
        let Some(remaining) = cur.remaining() else { return };
        if remaining > cf {
            return;
        }
        // Don't fade for longer than either track has to give.
        let mut len = cf.min(remaining);
        if let Some(t) = next.track.total_frames {
            len = len.min(t / 2);
        }
        if len == 0 {
            return;
        }
        let outgoing = self.current.take().unwrap();
        self.fade = Some(Fade { outgoing, done: 0, len });
        let next = self.next.take().unwrap();
        self.begin(next);
    }

    /// Mixes the sources for one chunk. While pausing, only the frames the ramp still lets through
    /// are read from the sources, so pausing consumes exactly the audio that was heard.
    fn mix_chunk(&mut self, chunk: &mut [f32]) {
        let n = if self.playing {
            chunk.len() / 2
        } else {
            // play_gain is a multiple of 1/PAUSE_RAMP_FRAMES, so this is an exact frame count.
            ((self.play_gain * PAUSE_RAMP_FRAMES as f32).ceil() as usize).min(chunk.len() / 2)
        };
        chunk[n * 2..].fill(0.0);
        if n == 0 {
            return;
        }
        let chunk = &mut chunk[..n * 2];
        self.maybe_start_crossfade();

        // --- current track ---
        self.in_buf[..n * 2].fill(0.0);
        let mut filled = 0;
        while filled < n {
            let Some(cur) = self.current.as_mut() else { break };
            let (got, ended) = cur.track.source.read(&mut self.in_buf[filled * 2..n * 2]);
            let g = cur.track.gain;
            if g != 1.0 {
                for s in &mut self.in_buf[filled * 2..(filled + got) * 2] {
                    *s *= g;
                }
            }
            cur.played += got as u64;
            filled += got;
            if ended {
                let done = self.current.take();
                self.retire_active(done);
                self.promote_next();
            } else {
                if filled < n {
                    self.shared.underruns.fetch_add(1, Ordering::Relaxed);
                }
                break;
            }
        }

        // --- crossfade with the outgoing track, if any ---
        match self.fade.as_mut() {
            None => chunk.copy_from_slice(&self.in_buf[..n * 2]),
            Some(fade) => {
                self.out_buf[..n * 2].fill(0.0);
                let (got, _) = fade.outgoing.track.source.read(&mut self.out_buf[..n * 2]);
                fade.outgoing.played += got as u64;
                let g = fade.outgoing.track.gain;
                for i in 0..n {
                    let t = ((fade.done + i as u64) as f32 / fade.len as f32).min(1.0);
                    // Equal power: constant loudness across the fade for unrelated material.
                    let (gain_in, gain_out) = ((t * FRAC_PI_2).sin(), (t * FRAC_PI_2).cos());
                    for ch in 0..2 {
                        chunk[i * 2 + ch] =
                            self.in_buf[i * 2 + ch] * gain_in + self.out_buf[i * 2 + ch] * g * gain_out;
                    }
                }
                fade.done += n as u64;
                if fade.done >= fade.len {
                    let f = self.fade.take().unwrap();
                    self.retire_active(Some(f.outgoing));
                }
            }
        }
    }

    /// Volume, pause ramp, and a soft limiter so EQ boosts can't hard-clip.
    fn apply_output_stage(&mut self, out: &mut [f32]) {
        let sr = self.sample_rate as f32;
        let target_vol = if self.shared.muted() { 0.0 } else { self.shared.volume() };
        // ~10 ms smoothing for volume changes.
        let vol_k = 1.0 - (-1.0 / (0.010 * sr)).exp();
        let ramp = 1.0 / PAUSE_RAMP_FRAMES as f32;
        let play_target = if self.playing { 1.0 } else { 0.0 };
        for frame in out.chunks_exact_mut(2) {
            self.volume += (target_vol - self.volume) * vol_k;
            self.play_gain += (play_target - self.play_gain).clamp(-ramp, ramp);
            let g = self.volume * self.play_gain;
            frame[0] = soft_clip(frame[0] * g);
            frame[1] = soft_clip(frame[1] * g);
        }
    }
}

/// Leaves everything below 0.8 untouched and rounds off anything above it, never exceeding 1.0.
fn soft_clip(x: f32) -> f32 {
    const KNEE: f32 = 0.8;
    let a = x.abs();
    if a <= KNEE {
        x
    } else {
        x.signum() * (KNEE + (1.0 - KNEE) * ((a - KNEE) / (1.0 - KNEE)).tanh())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::bounded;

    const SR: u32 = 48_000;

    /// Plays back a fixed buffer; optionally withholds frames to simulate underruns.
    struct VecSource {
        data: Vec<f32>,
        pos: usize,
        /// After this many frames have been served, serve nothing for `stall_reads` calls.
        stall_at: Option<usize>,
        stall_reads: usize,
    }

    impl VecSource {
        fn new(data: Vec<f32>) -> Box<Self> {
            Box::new(Self { data, pos: 0, stall_at: None, stall_reads: 0 })
        }
        /// A constant level on both channels.
        fn constant(level: f32, frames: usize) -> Box<Self> {
            Self::new(vec![level; frames * 2])
        }
        /// `base + i * step` on both channels, so ordering, gaps and repeats are visible.
        fn ramp(frames: usize, base: f32, step: f32) -> Box<Self> {
            Self::new((0..frames).flat_map(|i| [base + i as f32 * step; 2]).collect())
        }
    }

    impl FrameSource for VecSource {
        fn read(&mut self, out: &mut [f32]) -> (usize, bool) {
            if let Some(at) = self.stall_at {
                if self.pos >= at * 2 && self.stall_reads > 0 {
                    self.stall_reads -= 1;
                    return (0, false);
                }
            }
            let mut want = out.len().min(self.data.len() - self.pos);
            if let Some(at) = self.stall_at {
                if self.pos < at * 2 {
                    want = want.min(at * 2 - self.pos);
                }
            }
            out[..want].copy_from_slice(&self.data[self.pos..self.pos + want]);
            self.pos += want;
            (want / 2, self.pos >= self.data.len())
        }
    }

    struct Rig {
        mixer: Mixer,
        cmd: Sender<Command>,
        events: Receiver<Event>,
        retired: Receiver<Track>,
        shared: Arc<Shared>,
        eq: Arc<EqParams>,
    }

    fn rig() -> Rig {
        let (cmd, commands) = bounded(64);
        let (events_tx, events) = bounded(64);
        let (retired_tx, retired) = bounded(64);
        let shared = Shared::new();
        let eq = EqParams::new();
        eq.set_enabled(false); // most tests want a transparent signal path
        let mixer = Mixer::new(
            SR,
            shared.clone(),
            eq.clone(),
            VizTap::new(),
            MixerPorts { commands, events: events_tx, retired: retired_tx },
        );
        Rig { mixer, cmd, events, retired, shared, eq }
    }

    fn track(id: u64, source: Box<dyn FrameSource>, frames: usize) -> Track {
        Track { id, source, total_frames: Some(frames as u64), start_frame: 0, gain: 1.0 }
    }

    impl Rig {
        /// Skips the play ramp-up, so tests can read exact sample values from frame 0.
        fn ready(mut self) -> Self {
            self.mixer.playing = true;
            self.mixer.play_gain = 1.0;
            self.shared.playing.store(true, Ordering::Relaxed);
            self
        }
        fn start(&self, id: u64, source: Box<dyn FrameSource>, frames: usize) {
            self.cmd.send(Command::Start { track: track(id, source, frames), fade_frames: 0 }).unwrap();
        }
        fn queue(&self, id: u64, source: Box<dyn FrameSource>, frames: usize) {
            self.cmd.send(Command::SetNext(Some(track(id, source, frames)))).unwrap();
        }
        /// Renders `frames` frames in irregular callback sizes, returning the left channel.
        fn run(&mut self, frames: usize) -> Vec<f32> {
            let mut left = Vec::with_capacity(frames);
            let mut buf = Vec::new();
            let sizes = [480, 512, 1024, 333, 96, 2048];
            let mut i = 0;
            while left.len() < frames {
                let n = sizes[i % sizes.len()].min(frames - left.len());
                i += 1;
                buf.clear();
                buf.resize(n * 2, 0.0);
                self.mixer.render(&mut buf);
                left.extend(buf.chunks_exact(2).map(|f| f[0]));
            }
            left
        }
        fn events(&self) -> Vec<Event> {
            self.events.try_iter().collect()
        }
        /// Lets volume smoothing and the pause ramp settle.
        fn settle(&mut self) {
            self.run(4800);
        }
    }

    #[test]
    fn plays_a_track_exactly_then_reports_the_end() {
        let mut r = rig().ready();
        r.start(1, VecSource::ramp(1000, 0.0, 0.0005), 1000);
        let out = r.run(1500);
        for (i, v) in out[..1000].iter().enumerate() {
            assert!((v - i as f32 * 0.0005).abs() < 1e-6, "frame {i} is {v}");
        }
        assert!(out[1000..].iter().all(|&v| v == 0.0), "silence after the end");
        assert_eq!(r.events(), vec![Event::TrackStarted { id: 1 }, Event::NeedNext, Event::PlaybackEnded]);
        assert_eq!(r.shared.current_id(), 0);
        assert_eq!(r.retired.try_recv().unwrap().id, 1, "finished tracks go back to the control thread");
    }

    #[test]
    fn consecutive_tracks_are_gapless() {
        let mut r = rig().ready();
        r.start(1, VecSource::ramp(1000, 0.0, 0.0001), 1000);
        r.queue(2, VecSource::ramp(700, 0.5, 0.0001), 700);
        let out = r.run(2000);
        for i in 0..1000 {
            assert!((out[i] - 0.0001 * i as f32).abs() < 1e-6, "track 1, frame {i}");
        }
        for i in 0..700 {
            let v = out[1000 + i];
            assert!((v - (0.5 + 0.0001 * i as f32)).abs() < 1e-6, "track 2 frame {i} is {v}: gap or overlap at the join?");
        }
        assert!(out[1700..].iter().all(|&v| v == 0.0));
        assert_eq!(
            r.events(),
            // NeedNext after each start: the queue slot was empty (or became empty) at that moment.
            vec![
                Event::TrackStarted { id: 1 },
                Event::NeedNext,
                Event::TrackStarted { id: 2 },
                Event::NeedNext,
                Event::PlaybackEnded
            ]
        );
    }

    #[test]
    fn crossfade_overlaps_with_equal_power_curves() {
        let mut r = rig().ready();
        let (len_a, len_b, cf) = (24_000usize, 24_000usize, 4_800usize);
        r.shared.set_crossfade_frames(cf as u32);
        r.start(1, VecSource::constant(0.4, len_a), len_a);
        r.queue(2, VecSource::constant(0.2, len_b), len_b);
        let out = r.run(len_a + len_b);

        // The fade starts at the first chunk boundary where A has `cf` or fewer frames left, and
        // always ends exactly where A ends. The audible length therefore tells us its length.
        let audible = out.iter().rposition(|&v| v != 0.0).unwrap() + 1;
        let len = len_a + len_b - audible;
        assert!(len <= cf && len + CHUNK > cf, "fade length {len}, requested {cf}");
        let s = len_a - len;
        assert!(out[..s].iter().all(|&v| (v - 0.4).abs() < 1e-6), "pure A before the fade");

        // Inside the fade the output is exactly A*cos + B*sin.
        for i in s..len_a {
            let t = (i - s) as f32 / len as f32;
            let expected = 0.4 * (t * FRAC_PI_2).cos() + 0.2 * (t * FRAC_PI_2).sin();
            assert!((out[i] - expected).abs() < 1e-4, "frame {i}: {} vs {expected}", out[i]);
        }
        // Equal power: halfway through, each track is at -3 dB.
        let mid = out[s + len / 2];
        assert!((mid - 0.6 * std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-3, "midpoint {mid}");

        // After the fade it's pure B.
        assert!((out[len_a + 100] - 0.2).abs() < 1e-6);
        assert!(r.events().contains(&Event::TrackStarted { id: 2 }));
    }

    #[test]
    fn crossfade_is_shortened_when_the_next_track_is_queued_late() {
        let mut r = rig().ready();
        r.shared.set_crossfade_frames(9_600);
        r.start(1, VecSource::constant(0.4, 12_000), 12_000);
        r.run(10_000); // 2000 frames of A remain: less than the fade window
        r.queue(2, VecSource::constant(0.2, 12_000), 12_000);
        let out = r.run(4_000);
        // The overlap can only be as long as A had left, so B is fully in right as A ends.
        let mid = out[1_000];
        assert!((mid - 0.6 * std::f32::consts::FRAC_1_SQRT_2).abs() < 2e-3, "mid-fade level {mid}");
        assert!((out[2_100] - 0.2).abs() < 1e-6, "B at full level after A's end: {}", out[2_100]);
    }

    #[test]
    fn unknown_length_tracks_are_never_crossfaded_out_of() {
        let mut r = rig().ready();
        r.shared.set_crossfade_frames(4800);
        let live = Track { total_frames: None, ..track(1, VecSource::constant(0.4, 10_000), 10_000) };
        r.cmd.send(Command::Start { track: live, fade_frames: 0 }).unwrap();
        r.queue(2, VecSource::constant(0.2, 10_000), 10_000);
        let out = r.run(10_000);
        assert!(out.iter().all(|&v| (v - 0.4).abs() < 1e-6), "A must play to its end untouched");
        assert!((r.run(500)[100] - 0.2).abs() < 1e-6, "then B follows");
    }

    #[test]
    fn pausing_ramps_out_and_resuming_loses_nothing() {
        let mut r = rig();
        r.cmd.send(Command::Play).unwrap();
        let n = 200_000;
        r.start(1, VecSource::constant(0.3, n), n);
        r.settle();
        let before = r.run(5_000);
        assert!((before[4_000] - 0.3).abs() < 1e-6);
        let pos_before = r.shared.position_frames();

        r.cmd.send(Command::Pause).unwrap();
        let during = r.run(10_000);
        let pos_paused = r.shared.position_frames();
        // A short ramp, not a cut...
        assert!(during[0] > 0.29 && during[0] < 0.3 + 1e-6, "starts from full level: {}", during[0]);
        let ramp = PAUSE_RAMP_FRAMES as u64;
        assert!(during[(ramp - 1) as usize] < 0.001, "fully faded after {ramp} frames");
        assert!(during[ramp as usize..].iter().all(|&v| v == 0.0));
        let max_step = during.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step < 0.001, "pause ramp has steps of {max_step}");
        // ...and it consumed exactly the ramp's worth of audio, not the whole callback.
        assert_eq!(pos_paused - pos_before, ramp, "audio consumed while pausing");

        let still = r.run(10_000);
        assert_eq!(r.shared.position_frames(), pos_paused, "position must not move while paused");
        assert!(!r.shared.playing());
        assert!(still.iter().all(|&v| v == 0.0));

        r.cmd.send(Command::Play).unwrap();
        let after = r.run(5_000);
        assert_eq!(r.shared.position_frames(), pos_paused + 5_000, "resume continues exactly where it stopped");
        assert!(after[0] < 0.001 && (after[4_000] - 0.3).abs() < 1e-6);
    }

    #[test]
    fn start_replaces_the_current_track_with_a_short_fade_and_retires_the_old_one() {
        let mut r = rig().ready();
        r.start(1, VecSource::constant(0.5, 100_000), 100_000);
        r.run(2_000);
        let seek = Track { start_frame: 70_000, ..track(1, VecSource::constant(0.25, 30_000), 100_000) };
        r.cmd.send(Command::Start { track: seek, fade_frames: 480 }).unwrap();
        let out = r.run(2_000);
        assert!((out[0] - 0.5).abs() < 1e-3, "begins at the old level");
        assert!((out[1_000] - 0.25).abs() < 1e-6, "ends on the new source");
        let max_step = out.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0, f32::max);
        assert!(max_step < 0.01, "step of {max_step} during the switch");
        assert_eq!(r.shared.position_frames(), 70_000 + 2_000);
        assert_eq!(r.retired.try_iter().count(), 1, "the replaced source is handed back");
    }

    #[test]
    fn an_underrun_plays_silence_without_losing_or_repeating_audio() {
        let mut r = rig().ready();
        let n = 3000;
        let mut src = VecSource::ramp(n, 0.001, 0.0001);
        src.stall_at = Some(1000);
        src.stall_reads = 3;
        let reference: Vec<f32> = src.data.chunks_exact(2).map(|f| f[0]).collect();
        r.start(1, src, n);
        let out = r.run(6000);
        assert!(r.shared.underruns() >= 1);
        let heard: Vec<f32> = out.iter().copied().filter(|&v| v != 0.0).collect();
        assert_eq!(heard.len(), reference.len(), "every frame played exactly once");
        for (h, e) in heard.iter().zip(&reference) {
            assert!((h - e).abs() < 1e-6);
        }
    }

    #[test]
    fn volume_mute_and_soft_clip() {
        let mut r = rig().ready();
        r.start(1, VecSource::constant(0.5, 500_000), 500_000);
        r.shared.set_volume(0.5);
        r.settle();
        assert!((r.run(100)[50] - 0.25).abs() < 1e-3);
        r.shared.set_muted(true);
        r.settle();
        assert!(r.run(100).iter().all(|v| v.abs() < 1e-4));
        r.shared.set_muted(false);
        r.shared.set_volume(2.0);
        r.settle();
        let loud = r.run(100)[50];
        assert!(loud <= 1.0 && loud > 0.7, "0.5 at +6 dB lands in the limiter's knee, got {loud}");

        assert_eq!(soft_clip(0.5), 0.5);
        assert_eq!(soft_clip(-0.8), -0.8);
        for x in [0.81, 1.0, 1.5, 10.0, 1e6] {
            assert!(soft_clip(x) <= 1.0 && soft_clip(x) > 0.8, "{x} -> {}", soft_clip(x));
            assert_eq!(soft_clip(-x), -soft_clip(x));
        }
        let mut last = 0.0;
        for i in 0..2000 {
            let y = soft_clip(i as f32 * 0.001);
            assert!(y >= last, "limiter must be monotonic");
            last = y;
        }
    }

    #[test]
    fn eq_is_applied_after_mixing() {
        let mut r = rig().ready();
        r.eq.set_enabled(true);
        r.eq.set_preamp_db(-6.0);
        r.start(1, VecSource::constant(0.5, 100_000), 100_000);
        r.settle();
        let v = r.run(100)[50];
        assert!((v - 0.5 * 0.5012).abs() < 0.005, "preamp -6 dB should halve the level, got {v}");
    }

    #[test]
    fn replaygain_scales_a_track() {
        let mut r = rig().ready();
        let t = Track { gain: 0.5, ..track(1, VecSource::constant(0.6, 1000), 1000) };
        r.cmd.send(Command::Start { track: t, fade_frames: 0 }).unwrap();
        assert!((r.run(100)[50] - 0.3).abs() < 1e-5);
    }

    #[test]
    fn stop_silences_and_retires_everything() {
        let mut r = rig().ready();
        r.start(1, VecSource::constant(0.5, 100_000), 100_000);
        r.queue(2, VecSource::constant(0.5, 100_000), 100_000);
        r.run(1000);
        r.cmd.send(Command::Stop).unwrap();
        r.run(2000);
        assert!(r.run(1000).iter().all(|&v| v == 0.0));
        assert_eq!(r.shared.current_id(), 0);
        assert_eq!(r.retired.try_iter().count(), 2);
    }
}
