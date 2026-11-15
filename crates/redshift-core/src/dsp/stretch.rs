//! Play speed and pitch, live, for interleaved stereo.
//!
//! Two stages in a row:
//!
//! 1. **Tempo** (WSOLA, waveform-similarity overlap-add): the song is cut into overlapping windows,
//!    and the windows are laid down closer together (faster) or further apart (slower). Each window
//!    is nudged a few milliseconds to the spot that continues the previous one most smoothly, so
//!    the joins are not heard. The pitch stays where it was.
//! 2. **Pitch** (resampling): the stretched audio is read back faster or slower with cubic
//!    interpolation. Faster raises the pitch and speeds things up, so the tempo stage has
//!    already slowed the music by the same factor and the two cancel.
//!
//! Speed `T` and pitch `P` (both as ratios) therefore ask the tempo stage to stretch by `T / P`,
//! and the resampler to read at `P`. "Tape style" (the pitch follows the speed) is `P = T`, which
//! needs no stretching at all and is exact.
//!
//! The stage pulls its input in small blocks, as it needs it, from a closure. It never allocates
//! after construction, so it is safe on the audio thread.

use std::f32::consts::PI;

/// Frames the caller's closure is asked for at a time.
pub const PULL: usize = 256;
/// Room for the input and output buffers. Far more than any window plus search range needs.
const CAPACITY: usize = 1 << 16;

pub struct Stretcher {
    /// Window length in frames, and the step between windows (half of it).
    win: usize,
    hop: usize,
    /// How far (frames) a window may move either way to find a better join.
    tol: usize,
    window: Vec<f32>,

    /// Source audio, interleaved, `inp_frames` long. Frame 0 is whatever is still needed.
    inp: Vec<f32>,
    inp_frames: usize,
    /// Where the next window nominally starts, and where the last one really started.
    pos: f64,
    prev: Option<usize>,
    /// Overlap-add accumulator, `win` frames.
    acc: Vec<f32>,
    first: bool,

    /// Tempo-stretched audio waiting for the resampler, `wlen` frames, and the read position in it.
    wout: Vec<f32>,
    wlen: usize,
    rp: f64,

    /// Stretch factor of the tempo stage (input frames used per output frame) and read speed of the resampler.
    ts: f64,
    pitch: f64,
    scratch: Vec<f32>,
    mono_target: Vec<f32>,
}

impl Stretcher {
    pub fn new(sample_rate: u32) -> Self {
        let win = ((sample_rate as f32 * 0.040) as usize).max(256) & !1;
        let tol = (sample_rate as f32 * 0.012) as usize;
        let window = (0..win).map(|i| 0.5 - 0.5 * (2.0 * PI * i as f32 / win as f32).cos()).collect();
        let mut s = Self {
            win,
            hop: win / 2,
            tol,
            window,
            inp: Vec::with_capacity(CAPACITY * 2),
            inp_frames: 0,
            pos: 0.0,
            prev: None,
            acc: vec![0.0; win * 2],
            first: true,
            wout: vec![0.0; CAPACITY * 2],
            wlen: 0,
            rp: 1.0,
            ts: 1.0,
            pitch: 1.0,
            scratch: vec![0.0; PULL * 2],
            mono_target: vec![0.0; win / 2],
        };
        s.reset();
        s
    }

    /// Forgets all audio, e.g. at a new track or a seek. The next sound out starts at full level.
    pub fn reset(&mut self) {
        self.inp.clear();
        self.inp_frames = 0;
        self.pos = 0.0;
        self.prev = None;
        self.acc.fill(0.0);
        self.first = true;
        self.wout[..2].fill(0.0); // one frame of history for the interpolator
        self.wlen = 1;
        self.rp = 1.0;
    }

    /// `tempo` is how many seconds of song go by per second heard; `pitch` is the frequency ratio.
    pub fn set(&mut self, tempo: f32, pitch: f32) {
        let tempo = (tempo as f64).clamp(0.25, 4.0);
        let pitch = (pitch as f64).clamp(0.25, 4.0);
        self.pitch = pitch;
        self.ts = tempo / pitch;
    }

