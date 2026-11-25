//! Playing a stream from the internet (radio) instead of a file.
//!
//! A station sends audio forever, so the pieces differ from a file:
//! * **A read-ahead buffer** on its own thread, so a hiccup on the network doesn't become a gap in
//!   the sound ([`Prefetch`]).
//! * **ICY metadata**: stations that are asked for it interleave the current song title into the audio
//!   every `icy-metaint` bytes. [`IcyReader`] takes it out and keeps the latest title.
//! * **Reconnecting** when the connection drops or stalls, with a pause that grows each try
//!   ([`LiveDecoder`]); the song title and the rest of the readout stay in [`LiveNow`].
//!
//! The decoder underneath is whichever fits the first bytes: libopus for Ogg Opus, Symphonia for the
//! rest (MP3, AAC in ADTS, FLAC, Ogg Vorbis).

use std::io::{self, Read};
use std::sync::Arc;
use std::time::Duration;

use crossbeam_channel::{bounded, Receiver, RecvTimeoutError};
use parking_lot::Mutex;
use serde::Serialize;

use super::opus::OpusLiveDecoder;
use super::{Decoder, StreamInfo, SymphoniaDecoder};
use crate::error::{Error, Result};
use crate::net::{Net, StreamHead};

/// How much audio is read ahead of the player: this many chunks of [`CHUNK`] bytes.
const AHEAD_CHUNKS: usize = 128;
const CHUNK: usize = 16 * 1024;
/// A station that sends nothing for this long counts as gone.
const STALL: Duration = Duration::from_secs(12);
/// Pauses between tries when the connection is lost; after these the stream gives up.
const RETRY_PAUSES: [u64; 5] = [0, 1, 2, 4, 8];

/// What the app shows about the stream that is playing.
#[derive(Debug, Clone, Default, Serialize, PartialEq)]
pub struct LiveNow {
    pub url: String,
    /// The station's own name for itself, from its headers.
    pub name: String,
    /// The song the station says is playing now (empty when it doesn't say).
    pub title: String,
    pub genre: String,
    pub codec: String,
    /// What the station claims (`icy-br`), in kbit/s.
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: u32,
    pub channels: usize,
    pub bits_per_sample: Option<u32>,
    /// A lossless codec: what arrives is what the station sent, bit for bit.
    pub lossless: bool,
    /// The address is https. When it isn't, the stream and the station's address cross the network unencrypted.
    pub secure: bool,
    /// The connection dropped and Redshift is trying to get it back.
    pub reconnecting: bool,
}

pub type SharedNow = Arc<Mutex<LiveNow>>;

type Chunk = io::Result<Vec<u8>>;

/// Reads a network body on its own thread and hands the bytes over through a bounded queue.
pub struct Prefetch {
    rx: Receiver<Chunk>,
    cur: Vec<u8>,
    pos: usize,
    stall: Duration,
}

