#![allow(dead_code)]

use std::path::PathBuf;

use rustfft::{num_complex::Complex, FftPlanner};
use sonara_core::decode::{self, Decoder, StreamInfo};

pub fn fixture(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
}

/// Decodes everything the decoder will give, interleaved in the source layout.
pub fn read_all(dec: &mut dyn Decoder) -> Vec<f32> {
    let mut out = Vec::new();
    while dec.read(&mut out).expect("decode failed") {}
    out
}

pub fn decode_file(name: &str) -> (StreamInfo, Vec<f32>) {
    let mut dec = decode::open(&fixture(name)).expect("open failed");
    let info = dec.info().clone();
    let pcm = read_all(dec.as_mut());
    (info, pcm)
}

/// Reads until at least `frames` frames have been collected (or the stream ends).
pub fn read_frames(dec: &mut dyn Decoder, frames: usize, channels: usize) -> Vec<f32> {
    let mut out = Vec::new();
    while out.len() < frames * channels && dec.read(&mut out).expect("decode failed") {}
    out
}

pub fn channel(pcm: &[f32], channels: usize, ch: usize) -> Vec<f32> {
    pcm.chunks_exact(channels).map(|f| f[ch]).collect()
}

pub fn rms(x: &[f32]) -> f32 {
    (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt()
}

pub fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0, |m, v| m.max(v.abs()))
}

/// Strongest frequency in the first `n` samples of `x`, with parabolic interpolation between bins.
pub fn dominant_hz(x: &[f32], rate: u32, n: usize) -> f32 {
    let n = n.min(x.len());
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            Complex::new(x[i] * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let mag: Vec<f32> = buf[..n / 2].iter().map(|c| c.norm()).collect();
    let k = (1..mag.len() - 1).max_by(|&a, &b| mag[a].total_cmp(&mag[b])).unwrap();
    let (a, b, c) = (mag[k - 1], mag[k], mag[k + 1]);
    let shift = 0.5 * (a - c) / (a - 2.0 * b + c);
    (k as f32 + shift) * rate as f32 / n as f32
}

/// The test chirp is f(t) = 400 + 1000 t Hz. A window starting at `start_frame` and `len` long has
/// its energy centred on the window's midpoint, so that's where we expect the measured frequency.
pub fn chirp_hz_at_window(start_frame: usize, len: usize, rate: u32) -> f32 {
    let centre = (start_frame as f32 + len as f32 / 2.0) / rate as f32;
    400.0 + 1000.0 * centre
}

/// Largest FFT magnitude between `lo` and `hi` Hz in the first `n` samples of `x`, as a fraction of
/// what a full-scale sine would give. Tells whether energy is present around a frequency.
pub fn band_peak(x: &[f32], rate: u32, lo: f32, hi: f32, n: usize) -> f32 {
    let n = n.min(x.len());
    let mut buf: Vec<Complex<f32>> = (0..n)
        .map(|i| {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos();
            Complex::new(x[i] * w, 0.0)
        })
        .collect();
    FftPlanner::new().plan_fft_forward(n).process(&mut buf);
    let hz_per_bin = rate as f32 / n as f32;
    let wsum = n as f32 / 2.0;
    (0..n / 2)
        .filter(|&k| (k as f32 * hz_per_bin) >= lo && (k as f32 * hz_per_bin) <= hi)
        .map(|k| 2.0 * buf[k].norm() / wsum)
        .fold(0.0, f32::max)
}
