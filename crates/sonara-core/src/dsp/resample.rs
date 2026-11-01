//! Streaming sample-rate conversion for interleaved stereo, built on rubato's FFT resampler.
//!
//! The resampler works in fixed-size chunks and has some start-up delay. This wrapper hides both:
//! feed it any amount of audio with [`StreamResampler::push`], then call [`StreamResampler::finish`]
//! once at the end. The total output is exactly `round(frames_in * ratio)` frames, time-aligned
//! with the input.

use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Indexing, Resampler};

use crate::error::{Error, Result};

const CHANNELS: usize = 2;
const CHUNK: usize = 1024;

pub struct StreamResampler {
    inner: Option<Inner>,
}

struct Inner {
    resampler: Fft<f32>,
    in_rate: u32,
    out_rate: u32,
    /// Input not yet consumed (interleaved).
    pending: Vec<f32>,
    out_chunk: Vec<f32>,
    /// Output frames still to be discarded (the resampler's start-up delay).
    skip: usize,
    frames_in: u64,
    frames_out: u64,
}

impl StreamResampler {
    /// Same rates make a pass-through that copies the audio unchanged.
    pub fn new(in_rate: u32, out_rate: u32) -> Result<Self> {
        if in_rate == out_rate {
            return Ok(Self { inner: None });
        }
        let resampler = Fft::<f32>::new(
            in_rate as usize,
            out_rate as usize,
            CHUNK,
            CHANNELS,
            FixedSync::Input,
        )
        .map_err(|e| Error::Other(format!("creating resampler {in_rate}->{out_rate}: {e}")))?;
        let skip = resampler.output_delay();
        let out_chunk = vec![0.0; resampler.output_frames_max() * CHANNELS];
        Ok(Self {
            inner: Some(Inner {
                resampler,
                in_rate,
                out_rate,
                pending: Vec::new(),
                out_chunk,
                skip,
                frames_in: 0,
                frames_out: 0,
            }),
        })
    }

    /// Appends the resampled audio for `input` (interleaved stereo) to `out`. Some of it may be held
    /// back until more input arrives.
    pub fn push(&mut self, input: &[f32], out: &mut Vec<f32>) -> Result<()> {
        match &mut self.inner {
            None => out.extend_from_slice(input),
            Some(inner) => {
                inner.frames_in += (input.len() / CHANNELS) as u64;
                inner.pending.extend_from_slice(input);
                inner.run(out, false)?;
            }
        }
        Ok(())
    }

    /// Flushes the tail. Call once after the last `push`.
    pub fn finish(&mut self, out: &mut Vec<f32>) -> Result<()> {
        if let Some(inner) = &mut self.inner {
            inner.run(out, true)?;
        }
        Ok(())
    }
}

impl Inner {
    fn expected_total(&self) -> u64 {
        (self.frames_in as u128 * self.out_rate as u128 + self.in_rate as u128 / 2) as u64
            / self.in_rate as u64
    }

