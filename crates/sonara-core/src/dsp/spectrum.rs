//! Data for the visualisers: spectrum bars, oscilloscope traces and level meters.
//!
//! The audio thread writes what it plays into a [`VizTap`] (a circular buffer behind a lock that is
//! only ever `try_lock`ed, so it can't block). The UI side owns an [`Analyzer`] and calls
//! [`Analyzer::snapshot`] at its frame rate. Everything the front end needs to draw comes out as
//! plain numbers, so the actual drawing (bars, rings, scope, particles) lives entirely in the UI.

use std::sync::Arc;
use std::time::Instant;

use parking_lot::Mutex;
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use serde::{Deserialize, Serialize};

/// Frames kept in the tap. Must cover the FFT size plus the oscilloscope window.
const TAP_FRAMES: usize = 16_384;
pub const FFT_SIZE: usize = 4096;
pub const SCOPE_POINTS: usize = 512;
/// Frames of audio the oscilloscope looks at (about 43 ms at 48 kHz).
const SCOPE_WINDOW: usize = 2048;
const METER_WINDOW: usize = 1024;

struct TapBuf {
    /// Interleaved stereo.
    data: Vec<f32>,
    /// Frame index where the next write goes.
    head: usize,
    /// Frames ever written, saturating at `TAP_FRAMES`.
    filled: usize,
}

/// The audio-thread end.
#[derive(Clone)]
pub struct VizTap {
    buf: Arc<Mutex<TapBuf>>,
}

impl VizTap {
    pub fn new() -> Self {
        Self {
            buf: Arc::new(Mutex::new(TapBuf { data: vec![0.0; TAP_FRAMES * 2], head: 0, filled: 0 })),
        }
    }

    /// Records interleaved stereo audio. Skips the write if the UI happens to be reading.
    pub fn write(&self, interleaved: &[f32]) {
        let Some(mut b) = self.buf.try_lock() else { return };
        // Only the last TAP_FRAMES frames can matter.
        let interleaved = &interleaved[interleaved.len().saturating_sub(TAP_FRAMES * 2)..];
        for frame in interleaved.chunks_exact(2) {
            let h = b.head * 2;
            b.data[h] = frame[0];
            b.data[h + 1] = frame[1];
            b.head = (b.head + 1) % TAP_FRAMES;
        }
        b.filled = (b.filled + interleaved.len() / 2).min(TAP_FRAMES);
    }

    /// Clears history, so the visualisers fall quiet after a stop.
    pub fn clear(&self) {
        if let Some(mut b) = self.buf.try_lock() {
            b.filled = 0;
        }
    }

    /// The most recent `frames` frames in time order, left-padded with silence if there are fewer.
    fn latest(&self, frames: usize, into: &mut Vec<f32>) {
        let b = self.buf.lock();
        into.clear();
        into.resize(frames * 2, 0.0);
        let avail = b.filled.min(frames);
        for i in 0..avail {
            let src = (b.head + TAP_FRAMES - avail + i) % TAP_FRAMES;
            let dst = (frames - avail + i) * 2;
            into[dst] = b.data[src * 2];
            into[dst + 1] = b.data[src * 2 + 1];
        }
    }
}

impl Default for VizTap {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SpectrumConfig {
    pub bars: usize,
    pub min_hz: f32,
    pub max_hz: f32,
    /// Level that maps to 0.0 and to 1.0.
    pub floor_db: f32,
    pub ceil_db: f32,
    /// How fast bars fall, in full-scale units per second. Rising is instant.
    pub fall_per_sec: f32,
    /// How long a peak marker holds before falling, in seconds.
    pub peak_hold_secs: f32,
}

impl Default for SpectrumConfig {
    fn default() -> Self {
        Self {
            bars: 64,
            min_hz: 30.0,
            max_hz: 16_000.0,
            floor_db: -80.0,
            ceil_db: -10.0,
            fall_per_sec: 1.6,
            peak_hold_secs: 0.6,
        }
    }
}

/// One frame of visualiser data. Serialises straight to JSON for the UI.
#[derive(Debug, Clone, Serialize)]
pub struct VizFrame {
    /// Bar heights, 0..1, low to high frequency.
    pub bars: Vec<f32>,
    /// Peak-hold markers for the bars, 0..1.
    pub peaks: Vec<f32>,
    /// Oscilloscope traces, -1..1, aligned to a rising zero crossing so a steady tone holds still.
    pub scope_left: Vec<f32>,
    pub scope_right: Vec<f32>,
    pub rms_left: f32,
    pub rms_right: f32,
    pub peak_left: f32,
    pub peak_right: f32,
    /// Roughly the bass energy, 0..1; handy for pulsing effects.
    pub bass: f32,
}

pub struct Analyzer {
    tap: VizTap,
    sample_rate: f32,
    cfg: SpectrumConfig,
    fft: Arc<dyn Fft<f32>>,
    window: Vec<f32>,
    window_sum: f32,
    /// For each bar, the (fractional) FFT bin range it covers.
    bar_ranges: Vec<(f32, f32)>,
    bars: Vec<f32>,
    peaks: Vec<f32>,
    peak_age: Vec<f32>,
    last: Option<Instant>,
    frames: Vec<f32>,
    fft_buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Analyzer {
    pub fn new(tap: VizTap, sample_rate: u32, cfg: SpectrumConfig) -> Self {
        let fft = FftPlanner::new().plan_fft_forward(FFT_SIZE);
        let window: Vec<f32> = (0..FFT_SIZE)
            .map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos())
            .collect();
        let window_sum = window.iter().sum();
        let mut a = Self {
            tap,
            sample_rate: sample_rate as f32,
            cfg,
            scratch: vec![Complex::new(0.0, 0.0); fft.get_inplace_scratch_len()],
            fft,
            window,
            window_sum,
            bar_ranges: Vec::new(),
            bars: Vec::new(),
            peaks: Vec::new(),
            peak_age: Vec::new(),
            last: None,
            frames: Vec::new(),
            fft_buf: vec![Complex::new(0.0, 0.0); FFT_SIZE],
        };
        a.configure(cfg);
        a
    }

