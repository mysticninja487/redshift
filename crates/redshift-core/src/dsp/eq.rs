//! Ten-band graphic equaliser: a preamp followed by ten RBJ peaking biquads in series.
//!
//! The UI thread owns an [`EqParams`] (atomics, no locks) and the audio thread owns an
//! [`Equalizer`] that notices parameter changes by watching a version counter. Filter state is kept
//! across coefficient changes, so dragging a slider doesn't click.

use std::f64::consts::PI;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::Arc;

pub const BAND_COUNT: usize = 10;
/// Octave-spaced ISO centre frequencies, the usual graphic-EQ layout.
pub const BAND_FREQS: [f32; BAND_COUNT] =
    [31.25, 62.5, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
/// Q of every band. About √2 makes neighbouring octave bands overlap so a flat boost stays flat.
pub const BAND_Q: f64 = 1.41;
pub const MAX_GAIN_DB: f32 = 12.0;

/// Presets as (name, per-band gain in dB).
pub const PRESETS: &[(&str, [f32; BAND_COUNT])] = &[
    ("Flat", [0.0; BAND_COUNT]),
    ("Bass Boost", [6.0, 5.0, 4.0, 2.0, 0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Bass Reducer", [-6.0, -5.0, -4.0, -2.0, -0.5, 0.0, 0.0, 0.0, 0.0, 0.0]),
    ("Treble Boost", [0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 2.0, 4.0, 5.0, 6.0]),
    ("Vocal", [-2.0, -2.0, -1.0, 1.0, 3.0, 3.5, 3.0, 1.5, 0.0, -1.0]),
    ("Rock", [4.5, 3.5, 2.0, -1.0, -2.0, -0.5, 2.0, 3.5, 4.5, 4.5]),
    ("Pop", [-1.5, 1.0, 3.0, 4.0, 3.0, 0.5, -1.0, -1.5, -1.5, -1.5]),
    ("Jazz", [3.0, 2.0, 1.0, 2.0, -1.5, -1.5, 0.0, 1.0, 2.0, 3.0]),
    ("Classical", [3.5, 2.5, 2.0, 1.5, -1.0, -1.0, 0.0, 2.0, 2.5, 3.0]),
    ("Electronic", [5.0, 4.0, 1.0, 0.0, -2.0, 2.0, 1.0, 1.5, 4.0, 5.0]),
    ("Loudness", [6.0, 4.0, 0.0, 0.0, -1.0, 0.0, -0.5, 1.0, 4.0, 5.0]),
    ("Acoustic", [3.0, 3.0, 2.0, 0.5, 1.0, 1.0, 2.0, 2.5, 3.0, 2.0]),
    ("Hip-Hop", [5.0, 4.5, 2.0, 3.0, -1.0, -0.5, 1.5, -0.5, 2.0, 3.0]),
    ("R&B", [2.5, 6.0, 5.0, 1.5, -2.0, -1.5, 2.0, 2.5, 2.5, 3.0]),
    ("Dance", [6.0, 4.5, 2.5, 0.0, 0.0, -2.5, -3.0, -3.0, 0.0, 0.0]),
    ("Metal", [4.0, 3.0, 0.0, -2.0, -1.0, 1.0, 3.5, 4.0, 3.0, 2.0]),
    ("Piano", [1.5, 0.5, 0.0, 1.5, 2.5, 2.5, 3.0, 2.5, 3.0, 2.5]),
    ("Lounge", [-3.0, -1.5, 0.0, 1.5, 3.0, 2.0, 0.0, -1.5, 2.0, 1.0]),
    ("Live", [-2.5, 0.0, 2.0, 2.5, 3.0, 3.0, 3.0, 2.0, 1.5, 1.5]),
    ("Small Speakers", [6.0, 5.0, 4.0, 3.0, 2.0, 0.0, -1.0, -2.0, -2.5, -3.0]),
    ("Late Night", [4.0, 3.0, 0.0, 0.0, 1.0, 2.0, 1.5, -1.0, -2.0, -3.0]),
    ("Headphones", [4.0, 3.0, 1.5, -1.0, -1.0, 0.5, 1.5, 3.0, 3.5, 3.0]),
    ("Spoken Word", [-5.0, -4.0, -2.0, 1.0, 3.0, 4.0, 4.0, 3.0, 1.0, -2.0]),
];

fn load(a: &AtomicU32) -> f32 {
    f32::from_bits(a.load(Ordering::Relaxed))
}
fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed)
}

/// Equaliser settings, shared between the UI and the audio thread.
#[derive(Debug)]
pub struct EqParams {
    enabled: AtomicBool,
    preamp_db: AtomicU32,
    gains_db: [AtomicU32; BAND_COUNT],
    version: AtomicU32,
}

impl EqParams {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(true),
            preamp_db: AtomicU32::new(0f32.to_bits()),
            gains_db: std::array::from_fn(|_| AtomicU32::new(0f32.to_bits())),
            version: AtomicU32::new(0),
        })
    }

    fn touch(&self) {
        self.version.fetch_add(1, Ordering::Release);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }
    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
        self.touch();
    }

    pub fn preamp_db(&self) -> f32 {
        load(&self.preamp_db)
    }
    pub fn set_preamp_db(&self, db: f32) {
        store(&self.preamp_db, db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB));
        self.touch();
    }

    pub fn band_db(&self, band: usize) -> f32 {
        load(&self.gains_db[band])
    }
    /// Out-of-range band indexes are ignored.
    pub fn set_band_db(&self, band: usize, db: f32) {
        if let Some(slot) = self.gains_db.get(band) {
            store(slot, db.clamp(-MAX_GAIN_DB, MAX_GAIN_DB));
            self.touch();
        }
    }

    pub fn gains_db(&self) -> [f32; BAND_COUNT] {
        std::array::from_fn(|i| self.band_db(i))
    }
    pub fn set_gains_db(&self, gains: &[f32; BAND_COUNT]) {
        for (slot, g) in self.gains_db.iter().zip(gains) {
            store(slot, g.clamp(-MAX_GAIN_DB, MAX_GAIN_DB));
        }
        self.touch();
    }

    /// Applies a named preset (case-insensitive). Returns false if there's no such preset.
    pub fn apply_preset(&self, name: &str) -> bool {
        match PRESETS.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)) {
            Some((_, gains)) => {
                self.set_gains_db(gains);
                true
            }
            None => false,
        }
    }
}

