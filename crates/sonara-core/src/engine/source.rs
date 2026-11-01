//! A track on disk, decoded on its own thread and handed to the mixer through a lock-free ring.
//!
//! The decoder thread does all the slow work (file I/O, decoding, channel downmix, resampling) and
//! stays a fraction of a second ahead of playback; the audio thread only copies samples out of the
//! ring. When the source is dropped the thread notices and exits.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Sender;
use ringbuf::traits::{Consumer, Observer, Producer, Split};
use ringbuf::{HeapCons, HeapRb};

use super::mixer::FrameSource;
use crate::decode::{self, Decoder, StreamInfo};
use crate::dsp::resample::StreamResampler;
use crate::error::Result;

/// How much decoded audio the ring can hold, in seconds. Sources are created for the current
/// track and the next one, so this is also the memory cost of gapless playback.
const RING_SECS: f64 = 2.0;
/// Frames decoded before the thread checks whether it should stop.
const MAX_BACKOFF: Duration = Duration::from_millis(5);

/// Facts about an opened source, at the mixer's sample rate where relevant.
#[derive(Debug, Clone)]
pub struct SourceInfo {
    pub codec: &'static str,
    pub source_rate: u32,
    pub source_channels: usize,
    pub bits_per_sample: Option<u32>,
    /// Length in frames at the output rate.
    pub total_frames: Option<u64>,
    /// Where this source starts, in frames at the output rate (non-zero after a seek).
    pub start_frame: u64,
}

struct Flags {
    /// The decoder thread has pushed everything it ever will.
    finished: AtomicBool,
    /// The consumer is gone; the decoder thread should stop.
    cancelled: AtomicBool,
}

pub struct DecodedSource {
    cons: HeapCons<f32>,
    flags: Arc<Flags>,
}

impl DecodedSource {
    /// Frames currently buffered and ready to play.
    pub fn buffered_frames(&self) -> usize {
        self.cons.occupied_len() / 2
    }

    /// True once there's enough buffered to start without an immediate underrun, or the whole
    /// stream is already in the buffer.
    pub fn is_ready(&self, min_frames: usize) -> bool {
        self.buffered_frames() >= min_frames || self.flags.finished.load(Ordering::Acquire)
    }
}

impl FrameSource for DecodedSource {
    fn read(&mut self, out: &mut [f32]) -> (usize, bool) {
        // Read the flag *before* the ring: if it was already set, everything is in the ring, so
        // running short means the stream is truly over. The other order could miss the last samples.
        let finished = self.flags.finished.load(Ordering::Acquire);
        let n = self.cons.pop_slice(out);
        debug_assert!(n % 2 == 0, "ring must hold whole frames");
        (n / 2, finished && n < out.len())
    }
}

impl Drop for DecodedSource {
    fn drop(&mut self) {
        self.flags.cancelled.store(true, Ordering::Release);
    }
}

/// Opens `path` for playback at `out_rate`, starting `start_secs` into the track.
///
/// Errors found immediately (missing file, unsupported format) are returned. Errors that happen
/// later, mid-decode, end the stream early and are reported on `errors` as `(error_tag, message)`.
pub fn open_source(
    path: &Path,
    out_rate: u32,
    start_secs: f64,
    error_tag: u64,
    errors: Sender<(u64, String)>,
) -> Result<(DecodedSource, SourceInfo)> {
    let mut decoder = decode::open(path)?;
    let info = decoder.info().clone();
    if start_secs > 0.0 {
        decoder.seek(start_secs)?;
    }
    let resampler = StreamResampler::new(info.sample_rate, out_rate)?;

    let capacity = (out_rate as f64 * RING_SECS) as usize * 2;
    let (prod, cons) = HeapRb::<f32>::new(capacity).split();
    let flags = Arc::new(Flags { finished: AtomicBool::new(false), cancelled: AtomicBool::new(false) });

    let to_out = |frames: u64| (frames as u128 * out_rate as u128 + info.sample_rate as u128 / 2) as u64 / info.sample_rate as u64;
    let source_info = SourceInfo {
        codec: info.codec,
        source_rate: info.sample_rate,
        source_channels: info.channels,
        bits_per_sample: info.bits_per_sample,
        total_frames: info.total_frames.map(to_out),
        start_frame: (start_secs.max(0.0) * out_rate as f64).round() as u64,
    };

    let thread_flags = flags.clone();
    thread::Builder::new()
        .name("sonara-decode".into())
        .spawn(move || {
            if let Err(e) = decode_loop(decoder, info, resampler, prod, &thread_flags) {
                let _ = errors.send((error_tag, e.to_string()));
            }
            thread_flags.finished.store(true, Ordering::Release);
        })
        .map_err(crate::error::Error::Io)?;

    Ok((DecodedSource { cons, flags }, source_info))
}

