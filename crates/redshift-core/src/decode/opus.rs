//! Ogg Opus (RFC 7845) decoding: `ogg` for the container, libopus for the audio.
//!
//! Timeline notes, since Opus has a few quirks:
//! * Output is always 48 kHz.
//! * Ogg granule positions count 48 kHz frames *including* the encoder's pre-skip, so the first
//!   `pre_skip` frames are decoder warm-up and are discarded.
//! * The final page's granule position marks where valid audio ends; anything decoded past it is
//!   padding.
//! * Only channel mapping family 0 (mono and stereo) is handled. Surround Opus needs libopus's
//!   multistream decoder, which `audiopus` doesn't wrap.

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

use audiopus::coder::Decoder as OpusDecoder;
use audiopus::packet::Packet as OpusPacket;
use audiopus::{Channels, MutSignals, SampleRate};
use ogg::PacketReader;

use super::{Decoder, StreamInfo};
use crate::error::{Error, Result};

const OPUS_RATE: u32 = 48_000;
/// Longest Opus packet: 120 ms.
const MAX_PACKET_FRAMES: usize = 5760;
/// After a seek, start decoding this far early so the decoder state converges (RFC 7845 §4.6: 80 ms).
const PREROLL_FRAMES: u64 = 3840;

pub struct OpusFileDecoder {
    reader: PacketReader<BufReader<File>>,
    serial: u32,
    decoder: OpusDecoder,
    gain_q8: i32,
    channels: usize,
    pre_skip: u64,
    /// Granule position where valid audio ends (the last page's granule), if it could be found.
    valid_end: Option<u64>,
    info: StreamInfo,

    /// Granule position of the next frame to be decoded. Only meaningful while `resolving` is false.
    pos_gp: u64,
    /// Frames before this granule position are discarded (pre-skip, or the run-up to a seek target).
    emit_from: u64,
    /// True after a seek until we know where on the timeline the first packet sits. A seek lands on
    /// a page boundary and the page header only tells us where the page *ends*, so we buffer the
    /// first page's packets, then work backwards: start = page end - frames decoded.
    resolving: bool,
    pending: Option<PendingPage>,
    at_end: bool,
    pcm: Vec<f32>,
}

/// Decoded packets that all ended on the same Ogg page (they share its granule position).
struct PendingPage {
    granule: u64,
    frames: usize,
    pcm: Vec<f32>,
}

impl OpusFileDecoder {
    pub fn open(path: &Path) -> Result<Self> {
        let mut reader = PacketReader::new(BufReader::new(File::open(path)?));

        let head = reader
            .read_packet()
            .map_err(ogg_err)?
            .ok_or_else(|| Error::Decode("empty Ogg file".into()))?;
        let d = &head.data;
        if d.len() < 19 || &d[..8] != b"OpusHead" {
            return Err(Error::Decode("missing OpusHead packet".into()));
        }
        if d[8] >> 4 != 0 {
            return Err(Error::Unsupported(format!("Opus header version {}", d[8])));
        }
        let channels = d[9] as usize;
        let pre_skip = u16::from_le_bytes([d[10], d[11]]) as u64;
        let gain_q8 = i16::from_le_bytes([d[16], d[17]]) as i32;
        let mapping_family = d[18];
        if mapping_family != 0 || !(1..=2).contains(&channels) {
            return Err(Error::Unsupported(format!(
                "Opus with {channels} channels (mapping family {mapping_family}); only mono and stereo are supported"
            )));
        }
        let serial = head.stream_serial();

        // The second packet is OpusTags. Tags are read separately by the library scanner.
        reader.read_packet().map_err(ogg_err)?;

        let valid_end = last_granule(path, serial);
        let total_frames = valid_end.map(|end| end.saturating_sub(pre_skip));

        Ok(Self {
            reader,
            serial,
            decoder: new_decoder(channels, gain_q8)?,
            gain_q8,
            channels,
            pre_skip,
            valid_end,
            info: StreamInfo {
                sample_rate: OPUS_RATE,
                channels,
                total_frames,
                codec: "opus",
                bits_per_sample: None,
            },
            pos_gp: 0,
            emit_from: pre_skip,
            resolving: false,
            pending: None,
            at_end: false,
            pcm: vec![0.0; MAX_PACKET_FRAMES * channels],
        })
    }

    /// Decodes one packet into `self.pcm`, returning the number of frames. A packet libopus rejects
    /// is replaced with silence of the same length so the timeline stays correct.
    fn decode_packet(&mut self, data: &[u8]) -> Option<usize> {
        let packet = OpusPacket::try_from(data).ok()?;
        let signals = MutSignals::try_from(&mut self.pcm[..]).ok()?;
        match self.decoder.decode_float(Some(packet), signals, false) {
            Ok(n) => Some(n),
            Err(_) => {
                let n = self.decoder.nb_samples(OpusPacket::try_from(data).ok()?).ok()?;
                let n = n.min(MAX_PACKET_FRAMES);
                self.pcm[..n * self.channels].fill(0.0);
                Some(n)
            }
        }
    }

    fn flush_pending(&mut self, out: &mut Vec<f32>) {
        if let Some(page) = self.pending.take() {
            self.pos_gp = page.granule.saturating_sub(page.frames as u64);
            self.resolving = false;
            emit(
                &mut self.pos_gp,
                self.emit_from,
                self.valid_end,
                self.channels,
                &page.pcm,
                out,
            );
        }
    }
}