#[derive(Clone, Copy, Debug)]
struct Coeffs {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coeffs {
    const IDENTITY: Coeffs = Coeffs { b0: 1.0, b1: 0.0, b2: 0.0, a1: 0.0, a2: 0.0 };

    /// RBJ audio-EQ-cookbook peaking filter.
    fn peaking(freq: f64, gain_db: f64, q: f64, sample_rate: f64) -> Coeffs {
        let a = 10f64.powf(gain_db / 40.0);
        let w0 = 2.0 * PI * freq / sample_rate;
        let alpha = w0.sin() / (2.0 * q);
        let cos = w0.cos();
        let a0 = 1.0 + alpha / a;
        Coeffs {
            b0: (1.0 + alpha * a) / a0,
            b1: (-2.0 * cos) / a0,
            b2: (1.0 - alpha * a) / a0,
            a1: (-2.0 * cos) / a0,
            a2: (1.0 - alpha / a) / a0,
        }
    }

    /// |H(e^jw)| in dB.
    fn response_db(&self, freq: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * freq / sample_rate;
        let (c1, s1, c2, s2) = (w.cos(), w.sin(), (2.0 * w).cos(), (2.0 * w).sin());
        let num_re = self.b0 + self.b1 * c1 + self.b2 * c2;
        let num_im = -(self.b1 * s1 + self.b2 * s2);
        let den_re = 1.0 + self.a1 * c1 + self.a2 * c2;
        let den_im = -(self.a1 * s1 + self.a2 * s2);
        10.0 * ((num_re * num_re + num_im * num_im) / (den_re * den_re + den_im * den_im)).log10()
    }
}

/// Overall response of the EQ at `freq`, in dB, including the preamp. The UI uses this to draw the
/// curve; it's computed from the same coefficients the audio thread runs.
pub fn response_db(gains_db: &[f32; BAND_COUNT], preamp_db: f32, freq: f32, sample_rate: f32) -> f32 {
    let mut total = preamp_db as f64;
    for (band, &gain) in gains_db.iter().enumerate() {
        let c = Coeffs::peaking(band_freq(band, sample_rate as f64), gain as f64, BAND_Q, sample_rate as f64);
        total += c.response_db(freq as f64, sample_rate as f64);
    }
    total as f32
}

/// Centre frequency of a band, kept below Nyquist for low sample rates.
fn band_freq(band: usize, sample_rate: f64) -> f64 {
    (BAND_FREQS[band] as f64).min(sample_rate * 0.45)
}

#[derive(Clone, Copy, Default)]
struct State {
    z1: f64,
    z2: f64,
}

/// The audio-thread half. Processes interleaved stereo in place.
pub struct Equalizer {
    params: Arc<EqParams>,
    sample_rate: f64,
    seen_version: Option<u32>,
    coeffs: [Coeffs; BAND_COUNT],
    /// Bands at 0 dB are skipped entirely, which also keeps a flat EQ bit-exact.
    active: [bool; BAND_COUNT],
    state: [[State; 2]; BAND_COUNT],
    preamp: f64,
    /// 0 = fully bypassed, 1 = fully applied. Moves gradually so toggling doesn't click.
    wet: f32,
    wet_target: f32,
}

impl Equalizer {
    pub fn new(params: Arc<EqParams>, sample_rate: u32) -> Self {
        Self {
            params,
            sample_rate: sample_rate as f64,
            seen_version: None,
            coeffs: [Coeffs::IDENTITY; BAND_COUNT],
            active: [false; BAND_COUNT],
            state: [[State::default(); 2]; BAND_COUNT],
            preamp: 1.0,
            wet: 1.0,
            wet_target: 1.0,
        }
    }

