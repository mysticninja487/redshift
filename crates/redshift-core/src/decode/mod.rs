//! Turning audio files into f32 PCM.
//!
//! Symphonia handles FLAC, MP3, WAV, Ogg Vorbis, AAC and ALAC. It has no Opus codec, so Ogg Opus
//! goes through libopus (via `audiopus`) with the `ogg` crate doing the demuxing. [`open`] looks at
//! the file's contents, not just its extension, because `.ogg` can hold either Vorbis or Opus.

mod opus;
mod symphonia_dec;

use std::fs::File;
use std::io::Read;
use std::path::Path;

use crate::error::Result;

pub use opus::OpusFileDecoder;
pub use symphonia_dec::SymphoniaDecoder;

/// What a decoder produces. Sample rate and channel count describe the PCM that `read` returns,
/// which for Opus is always 48 kHz regardless of what the file was recorded at.
#[derive(Debug, Clone)]
pub struct StreamInfo {
    pub sample_rate: u32,
    pub channels: usize,
    /// Length in frames at `sample_rate`, if the container says. `None` for streams of unknown length.
    pub total_frames: Option<u64>,
    pub codec: &'static str,
    /// Bit depth of the source, when it has one (lossless formats). Lossy codecs report `None`.
    pub bits_per_sample: Option<u32>,
}

impl StreamInfo {
    pub fn duration_secs(&self) -> Option<f64> {
        self.total_frames.map(|f| f as f64 / self.sample_rate as f64)
    }
}

pub trait Decoder: Send {
    fn info(&self) -> &StreamInfo;

    /// Decodes the next chunk and appends it to `out` as interleaved f32 in the source channel
    /// layout. Returns `Ok(false)` once the stream has ended. A call may append nothing and still
    /// return `Ok(true)` (a skipped corrupt packet, or frames discarded after a seek), so callers
    /// should loop rather than treat an empty append as the end.
    fn read(&mut self, out: &mut Vec<f32>) -> Result<bool>;

    /// Repositions so the next frames returned start at `seconds`. Seeking past the end is allowed
    /// and makes the next `read` return `Ok(false)`.
    fn seek(&mut self, seconds: f64) -> Result<()>;
}

/// Opens `path` with whichever decoder fits its contents.
pub fn open(path: &Path) -> Result<Box<dyn Decoder>> {
    if is_ogg_opus(path)? {
        Ok(Box::new(OpusFileDecoder::open(path)?))
    } else {
        Ok(Box::new(SymphoniaDecoder::open(path)?))
    }
}

/// True if the file starts with an Ogg page whose first packet is an `OpusHead`.
fn is_ogg_opus(path: &Path) -> Result<bool> {
    let mut head = Vec::with_capacity(320);
    File::open(path)?.take(320).read_to_end(&mut head)?;
    if head.len() < 28 || &head[..4] != b"OggS" {
        return Ok(false);
    }
    // Ogg page header is 27 bytes, followed by one lacing byte per segment, then the packet data.
    let segments = head[26] as usize;
    let data = 27 + segments;
    Ok(head.get(data..data + 8) == Some(b"OpusHead".as_slice()))
}