impl Prefetch {
    pub fn spawn(mut source: Box<dyn Read + Send>, stall: Duration) -> Prefetch {
        let (tx, rx) = bounded::<Chunk>(AHEAD_CHUNKS);
        std::thread::Builder::new()
            .name("redshift-stream".into())
            .spawn(move || loop {
                let mut buf = vec![0u8; CHUNK];
                match source.read(&mut buf) {
                    Ok(0) => break, // the station closed the stream: the channel closing says so
                    Ok(n) => {
                        buf.truncate(n);
                        if tx.send(Ok(buf)).is_err() {
                            break; // nobody is listening any more
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            })
            .ok();
        Prefetch { rx, cur: Vec::new(), pos: 0, stall }
    }
}

impl Read for Prefetch {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos >= self.cur.len() {
            match self.rx.recv_timeout(self.stall) {
                Ok(Ok(chunk)) => {
                    self.cur = chunk;
                    self.pos = 0;
                }
                Ok(Err(e)) => return Err(e),
                Err(RecvTimeoutError::Disconnected) => return Ok(0),
                Err(RecvTimeoutError::Timeout) => return Err(io::Error::new(io::ErrorKind::TimedOut, "the station stopped sending audio")),
            }
        }
        let n = out.len().min(self.cur.len() - self.pos);
        out[..n].copy_from_slice(&self.cur[self.pos..self.pos + n]);
        self.pos += n;
        Ok(n)
    }
}

/// Removes the interleaved metadata blocks from a stream that was asked for them.
pub struct IcyReader<R> {
    inner: R,
    metaint: usize,
    until_meta: usize,
    now: SharedNow,
}

impl<R: Read> IcyReader<R> {
    pub fn new(inner: R, metaint: usize, now: SharedNow) -> Self {
        IcyReader { inner, metaint, until_meta: metaint, now }
    }

    fn read_meta(&mut self) -> io::Result<()> {
        let mut len = [0u8; 1];
        self.inner.read_exact(&mut len)?;
        let n = len[0] as usize * 16;
        if n > 0 {
            let mut block = vec![0u8; n];
            self.inner.read_exact(&mut block)?;
            if let Some(title) = parse_stream_title(&block) {
                self.now.lock().title = title;
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for IcyReader<R> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.until_meta == 0 {
            self.read_meta()?;
            self.until_meta = self.metaint;
        }
        let want = out.len().min(self.until_meta);
        let n = self.inner.read(&mut out[..want])?;
        self.until_meta -= n;
        Ok(n)
    }
}

/// `StreamTitle='Artist - Song';StreamUrl='...';` to the title. Stations send UTF-8 or, often,
/// Latin-1, so bytes that aren't valid UTF-8 are read as Latin-1.
pub fn parse_stream_title(block: &[u8]) -> Option<String> {
    let end = block.iter().rposition(|b| *b != 0).map_or(0, |i| i + 1);
    let block = &block[..end];
    let text = match std::str::from_utf8(block) {
        Ok(s) => s.to_string(),
        Err(_) => block.iter().map(|&b| b as char).collect(),
    };
    let start = text.find("StreamTitle='")? + "StreamTitle='".len();
    let rest = &text[start..];
    let stop = rest.find("';").or_else(|| rest.rfind('\'')).unwrap_or(rest.len());
    Some(rest[..stop].trim().to_string())
}

/// Puts back the bytes that were read to look at the start of a stream.
struct Replay {
    head: Vec<u8>,
    pos: usize,
    rest: Box<dyn Read + Send + Sync>,
}

impl Read for Replay {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        if self.pos < self.head.len() {
            let n = out.len().min(self.head.len() - self.pos);
            out[..n].copy_from_slice(&self.head[self.pos..self.pos + n]);
            self.pos += n;
            return Ok(n);
        }
        self.rest.read(out)
    }
}

/// Picks the decoder by looking at the first bytes of the stream.
pub fn open_reader(mut reader: Box<dyn Read + Send + Sync>, mime: &str) -> Result<Box<dyn Decoder>> {
    let mut head = vec![0u8; 96];
    let mut got = 0;
    while got < head.len() {
        match reader.read(&mut head[got..]) {
            Ok(0) => break,
            Ok(n) => got += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    head.truncate(got);
    if head.is_empty() {
        return Err(Error::Decode("the station sent no audio".into()));
    }
    let is_opus = head.starts_with(b"OggS") && head.windows(8).any(|w| w == b"OpusHead");
    let reader: Box<dyn Read + Send + Sync> = Box::new(Replay { head, pos: 0, rest: reader });
    if is_opus {
        Ok(Box::new(OpusLiveDecoder::open(reader)?))
    } else {
        Ok(Box::new(SymphoniaDecoder::open_live(reader, mime)?))
    }
}

/// What it takes to (re)connect: how to ask the network for the stream.
pub type Connect = Box<dyn Fn() -> Result<(Box<dyn Read + Send>, StreamHead)> + Send>;

/// A decoder for a station: connects, decodes, and reconnects when the connection is lost.
pub struct LiveDecoder {
    inner: Box<dyn Decoder>,
    info: StreamInfo,
    connect: Connect,
    now: SharedNow,
    stall: Duration,
}

fn start(connect: &Connect, now: &SharedNow, stall: Duration) -> Result<Box<dyn Decoder>> {
    let (body, head) = connect()?;
    {
        let mut n = now.lock();
        n.url = head.url.clone();
        if let Some(name) = &head.icy_name {
            n.name = name.clone();
        }
        if let Some(g) = &head.icy_genre {
            n.genre = g.clone();
        }
        n.bitrate_kbps = head.icy_bitrate;
        n.secure = head.encrypted;
    }
    let body: Box<dyn Read + Send> = match head.icy_metaint {
        Some(m) if m > 0 => Box::new(IcyReader::new(body, m, now.clone())),
        _ => body,
    };
    let reader: Box<dyn Read + Send + Sync> = Box::new(Prefetch::spawn(body, stall));
    open_reader(reader, &head.content_type)
}

impl LiveDecoder {
    pub fn open(connect: Connect, now: SharedNow) -> Result<Self> {
        Self::open_with(connect, now, STALL)
    }

    pub fn open_with(connect: Connect, now: SharedNow, stall: Duration) -> Result<Self> {
        let inner = start(&connect, &now, stall)?;
        // A station has no end, whatever a length in the first frame of the stream might claim.
        let mut info = inner.info().clone();
        info.total_frames = None;
        {
            let mut n = now.lock();
            n.codec = info.codec.to_string();
            n.sample_rate = info.sample_rate;
            n.channels = info.channels;
            n.bits_per_sample = info.bits_per_sample;
            n.lossless = matches!(info.codec, "flac" | "alac") || info.codec.starts_with("pcm");
            n.reconnecting = false;
        }
        Ok(LiveDecoder { inner, info, connect, now, stall })
    }

    fn reconnect(&mut self) -> Result<()> {
        self.now.lock().reconnecting = true;
        let mut last = Error::Decode("lost the connection to the station".into());
        for pause in RETRY_PAUSES {
            std::thread::sleep(Duration::from_secs(pause));
            match start(&self.connect, &self.now, self.stall) {
                Ok(d) if d.info().sample_rate == self.info.sample_rate && d.info().channels == self.info.channels => {
                    self.inner = d;
                    self.now.lock().reconnecting = false;
                    return Ok(());
                }
                Ok(_) => return Err(Error::Decode("the station changed its audio format".into())),
                Err(e) => last = e,
            }
        }
        Err(last)
    }
}

impl Decoder for LiveDecoder {
    fn info(&self) -> &StreamInfo {
        &self.info
    }

    fn read(&mut self, out: &mut Vec<f32>) -> Result<bool> {
        match self.inner.read(out) {
            Ok(true) => Ok(true),
            // A radio stream doesn't end by itself: both an end and an error mean the link is gone.
            Ok(false) | Err(_) => {
                out.clear();
                self.reconnect()?;
                Ok(true)
            }
        }
    }

    fn seek(&mut self, _: f64) -> Result<()> {
        Err(Error::Seek("a live stream can't be seeked".into()))
    }
}

/// Opens `url` through the network layer, which applies the online switch and logs the request.
pub fn open(net: Arc<Net>, url: &str, now: SharedNow) -> Result<LiveDecoder> {
    let url = url.to_string();
    let connect: Connect = Box::new(move || {
        let live = net.open_stream(&url, "Radio stream")?;
        Ok((live.reader, live.head))
    });
    LiveDecoder::open(connect, now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_come_out_of_the_metadata_block() {
        assert_eq!(parse_stream_title(b"StreamTitle='Paper Satellites - Carrier Wave';StreamUrl='';\0\0\0"), Some("Paper Satellites - Carrier Wave".into()));
        assert_eq!(parse_stream_title(b"StreamTitle='';\0\0"), Some(String::new()));
        assert_eq!(parse_stream_title(b"StreamTitle='It's a Trap - Live';"), Some("It's a Trap - Live".into()));
        assert_eq!(parse_stream_title(b"StreamUrl='x';"), None);
        // Latin-1: 0xE9 is e-acute.
        assert_eq!(parse_stream_title(b"StreamTitle='Caf\xe9 del Mar';"), Some("Café del Mar".into()));
    }

    #[test]
    fn metadata_is_removed_from_the_audio_and_the_title_kept() {
        let mut stream = Vec::new();
        let block = |text: &str| {
            let mut b = text.as_bytes().to_vec();
            b.resize(b.len().div_ceil(16) * 16, 0);
            let mut out = vec![(b.len() / 16) as u8];
            out.extend(b);
            out
        };
        stream.extend_from_slice(&[1u8; 10]);
        stream.extend(block("StreamTitle='First';"));
        stream.extend_from_slice(&[2u8; 10]);
        stream.push(0); // an empty block: no change
        stream.extend_from_slice(&[3u8; 10]);
        stream.extend(block("StreamTitle='Second';"));
        stream.extend_from_slice(&[4u8; 4]);
        let now: SharedNow = Default::default();
        let mut r = IcyReader::new(io::Cursor::new(stream), 10, now.clone());
        let mut audio = Vec::new();
        r.read_to_end(&mut audio).unwrap_or(0);
        // The last read hits the end inside a metadata slot, which is an error; what came before is kept.
        let expect: Vec<u8> = [vec![1u8; 10], vec![2u8; 10], vec![3u8; 10]].concat();
        assert_eq!(&audio[..30], &expect[..]);
        assert_eq!(now.lock().title, "Second");
    }

    #[test]
    fn prefetch_hands_over_everything_and_then_ends() {
        let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
        let mut p = Prefetch::spawn(Box::new(io::Cursor::new(data.clone())), Duration::from_secs(2));
        let mut got = Vec::new();
        p.read_to_end(&mut got).unwrap();
        assert_eq!(got, data);
    }

    #[test]
    fn a_silent_station_is_reported_as_stalled() {
        struct Never;
        impl Read for Never {
            fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
                std::thread::sleep(Duration::from_millis(600));
                Ok(0)
            }
        }
        let mut p = Prefetch::spawn(Box::new(Never), Duration::from_millis(100));
        let e = p.read(&mut [0u8; 8]).unwrap_err();
        assert_eq!(e.kind(), io::ErrorKind::TimedOut);
    }
}