    fn refresh(&mut self) {
        let version = self.params.version.load(Ordering::Acquire);
        if self.seen_version == Some(version) {
            return;
        }
        self.seen_version = Some(version);
        self.wet_target = if self.params.enabled() { 1.0 } else { 0.0 };
        self.preamp = 10f64.powf(self.params.preamp_db() as f64 / 20.0);
        for band in 0..BAND_COUNT {
            let gain = self.params.band_db(band) as f64;
            self.active[band] = gain.abs() > 0.01;
            self.coeffs[band] = if self.active[band] {
                Coeffs::peaking(band_freq(band, self.sample_rate), gain, BAND_Q, self.sample_rate)
            } else {
                Coeffs::IDENTITY
            };
        }
    }

    pub fn process(&mut self, buf: &mut [f32]) {
        self.refresh();
        if self.wet == 0.0 && self.wet_target == 0.0 {
            // Bypassed: leave the audio alone and forget filter history so re-enabling starts clean.
            self.state = [[State::default(); 2]; BAND_COUNT];
            return;
        }
        // Reach full wet/dry in about 10 ms.
        let step = 1.0 / (0.010 * self.sample_rate as f32);
        for frame in buf.chunks_exact_mut(2) {
            let dry = [frame[0], frame[1]];
            let mut wet = [dry[0] as f64 * self.preamp, dry[1] as f64 * self.preamp];
            for band in 0..BAND_COUNT {
                if !self.active[band] {
                    continue;
                }
                let c = self.coeffs[band];
                for (ch, x) in wet.iter_mut().enumerate() {
                    let s = &mut self.state[band][ch];
                    let y = c.b0 * *x + s.z1;
                    s.z1 = c.b1 * *x - c.a1 * y + s.z2;
                    s.z2 = c.b2 * *x - c.a2 * y;
                    *x = y;
                }
            }
            self.wet += (self.wet_target - self.wet).clamp(-step, step);
            for ch in 0..2 {
                frame[ch] = if self.wet >= 1.0 {
                    wet[ch] as f32
                } else {
                    dry[ch] + (wet[ch] as f32 - dry[ch]) * self.wet
                };
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FS: u32 = 48_000;

    fn sine_stereo(freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = 0.25 * (2.0 * std::f32::consts::PI * freq * i as f32 / FS as f32).sin();
                [v, v]
            })
            .collect()
    }

    /// Sine amplitude of the second half of a stereo buffer (the first half is settling time),
    /// measured by RMS. Sample peaks under-read high frequencies, where few samples fall near a crest.
    fn steady_amplitude(buf: &[f32]) -> f32 {
        let tail = &buf[buf.len() / 2..];
        (tail.iter().map(|v| v * v).sum::<f32>() / tail.len() as f32).sqrt() * std::f32::consts::SQRT_2
    }

    fn run(params: &Arc<EqParams>, freq: f32) -> f32 {
        let mut eq = Equalizer::new(params.clone(), FS);
        let mut buf = sine_stereo(freq, 48_000);
        eq.process(&mut buf);
        steady_amplitude(&buf) / 0.25
    }

    #[test]
    fn flat_eq_is_bit_exact() {
        let params = EqParams::new();
        let mut eq = Equalizer::new(params, FS);
        let original = sine_stereo(997.0, 4800);
        let mut buf = original.clone();
        eq.process(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn boost_at_a_band_centre_gives_that_gain() {
        let params = EqParams::new();
        params.set_band_db(5, 6.0); // 1 kHz
        let gain = run(&params, 1000.0);
        let db = 20.0 * gain.log10();
        assert!((db - 6.0).abs() < 0.1, "expected +6 dB at 1 kHz, measured {db:.2} dB");
    }

    #[test]
    fn cut_at_a_band_centre_gives_that_gain() {
        let params = EqParams::new();
        params.set_band_db(3, -9.0); // 250 Hz
        let db = 20.0 * run(&params, 250.0).log10();
        assert!((db + 9.0).abs() < 0.15, "expected -9 dB at 250 Hz, measured {db:.2} dB");
    }

    #[test]
    fn a_boost_leaves_distant_frequencies_nearly_alone() {
        let params = EqParams::new();
        params.set_band_db(2, 12.0); // 125 Hz
        let db = 20.0 * run(&params, 8000.0).log10();
        assert!(db.abs() < 0.2, "8 kHz moved by {db:.2} dB when only 125 Hz was boosted");
    }

    #[test]
    fn measured_response_matches_the_formula_the_ui_draws() {
        let params = EqParams::new();
        let gains = [3.0, -4.0, 5.0, 0.0, 2.0, -6.0, 4.0, 1.0, -3.0, 6.0];
        params.set_gains_db(&gains);
        params.set_preamp_db(-2.0);
        for freq in [80.0, 440.0, 1500.0, 3000.0, 9000.0] {
            let measured = 20.0 * run(&params, freq).log10();
            let predicted = response_db(&gains, -2.0, freq, FS as f32);
            assert!(
                (measured - predicted).abs() < 0.15,
                "{freq} Hz: measured {measured:.2} dB, predicted {predicted:.2} dB"
            );
        }
    }

    #[test]
    fn preamp_scales_everything() {
        let params = EqParams::new();
        params.set_preamp_db(-6.0);
        let gain = run(&params, 1000.0);
        assert!((gain - 0.5012).abs() < 0.005, "preamp -6 dB gave gain {gain}");
    }

    #[test]
    fn disabled_eq_passes_audio_through_after_the_fade() {
        let params = EqParams::new();
        params.set_band_db(5, 12.0);
        params.set_enabled(false);
        let mut eq = Equalizer::new(params, FS);
        let original = sine_stereo(1000.0, 9600);
        let mut buf = original.clone();
        eq.process(&mut buf);
        // wet/dry ramps over 10 ms (480 frames); after that the output is the dry signal exactly.
        assert_eq!(buf[2 * 600..], original[2 * 600..]);
    }

    #[test]
    fn toggling_does_not_produce_a_click() {
        let params = EqParams::new();
        params.set_band_db(5, 12.0);
        let mut eq = Equalizer::new(params.clone(), FS);
        let mut buf = sine_stereo(1000.0, 4800);
        eq.process(&mut buf);
        let mut more = sine_stereo(1000.0, 4800);
        params.set_enabled(false);
        eq.process(&mut more);
        // Largest sample-to-sample step across the whole stream, including the toggle point.
        // A 1 kHz sine peaking near 1.0 moves at most ~0.13 per sample at 48 kHz; a hard switch
        // between the boosted and dry signal would jump by far more than that.
        let all: Vec<f32> = buf.iter().chain(more.iter()).step_by(2).copied().collect();
        let max_step = all.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(max_step < 0.3, "discontinuity of {max_step} when disabling the EQ");
    }

    #[test]
    fn unknown_preset_is_rejected_and_known_one_applies() {
        let params = EqParams::new();
        assert!(!params.apply_preset("nonsense"));
        assert!(params.apply_preset("bass boost"));
        assert!(params.band_db(0) > 0.0);
        assert!(params.apply_preset("Flat"));
        assert!(params.gains_db().iter().all(|g| *g == 0.0));
    }

    #[test]
    fn gains_are_clamped() {
        let params = EqParams::new();
        params.set_band_db(0, 99.0);
        params.set_preamp_db(-99.0);
        params.set_band_db(42, 5.0); // out of range: ignored, must not panic
        assert_eq!(params.band_db(0), MAX_GAIN_DB);
        assert_eq!(params.preamp_db(), -MAX_GAIN_DB);
    }
}