    /// Fills `out` (interleaved stereo), asking `pull` for more source audio whenever it is needed.
    /// `pull` must fill the slice it is given (silence is fine).
    pub fn process(&mut self, out: &mut [f32], pull: &mut dyn FnMut(&mut [f32])) {
        for frame in out.chunks_exact_mut(2) {
            let i = self.rp as usize;
            while self.wlen < i + 3 {
                self.step(pull);
                // `step` may have slid the buffer; recompute.
                if self.wlen >= self.rp as usize + 3 {
                    break;
                }
            }
            let i = self.rp as usize;
            let t = (self.rp - i as f64) as f32;
            for ch in 0..2 {
                let g = |k: usize| self.wout[k * 2 + ch];
                frame[ch] = if t == 0.0 { g(i) } else { cubic(g(i - 1), g(i), g(i + 1), g(i + 2), t) };
            }
            self.rp += self.pitch;
        }
        // Drop what the resampler is finished with.
        let keep_from = (self.rp as usize).saturating_sub(1);
        if keep_from > 0 {
            self.wout.copy_within(keep_from * 2..self.wlen * 2, 0);
            self.wlen -= keep_from;
            self.rp -= keep_from as f64;
        }
    }

    fn need_input(&mut self, frames: usize, pull: &mut dyn FnMut(&mut [f32])) {
        while self.inp_frames < frames && self.inp_frames + PULL <= CAPACITY {
            pull(&mut self.scratch);
            self.inp.extend_from_slice(&self.scratch);
            self.inp_frames += PULL;
        }
    }

    /// Produces one hop of tempo-stretched audio.
    fn step(&mut self, pull: &mut dyn FnMut(&mut [f32])) {
        let (win, hop, tol) = (self.win, self.hop, self.tol);
        let nominal = self.pos.round().max(0.0) as usize;
        self.need_input(nominal + tol + win + 2, pull);
        let nominal = nominal.min(self.inp_frames.saturating_sub(win));

        let start = match self.prev {
            Some(prev) if (self.ts - 1.0).abs() > 1e-4 => self.best_start(prev, nominal),
            Some(prev) => prev + hop, // not stretching: the next window is exactly the continuation
            None => nominal,
        };
        let start = start.min(self.inp_frames.saturating_sub(win));

        // Lay the window into the overlap-add accumulator. The very first one starts at full level.
        for i in 0..win {
            let w = if self.first && i < hop { 1.0 } else { self.window[i] };
            self.acc[i * 2] += self.inp[(start + i) * 2] * w;
            self.acc[i * 2 + 1] += self.inp[(start + i) * 2 + 1] * w;
        }
        self.first = false;
        // The first `hop` frames are finished: hand them on and slide the rest down.
        let room = self.wout.len() / 2;
        if self.wlen + hop > room {
            // Cannot happen with sane ratios; refuse to grow rather than allocate.
            self.wlen = self.wlen.saturating_sub(hop);
        }
        self.wout[self.wlen * 2..(self.wlen + hop) * 2].copy_from_slice(&self.acc[..hop * 2]);
        self.wlen += hop;
        self.acc.copy_within(hop * 2.., 0);
        self.acc[(win - hop) * 2..].fill(0.0);

        self.prev = Some(start);
        self.pos += hop as f64 * self.ts;

        // Forget source audio that no window can reach any more.
        let keep_from = start.min((self.pos.round().max(0.0) as usize).saturating_sub(tol)).min(self.inp_frames);
        if keep_from > 0 {
            self.inp.drain(..keep_from * 2);
            self.inp_frames -= keep_from;
            self.pos -= keep_from as f64;
            self.prev = Some(start - keep_from);
        }
    }

