//! A song on the person's server or in the Internet Archive, readable at any position.
//!
//! The file is fetched in blocks with HTTP Range requests, so seeking jumps straight to the right
//! part instead of downloading everything before it, and the next blocks are fetched ahead of the
//! player so playback does not stall. A few blocks are kept, so the small back-and-forth reads of
//! a decoder do not cause new requests. If the server ignores Range requests (it answers 200 with
//! the whole file), the file is read whole into memory instead (up to [`WHOLE_MAX`]).

use std::collections::{HashMap, HashSet};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use symphonia::core::io::MediaSource;

use super::Net;
use crate::error::{Error, Result};

/// Bytes per request. A lossless song is a few dozen blocks.
pub const BLOCK: u64 = 1 << 20;
/// Blocks kept in memory.
const KEEP: usize = 24;
/// Blocks fetched ahead of the one being read.
const AHEAD: u64 = 2;
/// How long a read waits for a block before it gives up.
const WAIT: Duration = Duration::from_secs(25);
/// Tries per block.
const TRIES: u32 = 3;
/// Largest file read whole when the server does not do ranges.
pub const WHOLE_MAX: u64 = 512 * 1024 * 1024;

#[derive(Default)]
struct State {
    blocks: HashMap<u64, Arc<Vec<u8>>>,
    inflight: HashSet<u64>,
    failed: HashMap<u64, String>,
    /// The block the player is reading now; blocks far from it are dropped first.
    cursor: u64,
    closed: bool,
}

/// Where a song comes from.
#[derive(Debug, Clone, PartialEq)]
pub enum Origin {
    /// A song on the person's own Subsonic server, by the server's id.
    Server(String),
    /// A file in an Internet Archive item.
    Archive { identifier: String, file: String },
}

impl Origin {
    /// A name that is safe as part of a file name, for the cache.
    pub fn cache_name(&self) -> String {
        let raw = match self {
            Origin::Server(id) => format!("s-{id}"),
            Origin::Archive { identifier, file } => format!("a-{identifier}-{file}"),
        };
        raw.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect()
    }

    /// The file extension, when the name has one (a hint for telling the format).
    pub fn extension(&self) -> String {
        match self {
            Origin::Archive { file, .. } => file.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).unwrap_or_default(),
            Origin::Server(_) => String::new(),
        }
    }

    /// Asks for the file (or the bytes `range` of it).
    fn request(&self, net: &Net, range: Option<(u64, u64)>, log: bool) -> Result<super::ServerBody> {
        match self {
            Origin::Server(id) => {
                let url = net.server_stream_url(id)?;
                net.server_open(&url, range, "Server: song", log)
            }
            Origin::Archive { identifier, file } => {
                let url = net.archive_stream_url(identifier, file)?;
                net.archive_open(&url, "audio/*, */*", range, "Internet Archive: song", log)
            }
        }
    }

    fn source_word(&self) -> &'static str {
        match self {
            Origin::Server(_) => "the server",
            Origin::Archive { .. } => "the Internet Archive",
        }
    }
}

struct Shared {
    net: Arc<Net>,
    origin: Origin,
    total: u64,
    /// The whole file, when the server could not do ranges.
    whole: Option<Vec<u8>>,
    state: Mutex<State>,
    cv: Condvar,
}

pub struct RangedReader {
    shared: Arc<Shared>,
    pos: u64,
    content_type: String,
}

fn io_err(msg: impl Into<String>) -> io::Error {
    io::Error::other(msg.into())
}

fn block_count(total: u64) -> u64 {
    total.div_ceil(BLOCK)
}

impl Shared {
    fn nblocks(&self) -> u64 {
        block_count(self.total)
    }

    fn block_range(&self, idx: u64) -> (u64, u64) {
        let start = idx * BLOCK;
        let end = (start + BLOCK).min(self.total) - 1;
        (start, end)
    }