fn decode_loop(
    mut decoder: Box<dyn Decoder>,
    info: StreamInfo,
    mut resampler: StreamResampler,
    mut prod: ringbuf::HeapProd<f32>,
    flags: &Flags,
) -> Result<()> {
    let mut raw = Vec::new();
    let mut stereo = Vec::new();
    let mut resampled = Vec::new();
    let mut more = true;
    while more && !flags.cancelled.load(Ordering::Acquire) {
        raw.clear();
        more = decoder.read(&mut raw)?;
        if raw.is_empty() {
            continue;
        }
        to_stereo(&raw, info.channels, &mut stereo);
        resampled.clear();
        resampler.push(&stereo, &mut resampled)?;
        if !push_all(&mut prod, &resampled, flags) {
            return Ok(());
        }
    }
    resampled.clear();
    resampler.finish(&mut resampled)?;
    push_all(&mut prod, &resampled, flags);
    Ok(())
}

/// Writes everything to the ring, waiting for the consumer to make room. Returns false if the
/// source was dropped while waiting.
fn push_all(prod: &mut ringbuf::HeapProd<f32>, mut data: &[f32], flags: &Flags) -> bool {
    let mut backoff = Duration::from_micros(500);
    while !data.is_empty() {
        let n = prod.push_slice(data);
        data = &data[n..];
        if n == 0 {
            if flags.cancelled.load(Ordering::Acquire) {
                return false;
            }
            thread::sleep(backoff);
            backoff = (backoff * 2).min(MAX_BACKOFF);
        } else {
            backoff = Duration::from_micros(500);
        }
    }
    true
}

/// Converts interleaved audio with `channels` channels to interleaved stereo.
///
/// Mono is duplicated. Layouts with more than two channels are assumed to be in the usual
/// L R C LFE Ls Rs (Lb Rb) order and folded down with the standard -3 dB centre and surround
/// weights, normalised so a full-scale signal can't clip. The LFE channel is dropped.
pub fn to_stereo(input: &[f32], channels: usize, out: &mut Vec<f32>) {
    out.clear();
    match channels {
        0 => {}
        1 => out.extend(input.iter().flat_map(|&s| [s, s])),
        2 => out.extend_from_slice(input),
        n => {
            const C: f32 = std::f32::consts::FRAC_1_SQRT_2;
            // (index, weight) contributions to left and right.
            let (left, right): (Vec<(usize, f32)>, Vec<(usize, f32)>) = match n {
                3 => (vec![(0, 1.0), (2, C)], vec![(1, 1.0), (2, C)]),
                4 => (vec![(0, 1.0), (2, C)], vec![(1, 1.0), (3, C)]),
                5 => (vec![(0, 1.0), (2, C), (3, C)], vec![(1, 1.0), (2, C), (4, C)]),
                6 => (vec![(0, 1.0), (2, C), (4, C)], vec![(1, 1.0), (2, C), (5, C)]),
                _ => (
                    vec![(0, 1.0), (2, C), (4, C), (6, C)],
                    vec![(1, 1.0), (2, C), (5, C), (7.min(n - 1), C)],
                ),
            };
            let norm = |side: &[(usize, f32)]| 1.0 / side.iter().map(|w| w.1).sum::<f32>();
            let (nl, nr) = (norm(&left), norm(&right));
            for frame in input.chunks_exact(n) {
                let l: f32 = left.iter().filter(|(i, _)| *i < n).map(|&(i, w)| frame[i] * w).sum();
                let r: f32 = right.iter().filter(|(i, _)| *i < n).map(|&(i, w)| frame[i] * w).sum();
                out.push(l * nl);
                out.push(r * nr);
            }
        }
    }
}