    fn run(&mut self, out: &mut Vec<f32>, flush: bool) -> Result<()> {
        loop {
            let total = self.expected_total();
            if flush && self.frames_out >= total {
                self.pending.clear();
                return Ok(());
            }
            let need = self.resampler.input_frames_next();
            let have = self.pending.len() / CHANNELS;
            // `valid` is how many of the `need` input frames are real audio. When flushing with too
            // little input left, the rest is zero-padded: those silent chunks push the resampler's
            // delayed tail out.
            let valid = if have >= need {
                need
            } else if flush {
                have
            } else {
                return Ok(());
            };
            self.pending.resize(need.max(have) * CHANNELS, 0.0);

            let out_frames = self.resampler.output_frames_next();
            let input = InterleavedSlice::new(&self.pending[..need * CHANNELS], CHANNELS, need)
                .map_err(|e| Error::Other(format!("resampler input: {e}")))?;
            let mut output = InterleavedSlice::new_mut(&mut self.out_chunk, CHANNELS, out_frames)
                .map_err(|e| Error::Other(format!("resampler output: {e}")))?;
            let indexing = Indexing {
                input_offset: 0,
                output_offset: 0,
                partial_len: (valid < need).then_some(valid),
                active_channels_mask: None,
            };
            let (_, produced) = self
                .resampler
                .process_into_buffer(&input, &mut output, Some(&indexing))
                .map_err(|e| Error::Other(format!("resampling: {e}")))?;
            self.pending.drain(..valid * CHANNELS);

            // Drop the start-up delay, and never emit more than the true length.
            let start = self.skip.min(produced);
            self.skip -= start;
            let room = total.saturating_sub(self.frames_out) as usize;
            let end = produced.min(start + room);
            if start < end {
                out.extend_from_slice(&self.out_chunk[start * CHANNELS..end * CHANNELS]);
                self.frames_out += (end - start) as u64;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, freq: f32, frames: usize) -> Vec<f32> {
        (0..frames)
            .flat_map(|i| {
                let v = 0.5 * (2.0 * std::f32::consts::PI * freq * i as f32 / rate as f32).sin();
                [v, -v]
            })
            .collect()
    }

    /// Feeds `input` in irregular pieces, as a decoder would.
    fn resample_all(input: &[f32], in_rate: u32, out_rate: u32) -> Vec<f32> {
        let mut r = StreamResampler::new(in_rate, out_rate).unwrap();
        let mut out = Vec::new();
        let mut pos = 0;
        let mut size = 700;
        while pos < input.len() {
            let end = (pos + size * 2).min(input.len());
            r.push(&input[pos..end], &mut out).unwrap();
            pos = end;
            size = size * 3 % 4001 + 100;
        }
        r.finish(&mut out).unwrap();
        out
    }

    /// Phase-independent amplitude estimate of a sine over a window of one channel.
    fn amplitude(out: &[f32], from: usize, to: usize) -> f32 {
        let w: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).skip(from).take(to - from).collect();
        (w.iter().map(|v| v * v).sum::<f32>() / w.len() as f32).sqrt() * std::f32::consts::SQRT_2
    }

    #[test]
    fn same_rate_is_a_copy() {
        let input = sine(44_100, 440.0, 5000);
        assert_eq!(resample_all(&input, 44_100, 44_100), input);
    }

    #[test]
    fn output_length_is_exact_for_common_rate_pairs() {
        for (a, b) in [(44_100, 48_000), (48_000, 44_100), (96_000, 48_000), (44_100, 96_000), (22_050, 48_000)] {
            for frames in [1usize, 17, 1023, 1024, 1025, 44_100, 50_001] {
                let out = resample_all(&sine(a, 440.0, frames), a, b);
                let expected = ((frames as u128 * b as u128 + a as u128 / 2) / a as u128) as usize;
                assert_eq!(out.len() / 2, expected, "{a}->{b} with {frames} input frames");
            }
        }
    }

    #[test]
    fn tone_keeps_frequency_amplitude_and_channel_sign() {
        let out = resample_all(&sine(44_100, 1000.0, 44_100), 44_100, 48_000);
        let frames = out.len() / 2;
        // Mid-stream amplitude is preserved.
        let amp = amplitude(&out, 8000, frames - 8000);
        assert!((amp - 0.5).abs() < 0.01, "amplitude {amp}");
        // The second channel is the inverse of the first.
        for f in out.chunks_exact(2).skip(5000).take(2000) {
            assert!((f[0] + f[1]).abs() < 1e-3);
        }
        // Zero-crossing count: 1 kHz for 1 s should cross zero ~2000 times.
        let left: Vec<f32> = out.chunks_exact(2).map(|f| f[0]).collect();
        let crossings = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        assert!((crossings as i32 - 1000).abs() <= 2, "{crossings} rising crossings in 1 s");
    }

    #[test]
    fn output_is_time_aligned_with_input() {
        // An impulse at frame 10_000 of 44.1 kHz should appear at about frame 10_884 at 48 kHz.
        let mut input = vec![0.0f32; 2 * 20_000];
        input[2 * 10_000] = 1.0;
        input[2 * 10_000 + 1] = 1.0;
        let out = resample_all(&input, 44_100, 48_000);
        let peak_at = out.chunks_exact(2).map(|f| f[0].abs()).enumerate()
            .max_by(|a, b| a.1.total_cmp(&b.1)).unwrap().0;
        let expected = (10_000.0 * 48_000.0 / 44_100.0f64).round() as i64;
        assert!((peak_at as i64 - expected).abs() <= 1, "impulse at {peak_at}, expected {expected}");
    }

    #[test]
    fn tail_is_not_truncated() {
        // A constant signal must still be at full level shortly before the end. If the delay were
        // mishandled, the final few hundred frames would be silence or cut off.
        let input = vec![0.5f32; 2 * 10_000];
        let out = resample_all(&input, 44_100, 48_000);
        let frames = out.len() / 2;
        let v = out[2 * (frames - 64)];
        assert!((v - 0.5).abs() < 0.1, "64 frames before the end the level is {v}");
    }
}