    /// Downloads one block, trying a few times.
    fn download(&self, idx: u64) -> std::result::Result<Vec<u8>, String> {
        let (start, end) = self.block_range(idx);
        let want = (end - start + 1) as usize;
        let mut last = String::new();
        for attempt in 0..TRIES {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(300 * u64::from(attempt)));
            }
            if self.state.lock().closed {
                return Err("closed".into());
            }
            let result = (|| -> Result<Vec<u8>> {
                let mut body = self.origin.request(&self.net, Some((start, end)), false)?;
                if body.status != 206 {
                    return Err(Error::Network(format!("{} answered {} to a request for part of a song", self.origin.source_word(), body.status)));
                }
                let mut buf = Vec::with_capacity(want);
                body.reader.by_ref().take(want as u64).read_to_end(&mut buf).map_err(|e| Error::Network(e.to_string()))?;
                if buf.len() != want {
                    return Err(Error::Network("the connection ended early".into()));
                }
                Ok(buf)
            })();
            match result {
                Ok(buf) => return Ok(buf),
                Err(e) => last = e.to_string(),
            }
        }
        Err(last)
    }

    fn start_fetch(self: &Arc<Self>, st: &mut State, idx: u64) {
        if idx >= self.nblocks() || st.blocks.contains_key(&idx) || st.inflight.contains(&idx) || st.closed {
            return;
        }
        st.inflight.insert(idx);
        let shared = self.clone();
        let spawned = std::thread::Builder::new().name("server-block".into()).spawn(move || {
            let result = shared.download(idx);
            let mut st = shared.state.lock();
            st.inflight.remove(&idx);
            match result {
                Ok(buf) => {
                    st.failed.remove(&idx);
                    st.blocks.insert(idx, Arc::new(buf));
                    let cursor = st.cursor;
                    while st.blocks.len() > KEEP {
                        // Drop the block farthest from where the player is.
                        let far = st.blocks.keys().copied().max_by_key(|b| b.abs_diff(cursor));
                        match far {
                            Some(b) if b != idx => {
                                st.blocks.remove(&b);
                            }
                            _ => break,
                        }
                    }
                }
                Err(e) => {
                    st.failed.insert(idx, e);
                }
            }
            shared.cv.notify_all();
        });
        if spawned.is_err() {
            st.inflight.remove(&idx);
            st.failed.insert(idx, "could not start a download".into());
        }
    }

    /// The block `idx`, waiting for it if it is not here yet. Also asks for the ones after it.
    fn get(self: &Arc<Self>, idx: u64) -> io::Result<Arc<Vec<u8>>> {
        let mut st = self.state.lock();
        st.cursor = idx;
        for ahead in 1..=AHEAD {
            self.start_fetch(&mut st, idx + ahead);
        }
        loop {
            if let Some(b) = st.blocks.get(&idx) {
                return Ok(b.clone());
            }
            if let Some(e) = st.failed.remove(&idx) {
                return Err(io_err(e));
            }
            self.start_fetch(&mut st, idx);
            if st.closed {
                return Err(io_err("closed"));
            }
            if self.cv.wait_for(&mut st, WAIT).timed_out() && !st.blocks.contains_key(&idx) {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the server took too long to send the next part of the song"));
            }
        }
    }
}

impl RangedReader {
    /// Opens a song on the person's server.
    pub fn open(net: Arc<Net>, id: &str) -> Result<Self> {
        Self::open_origin(net, Origin::Server(id.to_string()))
    }

    /// Opens a song. The first block is fetched here, so a wrong login, a missing song or an
    /// unreachable server shows up now and not in the middle of playback.
    pub fn open_origin(net: Arc<Net>, origin: Origin) -> Result<Self> {
        let who = origin.source_word();
        let mut body = origin.request(&net, Some((0, BLOCK - 1)), true)?;
        let kind = body.content_type.clone();
        if matches!(origin, Origin::Server(_)) && (kind.contains("json") || kind.contains("xml") || kind.starts_with("text/")) {
            // The Subsonic way of saying no: an error document where the song should be.
            let mut text = Vec::new();
            let _ = body.reader.by_ref().take(16 * 1024).read_to_end(&mut text);
            return Err(super::subsonic::error_from_document(&text));
        }
        match body.status {
            206 => {
                let total = body.total.ok_or_else(|| Error::Network(format!("{who} did not say how large the song is")))?;
                if total == 0 {
                    return Err(Error::Network("the song is empty".into()));
                }
                let want = total.min(BLOCK) as usize;
                let mut first = Vec::with_capacity(want);
                body.reader.by_ref().take(want as u64).read_to_end(&mut first).map_err(|e| Error::Network(e.to_string()))?;
                if first.len() != want {
                    return Err(Error::Network("the connection ended early".into()));
                }
                let mut st = State::default();
                st.blocks.insert(0, Arc::new(first));
                let shared = Arc::new(Shared { net, origin, total, whole: None, state: Mutex::new(st), cv: Condvar::new() });
                Ok(Self { shared, pos: 0, content_type: kind })
            }
            200 => {
                // No range support: read it all.
                if body.total.is_some_and(|t| t > WHOLE_MAX) {
                    return Err(Error::Network(format!("{who} cannot send part of a song, and the song is too large to load whole")));
                }
                let mut all = Vec::new();
                body.reader.by_ref().take(WHOLE_MAX + 1).read_to_end(&mut all).map_err(|e| Error::Network(e.to_string()))?;
                if all.len() as u64 > WHOLE_MAX {
                    return Err(Error::Network(format!("{who} cannot send part of a song, and the song is too large to load whole")));
                }
                let total = all.len() as u64;
                let shared = Arc::new(Shared { net, origin, total, whole: Some(all), state: Mutex::new(State::default()), cv: Condvar::new() });
                Ok(Self { shared, pos: 0, content_type: kind })
            }
            401 | 403 => Err(Error::Network(match origin {
                Origin::Server(_) => "the server refused the login".to_string(),
                Origin::Archive { .. } => "the Internet Archive does not let this file be played (it may be restricted)".to_string(),
            })),
            404 => Err(Error::Network(match origin {
                Origin::Server(_) => "the server does not have that song any more".to_string(),
                Origin::Archive { .. } => "the Internet Archive does not have that file any more".to_string(),
            })),
            s => Err(Error::Network(format!("{who} answered {s}"))),
        }
    }

