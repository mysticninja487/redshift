//! A whole-track loudness outline for waveform seek bars.
//!
//! The file is decoded once, as fast as the decoder goes (typically dozens of times faster than
//! playback), keeping only the loudest sample and the energy of each short block. Those blocks
//! are then folded into the number of buckets the UI wants to draw.

use std::path::Path;

use serde::Serialize;

use crate::decode;
use crate::error::Result;

/// Frames per analysis block. Short enough that even a small bucket count keeps its detail.
const BLOCK_FRAMES: usize = 1024;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Overview {
    /// Loudest sample in each bucket, scaled so the loudest bucket in the track is 1.0.
    pub peaks: Vec<f32>,
    /// RMS level of each bucket, on the same scale as `peaks`.
    pub rms: Vec<f32>,
    pub duration_secs: f64,
}

/// Decodes `path` and returns `buckets` (clamped to 8..=4096) peak and RMS values.
pub fn overview(path: &Path, buckets: usize) -> Result<Overview> {
    let buckets = buckets.clamp(8, 4096);
    let mut dec = decode::open(path)?;
    let channels = dec.info().channels.max(1);
    let rate = dec.info().sample_rate as f64;

    // (peak, sum of squares, frames) per block.
    let mut blocks: Vec<(f32, f64, usize)> = Vec::new();
    let mut cur = (0f32, 0f64, 0usize);
    let mut pcm = Vec::new();
    let mut total_frames: u64 = 0;
    loop {
        pcm.clear();
        let more = dec.read(&mut pcm)?;
        for frame in pcm.chunks_exact(channels) {
            // A frame's level is its loudest channel, so a hard-panned track still shows.
            let level = frame.iter().fold(0f32, |m, s| m.max(s.abs()));
            cur.0 = cur.0.max(level);
            cur.1 += (level as f64) * (level as f64);
            cur.2 += 1;
            if cur.2 == BLOCK_FRAMES {
                blocks.push(cur);
                cur = (0.0, 0.0, 0);
            }
            total_frames += 1;
        }
        if !more {
            break;
        }
    }
    if cur.2 > 0 {
        blocks.push(cur);
    }

    let mut peaks = vec![0f32; buckets];
    let mut rms = vec![0f32; buckets];
    if !blocks.is_empty() {
        // Block i covers [i, i+1) of the track; bucket b covers [b, b+1) * blocks/buckets.
        let per = blocks.len() as f64 / buckets as f64;
        for b in 0..buckets {
            let lo = (b as f64 * per).floor() as usize;
            let hi = (((b + 1) as f64 * per).ceil() as usize).clamp(lo + 1, blocks.len());
            let lo = lo.min(blocks.len() - 1);
            let (mut p, mut sq, mut n) = (0f32, 0f64, 0usize);
            for blk in &blocks[lo..hi] {
                p = p.max(blk.0);
                sq += blk.1;
                n += blk.2;
            }
            peaks[b] = p;
            rms[b] = if n > 0 { (sq / n as f64).sqrt() as f32 } else { 0.0 };
        }
    }
    let top = peaks.iter().cloned().fold(0f32, f32::max);
    if top > 1e-6 {
        for v in peaks.iter_mut().chain(rms.iter_mut()) {
            *v = (*v / top).min(1.0);
        }
    }
    Ok(Overview { peaks, rms, duration_secs: total_frames as f64 / rate })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    /// A mono 16-bit WAV: `quiet_secs` of silence, then `loud_secs` of a square wave at `amp`.
    fn write_wav(path: &Path, rate: u32, quiet_secs: f32, loud_secs: f32, amp: f32) {
        let mut samples: Vec<i16> = vec![0; (rate as f32 * quiet_secs) as usize];
        for i in 0..(rate as f32 * loud_secs) as usize {
            let s = if (i / 20) % 2 == 0 { amp } else { -amp };
            samples.push((s * 32767.0) as i16);
        }
        let data_len = (samples.len() * 2) as u32;
        let mut b = Vec::new();
        b.extend(b"RIFF");
        b.extend((36 + data_len).to_le_bytes());
        b.extend(b"WAVEfmt ");
        b.extend(16u32.to_le_bytes());
        b.extend(1u16.to_le_bytes()); // PCM
        b.extend(1u16.to_le_bytes()); // mono
        b.extend(rate.to_le_bytes());
        b.extend((rate * 2).to_le_bytes());
        b.extend(2u16.to_le_bytes());
        b.extend(16u16.to_le_bytes());
        b.extend(b"data");
        b.extend(data_len.to_le_bytes());
        for s in samples {
            b.extend(s.to_le_bytes());
        }
        fs::write(path, b).unwrap();
    }

    #[test]
    fn shows_where_the_loud_part_is_and_scales_to_the_loudest() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("step.wav");
        write_wav(&p, 44_100, 1.0, 1.0, 0.25); // quiet absolute level: still scales to 1.0
        let o = overview(&p, 40).unwrap();
        assert_eq!((o.peaks.len(), o.rms.len()), (40, 40));
        assert!((o.duration_secs - 2.0).abs() < 0.01, "{}", o.duration_secs);
        assert!(o.peaks[..18].iter().all(|v| *v < 0.01), "first second is silent: {:?}", &o.peaks[..18]);
        assert!(o.peaks[22..].iter().all(|v| *v > 0.99), "second half is loud: {:?}", &o.peaks[22..]);
        assert!((o.peaks.iter().cloned().fold(0f32, f32::max) - 1.0).abs() < 1e-6);
        // A square wave's RMS equals its peak.
        assert!(o.rms[30] > 0.95, "{}", o.rms[30]);
        assert!(o.rms.iter().zip(&o.peaks).all(|(r, p)| *r <= *p + 1e-5));
    }

    #[test]
    fn silence_stays_silent_without_dividing_by_zero() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("silence.wav");
        write_wav(&p, 44_100, 1.0, 0.0, 0.0);
        let o = overview(&p, 16).unwrap();
        assert!(o.peaks.iter().chain(&o.rms).all(|v| *v == 0.0));
    }

    #[test]
    fn works_on_real_formats_and_more_buckets_than_blocks() {
        for name in ["tagged.flac", "tagged.opus", "chirp_44k_stereo.mp3"] {
            let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name);
            let o = overview(&p, 4096).unwrap();
            assert_eq!(o.peaks.len(), 4096, "{name}");
            assert!(o.peaks.iter().all(|v| v.is_finite() && (0.0..=1.0).contains(v)), "{name}");
            assert!(o.peaks.iter().cloned().fold(0f32, f32::max) > 0.99, "{name}");
            assert!(o.duration_secs > 0.5, "{name}: {}", o.duration_secs);
        }
        // A steady tone is about equally loud everywhere, apart from edges.
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tagged.flac");
        let o = overview(&p, 20).unwrap();
        assert!(o.peaks[2..18].iter().all(|v| *v > 0.9), "{:?}", o.peaks);
    }

    #[test]
    fn bucket_count_is_clamped_and_bad_files_error() {
        let p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/tagged.flac");
        assert_eq!(overview(&p, 0).unwrap().peaks.len(), 8);
        assert_eq!(overview(&p, 1_000_000).unwrap().peaks.len(), 4096);
        let d = tempfile::tempdir().unwrap();
        let junk = d.path().join("junk.flac");
        fs::write(&junk, vec![7u8; 4000]).unwrap();
        assert!(overview(&junk, 64).is_err());
        assert!(overview(&d.path().join("missing.flac"), 64).is_err());
    }
}