impl Decoder for OpusFileDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        if self.at_end {
            return Ok(false);
        }
        let ch = self.channels;
        loop {
            let packet = match self.reader.read_packet().map_err(ogg_err)? {
                Some(p) => p,
                None => {
                    // End of file. A seek into the final page leaves its packets buffered.
                    self.at_end = true;
                    let before = out.len();
                    self.flush_pending(out);
                    return Ok(out.len() > before);
                }
            };
            if packet.stream_serial() != self.serial
                || packet.data.starts_with(b"OpusHead")
                || packet.data.starts_with(b"OpusTags")
            {
                continue;
            }
            let Some(frames) = self.decode_packet(&packet.data) else { continue };

            if !self.resolving {
                emit(
                    &mut self.pos_gp,
                    self.emit_from,
                    self.valid_end,
                    ch,
                    &self.pcm[..frames * ch],
                    out,
                );
                return Ok(true);
            }

            let granule = packet.absgp_page();
            match &mut self.pending {
                Some(page) if page.granule == granule => {
                    page.pcm.extend_from_slice(&self.pcm[..frames * ch]);
                    page.frames += frames;
                }
                None => {
                    self.pending = Some(PendingPage {
                        granule,
                        frames,
                        pcm: self.pcm[..frames * ch].to_vec(),
                    });
                }
                Some(_) => {
                    // First packet of the next page: the buffered page is complete.
                    self.flush_pending(out);
                    emit(
                        &mut self.pos_gp,
                        self.emit_from,
                        self.valid_end,
                        ch,
                        &self.pcm[..frames * ch],
                        out,
                    );
                    return Ok(true);
                }
            }
        }
    }

    fn seek(&mut self, seconds: f64) -> Result<()> {
        let target_frames = (seconds.max(0.0) * OPUS_RATE as f64) as u64;
        self.pending = None;
        if self.info.total_frames.is_some_and(|total| target_frames >= total) {
            self.at_end = true;
            return Ok(());
        }
        let target_gp = target_frames + self.pre_skip;
        let goal = target_gp.saturating_sub(PREROLL_FRAMES);
        match self.reader.seek_absgp(Some(self.serial), goal) {
            Ok(true) => {}
            Ok(false) => {
                self.at_end = true;
                return Ok(());
            }
            Err(e) => return Err(Error::Seek(e.to_string())),
        }
        // Decoder history belongs to the old position; start clean and let the pre-roll warm it up.
        self.decoder = new_decoder(self.channels, self.gain_q8)?;
        self.resolving = true;
        self.emit_from = target_gp;
        self.at_end = false;
        Ok(())
    }
}

/// Appends the valid part of `pcm` to `out` and advances `pos_gp` past it.
fn emit(
    pos_gp: &mut u64,
    emit_from: u64,
    valid_end: Option<u64>,
    channels: usize,
    pcm: &[f32],
    out: &mut Vec<f32>,
) {
    let frames = (pcm.len() / channels) as u64;
    let start = *pos_gp;
    let end = start + frames;
    *pos_gp = end;
    let lo = emit_from.max(start);
    let hi = valid_end.map_or(end, |v| end.min(v));
    if lo < hi {
        out.extend_from_slice(&pcm[(lo - start) as usize * channels..(hi - start) as usize * channels]);
    }
}

fn new_decoder(channels: usize, gain_q8: i32) -> Result<OpusDecoder> {
    let layout = if channels == 1 { Channels::Mono } else { Channels::Stereo };
    let decoder = OpusDecoder::new(SampleRate::Hz48000, layout)
        .map_err(|e| Error::Decode(format!("creating Opus decoder: {e}")))?;
    // The header's output gain is Q7.8 dB, the same unit libopus uses for OPUS_SET_GAIN.
    decoder
        .set_gain(gain_q8)
        .map_err(|e| Error::Decode(format!("setting Opus output gain: {e}")))?;
    Ok(decoder)
}

/// Reads the granule position of the last Ogg page of logical stream `serial`, which for a complete
/// file is the total length in frames (pre-skip included).
fn last_granule(path: &Path, serial: u32) -> Option<u64> {
    let mut file = File::open(path).ok()?;
    let len = file.metadata().ok()?.len();
    // An Ogg page is at most 27 + 255 + 255*255 bytes.
    let take = len.min(65_307 + 27);
    file.seek(SeekFrom::Start(len - take)).ok()?;
    let mut buf = vec![0u8; take as usize];
    file.read_exact(&mut buf).ok()?;

    let mut i = buf.len().checked_sub(27)?;
    loop {
        if &buf[i..i + 4] == b"OggS" && buf[i + 4] == 0 {
            let granule = u64::from_le_bytes(buf[i + 6..i + 14].try_into().ok()?);
            let page_serial = u32::from_le_bytes(buf[i + 14..i + 18].try_into().ok()?);
            // An all-ones granule means "no packet finishes on this page".
            if page_serial == serial && granule != u64::MAX {
                return Some(granule);
            }
        }
        i = i.checked_sub(1)?;
    }
}

fn ogg_err(e: ogg::OggReadError) -> Error {
    Error::Decode(format!("Ogg: {e}"))
}