    /// The server's description of the file (`audio/flac`), if it gave one.
    pub fn content_type(&self) -> &str {
        &self.content_type
    }

    /// The first `n` bytes of the file, for telling what kind of file it is. Call right after
    /// [`open`](Self::open).
    pub fn head(&self, n: usize) -> Vec<u8> {
        if let Some(all) = &self.shared.whole {
            return all[..n.min(all.len())].to_vec();
        }
        self.shared.state.lock().blocks.get(&0).map(|b| b[..n.min(b.len())].to_vec()).unwrap_or_default()
    }

    /// Writes the whole file to `dest` (a half file is never left under that name).
    pub fn save_to(&mut self, dest: &std::path::Path) -> Result<()> {
        let part = dest.with_extension("part");
        let result = (|| -> io::Result<()> {
            self.seek(SeekFrom::Start(0))?;
            let mut out = std::fs::File::create(&part)?;
            io::copy(self, &mut out)?;
            out.flush()?;
            Ok(())
        })();
        match result {
            Ok(()) => std::fs::rename(&part, dest).map_err(Error::Io),
            Err(e) => {
                let _ = std::fs::remove_file(&part);
                Err(Error::Network(e.to_string()))
            }
        }
    }

    pub fn len(&self) -> u64 {
        self.shared.total
    }

    pub fn is_empty(&self) -> bool {
        self.shared.total == 0
    }

    /// True when the server could do ranges (the song is fetched in pieces).
    pub fn is_ranged(&self) -> bool {
        self.shared.whole.is_none()
    }
}

impl Drop for RangedReader {
    fn drop(&mut self) {
        self.shared.state.lock().closed = true;
        self.shared.cv.notify_all();
    }
}

impl Read for RangedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() || self.pos >= self.shared.total {
            return Ok(0);
        }
        if let Some(all) = &self.shared.whole {
            let start = self.pos as usize;
            let n = buf.len().min(all.len() - start);
            buf[..n].copy_from_slice(&all[start..start + n]);
            self.pos += n as u64;
            return Ok(n);
        }
        let idx = self.pos / BLOCK;
        let block = self.shared.get(idx)?;
        let off = (self.pos - idx * BLOCK) as usize;
        let n = buf.len().min(block.len().saturating_sub(off));
        buf[..n].copy_from_slice(&block[off..off + n]);
        self.pos += n as u64;
        Ok(n)
    }
}

impl Seek for RangedReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let target = match to {
            SeekFrom::Start(n) => i128::from(n),
            SeekFrom::Current(d) => i128::from(self.pos) + i128::from(d),
            SeekFrom::End(d) => i128::from(self.shared.total) + i128::from(d),
        };
        if target < 0 {
            return Err(io::Error::new(io::ErrorKind::InvalidInput, "seek before the start of the song"));
        }
        self.pos = target.min(i128::from(u64::MAX)) as u64;
        Ok(self.pos)
    }
}

impl MediaSource for RangedReader {
    fn is_seekable(&self) -> bool {
        true
    }
    fn byte_len(&self) -> Option<u64> {
        Some(self.shared.total)
    }
}

#[cfg(test)]
mod tests {
    use super::super::fakesub::{self, Fake};
    use super::*;
    use crate::net::ServerConfig;