/// Blocks until `source` has `min_frames` buffered (or has finished), up to `timeout`.
/// Returns whether it became ready.
pub fn wait_ready(source: &DecodedSource, min_frames: usize, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while !source.is_ready(min_frames) {
        if Instant::now() >= deadline {
            return false;
        }
        thread::sleep(Duration::from_millis(1));
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    fn drain(src: &mut DecodedSource) -> Vec<f32> {
        let mut all = Vec::new();
        let mut buf = vec![0.0; 1000];
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let (n, ended) = src.read(&mut buf);
            all.extend_from_slice(&buf[..n * 2]);
            if ended {
                return all;
            }
            assert!(Instant::now() < deadline, "source never ended");
            if n == 0 {
                thread::sleep(Duration::from_millis(1));
            }
        }
    }

    fn open(name: &str, rate: u32, start: f64) -> (DecodedSource, SourceInfo, crossbeam_channel::Receiver<(u64, String)>) {
        let (tx, rx) = crossbeam_channel::unbounded();
        let (s, i) = open_source(&fixture(name), rate, start, 1, tx).unwrap();
        (s, i, rx)
    }

    #[test]
    fn same_rate_file_comes_through_whole() {
        let (mut src, info, _) = open("chirp_44k_stereo.flac", 44_100, 0.0);
        assert_eq!((info.codec, info.source_rate, info.total_frames), ("flac", 44_100, Some(132_300)));
        assert_eq!(drain(&mut src).len() / 2, 132_300);
    }

    #[test]
    fn resampled_length_matches_the_reported_length() {
        for (file, rate) in [("chirp_44k_stereo.flac", 48_000), ("chirp_48k_stereo.opus", 44_100), ("tone_96k_24bit_mono.flac", 48_000), ("chirp_44k_stereo.mp3", 96_000)] {
            let (mut src, info, errors) = open(file, rate, 0.0);
            let frames = drain(&mut src).len() / 2;
            assert_eq!(Some(frames as u64), info.total_frames, "{file} at {rate}");
            assert!(errors.try_recv().is_err());
        }
    }

    #[test]
    fn starting_part_way_in_reports_the_offset_and_plays_the_rest() {
        let (mut src, info, _) = open("chirp_44k_stereo.flac", 48_000, 1.0);
        assert_eq!(info.start_frame, 48_000);
        let frames = drain(&mut src).len() as u64 / 2;
        let total = info.total_frames.unwrap();
        assert!(frames.abs_diff(total - 48_000) <= 2, "{frames} frames left of {total}");
    }

    #[test]
    fn mono_becomes_identical_stereo() {
        let (mut src, _, _) = open("tone_48k_mono.opus", 48_000, 0.0);
        let pcm = drain(&mut src);
        assert!(pcm.chunks_exact(2).all(|f| f[0] == f[1]));
        assert!(pcm.iter().any(|&v| v.abs() > 0.3));
    }

    #[test]
    fn becomes_ready_and_buffers_ahead_of_playback() {
        let (src, _, _) = open("chirp_44k_stereo.flac", 48_000, 0.0);
        assert!(wait_ready(&src, 4800, Duration::from_secs(5)));
        assert!(src.buffered_frames() >= 4800);
    }

    #[test]
    fn dropping_the_source_stops_the_decoder_thread() {
        // A long source that would fill the ring and then wait. After the drop the thread must
        // notice and exit; we can only observe that indirectly, so check it doesn't hang the test
        // process and that the flag is set.
        let (src, _, _) = open("chirp_44k_stereo.flac", 44_100, 0.0);
        let flags = src.flags.clone();
        drop(src);
        assert!(flags.cancelled.load(Ordering::Acquire));
    }

    #[test]
    fn errors_at_open_are_returned() {
        let (tx, _rx) = crossbeam_channel::unbounded();
        assert!(open_source(Path::new("/nonexistent/file.flac"), 48_000, 0.0, 1, tx).is_err());
    }

    #[test]
    fn downmix_layouts() {
        let mut out = Vec::new();
        to_stereo(&[0.5, -0.5], 1, &mut out);
        assert_eq!(out, [0.5, 0.5, -0.5, -0.5]);

        to_stereo(&[0.1, 0.2, 0.3, 0.4], 2, &mut out);
        assert_eq!(out, [0.1, 0.2, 0.3, 0.4]);

        // 5.1: only front-left is active -> only left output, and at unity after normalisation
        // of the (1.0, C, C) weights it is scaled down but never above full scale.
        to_stereo(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0], 6, &mut out);
        assert!(out[0] > 0.3 && out[0] <= 1.0 && out[1] == 0.0, "{out:?}");
        // Everything at full scale (except LFE) must stay at or below full scale.
        to_stereo(&[1.0, 1.0, 1.0, 1.0, 1.0, 1.0], 6, &mut out);
        assert!((out[0] - 1.0).abs() < 1e-5 && (out[1] - 1.0).abs() < 1e-5, "{out:?}");
        // The centre channel lands equally on both sides.
        to_stereo(&[0.0, 0.0, 1.0, 0.0, 0.0, 0.0], 6, &mut out);
        assert!(out[0] > 0.0 && (out[0] - out[1]).abs() < 1e-6);
        // 7.1 and an odd layout shouldn't panic.
        to_stereo(&[0.1; 8 * 4], 8, &mut out);
        assert_eq!(out.len(), 8);
        to_stereo(&[0.1; 9 * 2], 9, &mut out);
        assert_eq!(out.len(), 4);
    }
}