    pub fn config(&self) -> SpectrumConfig {
        self.cfg
    }

    pub fn configure(&mut self, cfg: SpectrumConfig) {
        self.cfg = SpectrumConfig { bars: cfg.bars.clamp(4, 256), ..cfg };
        let n = self.cfg.bars;
        let nyquist = self.sample_rate / 2.0;
        let max_hz = self.cfg.max_hz.min(nyquist * 0.98);
        let min_hz = self.cfg.min_hz.clamp(1.0, max_hz / 2.0);
        let hz_per_bin = self.sample_rate / FFT_SIZE as f32;
        let ratio = (max_hz / min_hz).powf(1.0 / n as f32);
        self.bar_ranges = (0..n)
            .map(|i| {
                let lo = min_hz * ratio.powi(i as i32);
                (lo / hz_per_bin, lo * ratio / hz_per_bin)
            })
            .collect();
        self.bars = vec![0.0; n];
        self.peaks = vec![0.0; n];
        self.peak_age = vec![0.0; n];
    }

    /// Samples the current audio and advances smoothing by the time since the previous call.
    pub fn snapshot(&mut self) -> VizFrame {
        let now = Instant::now();
        let dt = self.last.map_or(1.0 / 60.0, |t| (now - t).as_secs_f32()).clamp(0.001, 0.25);
        self.last = Some(now);
        self.snapshot_with_dt(dt)
    }