    fn data(n: usize) -> Vec<u8> {
        (0..n).map(|i| (i.wrapping_mul(31).wrapping_add(i >> 8) % 251) as u8).collect()
    }

    fn net_for(f: &Fake, tag: &str) -> Arc<Net> {
        let n = Arc::new(Net::new(std::env::temp_dir().join(format!("redshift-ranged-{tag}")), "t", &[]));
        n.set_enabled(true);
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: fakesub::USER.into(), password: fakesub::PASSWORD.into() }));
        n
    }

    #[test]
    fn it_reads_a_song_in_blocks_and_gets_every_byte() {
        let audio = data(BLOCK as usize * 3 + 12345);
        let f = fakesub::start(audio.clone(), true);
        let mut r = RangedReader::open(net_for(&f, "seq"), "s1").unwrap();
        assert!(r.is_ranged());
        assert_eq!(r.len(), audio.len() as u64);
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert!(got == audio, "the bytes read back are not the bytes served");
        let ranges = f.ranges.lock().clone();
        assert!(ranges.contains(&format!("bytes=0-{}", BLOCK - 1)), "{ranges:?}");
        assert!(ranges.len() >= 4, "every block should have been asked for: {ranges:?}");
    }

    #[test]
    fn seeking_fetches_only_the_part_that_is_needed() {
        let audio = data(BLOCK as usize * 8);
        let f = fakesub::start(audio.clone(), true);
        let mut r = RangedReader::open(net_for(&f, "seek"), "s1").unwrap();
        r.seek(SeekFrom::Start(BLOCK * 6 + 100)).unwrap();
        let mut buf = [0u8; 64];
        r.read_exact(&mut buf).unwrap();
        assert_eq!(&buf[..], &audio[(BLOCK * 6 + 100) as usize..][..64]);
        r.seek(SeekFrom::End(-10)).unwrap();
        let mut tail = Vec::new();
        r.read_to_end(&mut tail).unwrap();
        assert_eq!(tail, audio[audio.len() - 10..]);
        r.seek(SeekFrom::Current(-20)).unwrap();
        let mut back = [0u8; 20];
        r.read_exact(&mut back).unwrap();
        assert_eq!(&back[..], &audio[audio.len() - 20..audio.len()][..20]);
        assert!(r.seek(SeekFrom::Current(-1_000_000_000)).is_err());
        let ranges = f.ranges.lock().clone();
        assert!(!ranges.iter().any(|x| x.starts_with(&format!("bytes={}-", BLOCK * 3))), "blocks in the middle were never needed: {ranges:?}");
    }

    #[test]
    fn a_server_without_range_support_is_read_whole() {
        let audio = data(BLOCK as usize + 777);
        let f = fakesub::start(audio.clone(), false);
        let mut r = RangedReader::open(net_for(&f, "whole"), "s1").unwrap();
        assert!(!r.is_ranged());
        r.seek(SeekFrom::Start(BLOCK)).unwrap();
        let mut got = Vec::new();
        r.read_to_end(&mut got).unwrap();
        assert_eq!(got, audio[BLOCK as usize..]);
    }

    #[test]
    fn a_wrong_password_is_said_in_words() {
        let f = fakesub::start(data(1000), true);
        let n = Arc::new(Net::new(std::env::temp_dir().join("redshift-ranged-bad"), "t", &[]));
        n.set_enabled(true);
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: fakesub::USER.into(), password: "nope".into() }));
        let err = RangedReader::open(n, "s1").err().expect("must fail").to_string();
        assert!(err.contains("not accepted"), "{err}");
    }

    #[test]
    fn nothing_is_requested_while_online_features_are_off() {
        let f = fakesub::start(data(1000), true);
        let n = net_for(&f, "off");
        n.set_enabled(false);
        assert!(matches!(RangedReader::open(n, "s1").err(), Some(Error::Offline)));
        assert!(f.requests.lock().is_empty());
    }

    #[test]
    fn the_log_shows_the_song_request_once_and_never_the_token() {
        let audio = data(BLOCK as usize * 2);
        let f = fakesub::start(audio, true);
        let n = net_for(&f, "log");
        let mut r = RangedReader::open(n.clone(), "s1").unwrap();
        let mut sink = Vec::new();
        r.read_to_end(&mut sink).unwrap();
        let log = n.log();
        assert_eq!(log.len(), 1, "{log:?}");
        assert!(!log[0].url.contains("t=") && !log[0].url.contains(fakesub::PASSWORD), "{}", log[0].url);
    }
}