    /// Of the places near `nominal`, the one whose first half looks most like the audio that
    /// naturally follows the previous window.
    fn best_start(&mut self, prev: usize, nominal: usize) -> usize {
        let hop = self.hop;
        let target_at = prev + hop;
        if target_at + hop > self.inp_frames {
            return nominal;
        }
        for i in 0..hop {
            self.mono_target[i] = self.inp[(target_at + i) * 2] + self.inp[(target_at + i) * 2 + 1];
        }
        let lo = nominal.saturating_sub(self.tol);
        let hi = (nominal + self.tol).min(self.inp_frames.saturating_sub(self.win));
        let (mut best, mut best_score) = (nominal.min(hi), f32::MIN);
        let mut c = lo;
        while c <= hi {
            let (mut dot, mut energy) = (0.0f32, 1e-9f32);
            let mut i = 0;
            while i < hop {
                let m = self.inp[(c + i) * 2] + self.inp[(c + i) * 2 + 1];
                dot += m * self.mono_target[i];
                energy += m * m;
                i += 2;
            }
            let score = dot / energy.sqrt();
            if score > best_score {
                best_score = score;
                best = c;
            }
            c += 1;
        }
        best
    }
}

/// Catmull-Rom interpolation between `b` (t = 0) and `c` (t = 1).
#[inline]
fn cubic(a: f32, b: f32, c: f32, d: f32, t: f32) -> f32 {
    let (t2, t3) = (t * t, t * t * t);
    0.5 * ((2.0 * b) + (-a + c) * t + (2.0 * a - 5.0 * b + 4.0 * c - d) * t2 + (-a + 3.0 * b - 3.0 * c + d) * t3)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: u32 = 48_000;

    /// A stereo sine, pulled in blocks. Counts how many source frames were used.
    struct Tone {
        hz: f64,
        at: u64,
    }
    impl Tone {
        fn pull(&mut self, buf: &mut [f32]) {
            for f in buf.chunks_exact_mut(2) {
                let v = (2.0 * std::f64::consts::PI * self.hz * self.at as f64 / SR as f64).sin() as f32 * 0.5;
                f[0] = v;
                f[1] = v;
                self.at += 1;
            }
        }
    }

    /// Runs `secs` of output and returns (output left channel, source frames used).
    fn run(tempo: f32, pitch: f32, hz: f64, secs: f32) -> (Vec<f32>, u64) {
        let mut s = Stretcher::new(SR);
        s.set(tempo, pitch);
        let mut tone = Tone { hz, at: 0 };
        let mut out = vec![0.0f32; (SR as f32 * secs) as usize * 2];
        for block in out.chunks_mut(512 * 2) {
            s.process(block, &mut |b| tone.pull(b));
        }
        (out.chunks_exact(2).map(|f| f[0]).collect(), tone.at)
    }

    /// Frequency of a clean tone from its rising zero crossings, ignoring the first and last 20%.
    fn hz_of(x: &[f32]) -> f64 {
        let (a, b) = (x.len() / 5, x.len() * 4 / 5);
        let n = x[a..b].windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count();
        n as f64 / ((b - a) as f64 / SR as f64)
    }

    fn rms(x: &[f32]) -> f32 {
        (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
    }

    #[test]
    fn untouched_audio_passes_through_exactly() {
        let (out, used) = run(1.0, 1.0, 440.0, 1.0);
        let mut tone = Tone { hz: 440.0, at: 0 };
        let mut want = vec![0.0; out.len() * 2];
        tone.pull(&mut want);
        let worst = out.iter().zip(want.chunks_exact(2)).map(|(a, w)| (a - w[0]).abs()).fold(0.0, f32::max);
        assert!(worst < 1e-5, "worst difference {worst}");
        assert!(used >= out.len() as u64 && used < out.len() as u64 + 8_000, "used {used}");
    }

    #[test]
    fn faster_tempo_keeps_the_pitch_and_uses_more_source() {
        let (out, used) = run(1.5, 1.0, 440.0, 2.0);
        let hz = hz_of(&out);
        assert!((hz - 440.0).abs() < 3.0, "pitch moved to {hz} Hz");
        let ratio = used as f64 / out.len() as f64;
        assert!((ratio - 1.5).abs() < 0.08, "source/output {ratio}");
        assert!((rms(&out[out.len() / 5..]) - 0.3535).abs() < 0.03, "level {}", rms(&out[out.len() / 5..]));
    }

    #[test]
    fn slower_tempo_keeps_the_pitch_and_uses_less_source() {
        let (out, used) = run(0.6, 1.0, 330.0, 2.0);
        let hz = hz_of(&out);
        assert!((hz - 330.0).abs() < 3.0, "pitch moved to {hz} Hz");
        let ratio = used as f64 / out.len() as f64;
        assert!((ratio - 0.6).abs() < 0.08, "source/output {ratio}");
    }

    #[test]
    fn pitch_alone_moves_the_pitch_and_keeps_the_tempo() {
        // An octave up at the same speed.
        let (out, used) = run(1.0, 2.0, 300.0, 2.0);
        let hz = hz_of(&out);
        assert!((hz - 600.0).abs() < 6.0, "pitch is {hz} Hz");
        let ratio = used as f64 / out.len() as f64;
        assert!((ratio - 1.0).abs() < 0.08, "source/output {ratio}");
    }

    #[test]
    fn tape_style_moves_both_together() {
        // Speed and pitch both 1.25: no stretching, just reading faster.
        let (out, used) = run(1.25, 1.25, 400.0, 2.0);
        let hz = hz_of(&out);
        assert!((hz - 500.0).abs() < 4.0, "pitch is {hz} Hz");
        let ratio = used as f64 / out.len() as f64;
        assert!((ratio - 1.25).abs() < 0.05, "source/output {ratio}");
    }

    #[test]
    fn speed_and_pitch_can_be_set_independently() {
        // Slower and a fifth up.
        let p = 2f32.powf(7.0 / 12.0);
        let (out, used) = run(0.8, p, 250.0, 2.0);
        let hz = hz_of(&out);
        assert!((hz - 250.0 * p as f64).abs() < 5.0, "pitch is {hz} Hz");
        let ratio = used as f64 / out.len() as f64;
        assert!((ratio - 0.8).abs() < 0.08, "source/output {ratio}");
    }

    #[test]
    fn changing_the_settings_while_running_is_safe_and_quiet_of_clicks() {
        let mut s = Stretcher::new(SR);
        let mut tone = Tone { hz: 440.0, at: 0 };
        let mut out = vec![0.0f32; 512 * 2];
        let mut peak = 0.0f32;
        for i in 0..400 {
            let t = 0.5 + 1.5 * ((i as f32 * 0.07).sin() * 0.5 + 0.5);
            s.set(t, 1.0 + 0.3 * (i as f32 * 0.05).cos());
            s.process(&mut out, &mut |b| tone.pull(b));
            peak = out.iter().fold(peak, |m, v| m.max(v.abs()));
            assert!(out.iter().all(|v| v.is_finite()));
        }
        assert!(peak < 0.8, "peak {peak}: a join clipped");
    }

    #[test]
    fn reset_starts_clean_at_full_level() {
        let mut s = Stretcher::new(SR);
        s.set(1.4, 1.0);
        let mut tone = Tone { hz: 440.0, at: 0 };
        let mut out = vec![0.0f32; 512 * 2];
        s.process(&mut out, &mut |b| tone.pull(b));
        s.reset();
        let mut tone = Tone { hz: 1000.0, at: 0 };
        s.process(&mut out, &mut |b| tone.pull(b));
        // The new tone starts with sin(0) = 0 and must reach a full swing within a few cycles: no fade-in.
        let swing = out[..400 * 2].chunks_exact(2).map(|f| f[0].abs()).fold(0.0, f32::max);
        assert!(swing > 0.45, "start swing {swing}");
    }
}