    /// As [`snapshot`](Self::snapshot) with an explicit time step; deterministic, for tests.
    pub fn snapshot_with_dt(&mut self, dt: f32) -> VizFrame {
        self.tap.latest(SCOPE_WINDOW.max(FFT_SIZE), &mut self.frames);
        let total = self.frames.len() / 2;

        // ---- spectrum: Hann-windowed FFT of the most recent FFT_SIZE frames, mono mix ----
        for i in 0..FFT_SIZE {
            let f = total - FFT_SIZE + i;
            let mono = 0.5 * (self.frames[f * 2] + self.frames[f * 2 + 1]);
            self.fft_buf[i] = Complex::new(mono * self.window[i], 0.0);
        }
        self.fft.process_with_scratch(&mut self.fft_buf, &mut self.scratch);
        // A full-scale sine reads 1.0 (0 dB) in its bin.
        let norm = 2.0 / self.window_sum;
        let mag = |bin: f32| -> f32 {
            // Linear interpolation between bins, so bars narrower than one bin still move smoothly.
            let b = bin.clamp(0.0, (FFT_SIZE / 2 - 1) as f32);
            let (i, frac) = (b as usize, b.fract());
            let a = self.fft_buf[i].norm();
            let c = self.fft_buf[(i + 1).min(FFT_SIZE / 2 - 1)].norm();
            (a + (c - a) * frac) * norm
        };
        let span = (self.cfg.ceil_db - self.cfg.floor_db).max(1.0);
        let fall = self.cfg.fall_per_sec * dt;
        for (i, &(lo, hi)) in self.bar_ranges.iter().enumerate() {
            let m = if hi - lo < 1.0 {
                mag((lo + hi) / 2.0)
            } else {
                // Wide bar: take the loudest bin it covers.
                let (first, last) = (lo.ceil() as usize, (hi.floor() as usize).max(lo.ceil() as usize));
                (first..=last).map(|b| mag(b as f32)).fold(0.0, f32::max)
            };
            let db = 20.0 * m.max(1e-9).log10();
            let level = ((db - self.cfg.floor_db) / span).clamp(0.0, 1.0);
            self.bars[i] = if level >= self.bars[i] { level } else { (self.bars[i] - fall).max(level) };

            if self.bars[i] >= self.peaks[i] {
                self.peaks[i] = self.bars[i];
                self.peak_age[i] = 0.0;
            } else {
                self.peak_age[i] += dt;
                if self.peak_age[i] > self.cfg.peak_hold_secs {
                    self.peaks[i] = (self.peaks[i] - fall * 0.6).max(self.bars[i]);
                }
            }
        }
        let bass_bars = (self.cfg.bars / 8).max(1);
        let bass = self.bars[..bass_bars].iter().sum::<f32>() / bass_bars as f32;

        // ---- oscilloscope: start at a rising zero crossing of the left channel ----
        let scope_start = total - SCOPE_WINDOW;
        let half = SCOPE_WINDOW / 2;
        let trigger = (0..half)
            .find(|&i| {
                let (a, b) = (self.frames[(scope_start + i) * 2], self.frames[(scope_start + i + 1) * 2]);
                a <= 0.0 && b > 0.0
            })
            .unwrap_or(0);
        let begin = scope_start + trigger;
        let len = half; // frames shown
        let (mut scope_left, mut scope_right) =
            (Vec::with_capacity(SCOPE_POINTS), Vec::with_capacity(SCOPE_POINTS));
        for p in 0..SCOPE_POINTS {
            let (a, b) = (begin + p * len / SCOPE_POINTS, begin + (p + 1) * len / SCOPE_POINTS);
            let n = (b - a).max(1) as f32;
            let (mut l, mut r) = (0.0, 0.0);
            for f in a..b.max(a + 1) {
                l += self.frames[f * 2];
                r += self.frames[f * 2 + 1];
            }
            scope_left.push(l / n);
            scope_right.push(r / n);
        }

        // ---- meters over the last ~20 ms ----
        let (mut sl, mut sr, mut pl, mut pr) = (0.0f32, 0.0f32, 0.0f32, 0.0f32);
        for f in (total - METER_WINDOW)..total {
            let (l, r) = (self.frames[f * 2], self.frames[f * 2 + 1]);
            sl += l * l;
            sr += r * r;
            pl = pl.max(l.abs());
            pr = pr.max(r.abs());
        }
        let n = METER_WINDOW as f32;
        VizFrame {
            bars: self.bars.clone(),
            peaks: self.peaks.clone(),
            scope_left,
            scope_right,
            rms_left: (sl / n).sqrt(),
            rms_right: (sr / n).sqrt(),
            peak_left: pl,
            peak_right: pr,
            bass,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 48_000;

    fn sine(freq: f32, amp: f32, frames: usize, offset: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = amp * (2.0 * std::f32::consts::PI * freq * (i + offset) as f32 / FS as f32).sin();
                [v, v * 0.5]
            })
            .collect()
    }

    fn analyzer_with(audio: &[f32]) -> Analyzer {
        let tap = VizTap::new();
        tap.write(audio);
        Analyzer::new(tap, FS, SpectrumConfig { fall_per_sec: 100.0, ..SpectrumConfig::default() })
    }

    fn bar_centre_hz(cfg: &SpectrumConfig, i: usize) -> f32 {
        let ratio = (cfg.max_hz / cfg.min_hz).powf(1.0 / cfg.bars as f32);
        cfg.min_hz * ratio.powf(i as f32 + 0.5)
    }

    #[test]
    fn silence_is_flat() {
        let mut a = analyzer_with(&vec![0.0; 2 * 8192]);
        let f = a.snapshot_with_dt(0.016);
        assert!(f.bars.iter().all(|&b| b == 0.0));
        assert_eq!((f.rms_left, f.peak_left, f.bass), (0.0, 0.0, 0.0));
    }

    #[test]
    fn nothing_played_yet_is_also_silent() {
        let mut a = Analyzer::new(VizTap::new(), FS, SpectrumConfig::default());
        let f = a.snapshot_with_dt(0.016);
        assert!(f.bars.iter().all(|&b| b == 0.0));
    }

    #[test]
    fn a_tone_lights_up_the_bar_at_its_frequency() {
        for freq in [100.0, 440.0, 1000.0, 5000.0, 12_000.0] {
            let mut a = analyzer_with(&sine(freq, 0.5, 8192, 0));
            let f = a.snapshot_with_dt(0.016);
            let loudest = f.bars.iter().enumerate().max_by(|x, y| x.1.total_cmp(y.1)).unwrap().0;
            let centre = bar_centre_hz(&a.config(), loudest);
            let ratio = (a.config().max_hz / a.config().min_hz).powf(1.0 / a.config().bars as f32);
            assert!(
                (centre / freq).max(freq / centre) <= ratio * 1.05,
                "{freq} Hz tone peaked in the bar centred on {centre:.0} Hz"
            );
            assert!(f.bars[loudest] > 0.8, "{freq} Hz at -6 dBFS only reached {}", f.bars[loudest]);
        }
    }

