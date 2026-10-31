use std::fs::File;
use std::io::ErrorKind;
use std::path::Path;

use symphonia::core::codecs::audio::{AudioDecoder, AudioDecoderOptions};
use symphonia::core::errors::{Error as SymError, SeekErrorKind};
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp, TimeBase};

use super::{Decoder, StreamInfo};
use crate::error::{Error, Result};

pub struct SymphoniaDecoder {
    format: Box<dyn FormatReader>,
    decoder: Box<dyn AudioDecoder>,
    track_id: u32,
    time_base: Option<TimeBase>,
    info: StreamInfo,
    /// Set by `seek`: valid frames before this timestamp are dropped. Symphonia lands on a packet at
    /// or before the requested position, so the first packet or two usually straddle the target.
    seek_target: Option<Timestamp>,
    at_end: bool,
    scratch: Vec<f32>,
}

impl SymphoniaDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let file = File::open(path)?;
        let mss = MediaSourceStream::new(Box::new(file), Default::default());

        let mut hint = Hint::new();
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            hint.with_extension(ext);
        }

        let format = symphonia::default::get_probe()
            .probe(&hint, mss, FormatOptions::default(), MetadataOptions::default())
            .map_err(map_err)?;

        let track = format
            .default_track(TrackType::Audio)
            .ok_or_else(|| Error::Unsupported("file has no audio track".into()))?;
        let params = track
            .codec_params
            .as_ref()
            .and_then(|p| p.audio())
            .ok_or_else(|| Error::Unsupported("audio track has no codec parameters".into()))?;

        let sample_rate = params
            .sample_rate
            .ok_or_else(|| Error::Unsupported("unknown sample rate".into()))?;
        let channels = params
            .channels
            .as_ref()
            .map(|c| c.count())
            .ok_or_else(|| Error::Unsupported("unknown channel layout".into()))?;
        let track_id = track.id;
        let time_base = track.time_base;
        let total_frames = track.num_frames;
        let bits_per_sample = params.bits_per_sample;

        let decoder = symphonia::default::get_codecs()
            .make_audio_decoder(params, &AudioDecoderOptions::default())
            .map_err(map_err)?;
        let codec = decoder.codec_info().short_name;

        Ok(Self {
            format,
            decoder,
            track_id,
            time_base,
            info: StreamInfo { sample_rate, channels, total_frames, codec, bits_per_sample },
            seek_target: None,
            at_end: false,
            scratch: Vec::new(),
        })
    }
}

impl Decoder for SymphoniaDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if self.at_end {
            return Ok(false);
        }
        let packet = match self.format.next_packet() {
            Ok(Some(p)) => p,
            Ok(None) => return Ok(false),
            // Some demuxers signal the end of the file this way instead of returning `None`.
            Err(SymError::IoError(e)) if e.kind() == ErrorKind::UnexpectedEof => return Ok(false),
            Err(SymError::ResetRequired) => {
                return Err(Error::Decode("stream parameters changed mid-file".into()))
            }
            Err(e) => return Err(map_err(e)),
        };
        if packet.track_id != self.track_id {
            return Ok(true);
        }

        // Encoder delay and padding need no handling here: the decoders are created with
        // `gapless: true` (the default), so MP3 and Vorbis already trim the frames named by
        // `packet.trim_start` / `trim_end`. Trimming again would eat real audio.
        //
        // What does need handling is seeking. The first decoded buffer after a seek usually
        // starts a little before the target, and `pts` marks the start of the *untrimmed* packet,
        // so the trimmed buffer begins at `pts + trim_start`.
        let rate = self.info.sample_rate;
        let tb = self.time_base;
        let buffer_start_ts = packet.pts.get() + packet.trim_start.get() as i64;
        let seek_skip = match self.seek_target {
            Some(target) => ts_to_frames(tb, rate, (target.get() - buffer_start_ts).max(0) as u64),
            None => 0,
        };

        match self.decoder.decode(&packet) {
            Ok(buf) => {
                let channels = buf.spec().channels().count();
                if channels != self.info.channels {
                    return Err(Error::Decode("channel count changed mid-file".into()));
                }
                let frames = buf.frames();
                if seek_skip < frames {
                    // The target is inside this buffer (or already behind us): play from here on.
                    self.seek_target = None;
                    self.scratch.clear();
                    buf.copy_to_vec_interleaved::<f32>(&mut self.scratch);
                    out.extend_from_slice(&self.scratch[seek_skip * channels..frames * channels]);
                }
                Ok(true)
            }
            // A corrupt packet shouldn't end playback; skip it and carry on.
            Err(SymError::DecodeError(_)) => Ok(true),
            Err(e) => Err(map_err(e)),
        }
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let seconds = seconds.max(0.0);
        if self.info.duration_secs().is_some_and(|d| seconds >= d) {
            self.at_end = true;
            return Ok(());
        }
        let time = Time::try_from_secs_f64(seconds)
            .ok_or_else(|| Error::Seek(format!("invalid seek position {seconds}")))?;
        match self.format.seek(
            SeekMode::Accurate,
            SeekTo::Time { time, track_id: Some(self.track_id) },
        ) {
            Ok(seeked) => {
                self.decoder.reset();
                self.seek_target = Some(seeked.required_ts);
                self.at_end = false;
                Ok(())
            }
            Err(SymError::SeekError(SeekErrorKind::OutOfRange)) => {
                self.at_end = true;
                Ok(())
            }
            Err(SymError::SeekError(kind)) => Err(Error::Seek(format!("{kind:?}"))),
            Err(e) => Err(map_err(e)),
        }
    }
}

fn ts_to_frames(tb: Option<TimeBase>, sample_rate: u32, ts: u64) -> usize {
    match tb {
        Some(tb) => {
            (ts as u128 * tb.numer.get() as u128 * sample_rate as u128 / tb.denom.get() as u128)
                as usize
        }
        // No timebase: assume timestamps are already in frames.
        None => ts as usize,
    }
}

fn map_err(e: SymError) -> Error {
    match e {
        SymError::IoError(e) => Error::Io(e),
        SymError::DecodeError(s) => Error::Decode(s.into()),
        SymError::Unsupported(s) => Error::Unsupported(s.into()),
        other => Error::Other(other.to_string()),
    }
}