    #[test]
    fn bar_height_follows_level_in_db() {
        let cfg = SpectrumConfig { fall_per_sec: 100.0, ..SpectrumConfig::default() };
        let height = |amp: f32| {
            let mut a = analyzer_with(&sine(1000.0, amp, 8192, 0));
            a.snapshot_with_dt(0.016).bars.iter().cloned().fold(0.0, f32::max)
        };
        // -20 dB per decade of amplitude: 0.5 -> 0.05 should drop 20 dB = 20/70 of the range.
        let drop = height(0.5) - height(0.05);
        let expected = 20.0 / (cfg.ceil_db - cfg.floor_db);
        assert!((drop - expected).abs() < 0.03, "dropped {drop}, expected {expected}");
    }

    #[test]
    fn bars_fall_gradually_and_peaks_hold() {
        let tap = VizTap::new();
        tap.write(&sine(1000.0, 0.5, 8192, 0));
        let mut a = Analyzer::new(tap.clone(), FS, SpectrumConfig::default());
        let loud = a.snapshot_with_dt(0.016);
        let top = loud.bars.iter().cloned().fold(0.0, f32::max);

        tap.write(&vec![0.0; 2 * 16_384]); // the music stops
        let after = a.snapshot_with_dt(0.1);
        let now = after.bars.iter().cloned().fold(0.0, f32::max);
        let default_fall = SpectrumConfig::default().fall_per_sec;
        assert!((top - now - default_fall * 0.1).abs() < 0.02, "fell from {top} to {now} in 0.1 s");
        // The peak marker is still where the bar was.
        assert!((after.peaks.iter().cloned().fold(0.0, f32::max) - top).abs() < 0.01);
        // After enough time everything is back down.
        let mut last = after;
        for _ in 0..40 {
            last = a.snapshot_with_dt(0.1);
        }
        assert!(last.bars.iter().chain(&last.peaks).all(|&v| v < 0.01));
    }

    #[test]
    fn meters_read_amplitude_per_channel() {
        let mut a = analyzer_with(&sine(1000.0, 0.8, 8192, 0)); // right channel is half as loud
        let f = a.snapshot_with_dt(0.016);
        assert!((f.peak_left - 0.8).abs() < 0.01, "peak L {}", f.peak_left);
        assert!((f.peak_right - 0.4).abs() < 0.01, "peak R {}", f.peak_right);
        assert!((f.rms_left - 0.8 / 2f32.sqrt()).abs() < 0.01, "rms L {}", f.rms_left);
    }

    #[test]
    fn scope_holds_still_for_a_steady_tone() {
        // The same tone captured at two different moments should draw as the same trace.
        let trace = |offset: usize| {
            let mut a = analyzer_with(&sine(440.0, 0.5, 8192, offset));
            a.snapshot_with_dt(0.016).scope_left
        };
        let (t1, t2) = (trace(0), trace(1234));
        let max_diff = t1.iter().zip(&t2).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(max_diff < 0.05, "traces differ by up to {max_diff}; the trigger isn't locking");
        assert!(t1[0].abs() < 0.1, "trace should start at a zero crossing, starts at {}", t1[0]);
        assert_eq!(t1.len(), SCOPE_POINTS);
    }

    #[test]
    fn tap_wraps_around_correctly() {
        let tap = VizTap::new();
        // Write more than the buffer holds, in odd-sized pieces, with a rising ramp as the signal.
        let total = TAP_FRAMES * 2 + 777;
        let ramp: Vec<f32> = (0..total).flat_map(|i| [i as f32, -(i as f32)]).collect();
        for piece in ramp.chunks(2 * 1000) {
            tap.write(piece);
        }
        let mut out = Vec::new();
        tap.latest(100, &mut out);
        let first = (total - 100) as f32;
        for (i, f) in out.chunks_exact(2).enumerate() {
            assert_eq!(f[0], first + i as f32);
            assert_eq!(f[1], -(first + i as f32));
        }
    }

    #[test]
    fn reconfiguring_bar_count_works() {
        let mut a = analyzer_with(&sine(1000.0, 0.5, 8192, 0));
        for bars in [4, 16, 128, 999] {
            a.configure(SpectrumConfig { bars, ..SpectrumConfig::default() });
            let f = a.snapshot_with_dt(0.016);
            assert_eq!(f.bars.len(), bars.clamp(4, 256));
            assert_eq!(f.peaks.len(), f.bars.len());
        }
    }
}
