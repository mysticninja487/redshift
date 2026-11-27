//! The network layer: the one place Redshift talks to the internet.
//!
//! Everything online goes through [`Net`], so the rules live in one spot:
//!
//! * **Off unless switched on.** A `Net` starts disabled and every request fails with
//!   [`Error::Offline`] until the user has turned online features on. Nothing is sent before that.
//! * **Only known hosts.** Each `Net` is built with a short list of hosts it may talk to (the lyrics
//!   service, GitHub for update checks). Anything else is refused, including redirects that leave the
//!   list, so a bad response cannot send the app somewhere unexpected.
//! * **HTTPS only**, except for the loopback address, which the tests use.
//! * **Small and bounded.** Short timeouts, a size limit on every reply, at most two redirects and at
//!   most one retry (after a 429 or 503).
//! * **Cached.** Replies, and "not found" answers, can be kept on disk for a while, so the same song
//!   is not asked about twice and the services are left in peace.
//! * **Transparent.** The last requests are kept in a log (what was asked, of whom, what came back)
//!   so Settings can show exactly what has left the computer.
//!
//! This is a blocking client (`ureq`), meant to be called from a worker thread, never from the audio
//! thread or the UI thread.

#[cfg(test)]
pub(crate) mod fakearchive;
#[cfg(test)]
pub(crate) mod fakesub;
pub mod artwork;
pub mod betterlyrics;
pub mod lrclib;
pub mod radiobrowser;
pub mod archive;
pub mod ranged;
pub mod remote_playlist;
pub mod subsonic;
pub mod updates;

use std::collections::VecDeque;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// A reply larger than this is cut off and counts as an error; real replies here are a few KB.
pub const MAX_BODY_BYTES: u64 = 2 * 1024 * 1024;
/// How many requests the log remembers.
pub const LOG_LEN: usize = 60;
/// Cached replies older than this are deleted when the layer starts.
const CACHE_MAX_AGE: Duration = Duration::from_secs(30 * 24 * 3600);
const MAX_REDIRECTS: u8 = 2;

/// One line of the request log.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RequestRecord {
    /// Seconds since 1970.
    pub time: u64,
    /// What the request was for, in words ("Lyrics", "Update check").
    pub purpose: String,
    pub host: String,
    /// The full address that was asked, including the query, which is everything that was sent.
    pub url: String,
    pub status: Option<u16>,
    pub bytes: usize,
    /// Answered from the cache, so nothing went over the network.
    pub cached: bool,
    pub error: Option<String>,
}

/// What to fetch.
#[derive(Debug, Clone)]
pub struct Request<'a> {
    pub url: &'a str,
    pub purpose: &'a str,
    /// Keep the reply (200 and 404 only) this long, and use a kept one if there is one.
    pub cache_ttl: Option<Duration>,
    /// How long a kept 404 ("nothing found") is believed, if shorter than `cache_ttl`.
    pub miss_ttl: Option<Duration>,
    /// Largest reply accepted; [`MAX_BODY_BYTES`] if not set.
    pub max_body: Option<u64>,
    /// What kind of reply is asked for; JSON if not set.
    pub accept: Option<&'a str>,
    /// A JSON body: the request is then a POST (never cached, and redirects are not followed).
    pub post_json: Option<&'a str>,
}

impl<'a> Request<'a> {
    pub fn new(url: &'a str, purpose: &'a str) -> Self {
        Self { url, purpose, cache_ttl: None, miss_ttl: None, max_body: None, accept: None, post_json: None }
    }
    /// Accept a reply up to this many bytes (a web page can be larger than the usual limit).
    pub fn max_body(mut self, bytes: u64) -> Self {
        self.max_body = Some(bytes);
        self
    }
    pub fn accept(mut self, accept: &'a str) -> Self {
        self.accept = Some(accept);
        self
    }
    pub fn post_json(mut self, body: &'a str) -> Self {
        self.post_json = Some(body);
        self
    }
    pub fn cached_for(mut self, ttl: Duration) -> Self {
        self.cache_ttl = Some(ttl);
        self
    }
    /// Believe a kept "not found" for less time than a kept answer, so a song whose lyrics get
    /// added later is looked up again sooner.
    pub fn misses_for(mut self, ttl: Duration) -> Self {
        self.miss_ttl = Some(ttl);
        self
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct Reply {
    pub status: u16,
    pub body: String,
    pub cached: bool,
}

#[derive(Serialize, Deserialize)]
struct CacheEntry {
    t: u64,
    s: u16,
    b: String,
}

/// What a station says about its stream in the reply headers.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StreamHead {
    /// The address that finally answered, after redirects.
    pub url: String,
    pub content_type: String,
    pub icy_name: Option<String>,
    pub icy_genre: Option<String>,
    pub icy_bitrate: Option<u32>,
    /// Bytes of audio between metadata blocks, when the station sends song titles inline.
    pub icy_metaint: Option<usize>,
    /// https. Plain http is allowed for stations, but it is said so.
    pub encrypted: bool,
}

/// An open stream: the headers and a reader for the audio.
pub struct LiveBody {
    pub head: StreamHead,
    pub reader: Box<dyn std::io::Read + Send>,
}

const STREAM_REDIRECTS: u8 = 5;

/// The person's own Navidrome or Subsonic server. Unlike the hosts Redshift ships with, this one
/// is typed in by the person, may be on the local network and may be plain http, so it is the one
/// extra place [`Net`] will talk to, and only while it is set.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerConfig {
    /// For example `https://music.example.com` or `http://192.168.1.20:4533`.
    pub url: String,
    pub user: String,
    /// Kept in plain text in the app's data folder (the Subsonic login needs the password itself to
    /// make its one-time tokens). Use an account made for Redshift.
    pub password: String,
}

#[derive(Debug, Clone)]
struct ServerAccess {
    /// `http(s)://host[:port][/path]` without a trailing slash.
    base: String,
    /// `host[:port]` in lowercase: the only authority requests may go to.
    authority: String,
    user: String,
    password: String,
}

/// What a server answered to an audio request.
pub struct ServerBody {
    pub status: u16,
    pub content_type: String,
    /// The whole file's size, from `Content-Range` or `Content-Length`.
    pub total: Option<u64>,
    pub reader: Box<dyn std::io::Read + Send>,
}

impl ServerConfig {
    pub fn is_set(&self) -> bool {
        !self.url.trim().is_empty() && !self.user.trim().is_empty()
    }
    /// The address with a scheme (https if none was typed) and no trailing slash or `/rest`.
    pub fn base_url(&self) -> String {
        let mut u = self.url.trim().to_string();
        if !u.contains("://") {
            u = format!("https://{u}");
        }
        let mut u = u.trim_end_matches('/').to_string();
        if u.to_ascii_lowercase().ends_with("/rest") {
            u.truncate(u.len() - 5);
        }
        u
    }
}

/// `scheme://authority/path` of an address, without its query: what the request log shows for
/// the server, so the login token never lands in it.
fn without_query(url: &str) -> String {
    url.split('?').next().unwrap_or(url).to_string()
}

pub struct Net {
    agent: ureq::Agent,
    /// No overall time limit: a stream is meant to go on.
    stream_agent: ureq::Agent,
    hosts: Vec<String>,
    server: parking_lot::RwLock<Option<ServerAccess>>,
    /// Where the Internet Archive is: `https://archive.org`, or a test server on this computer.
    archive_base: parking_lot::RwLock<String>,
    salt_counter: AtomicU64,
    enabled: AtomicBool,
    /// Lets streams come from this computer (tests); never on in the app.
    allow_local: AtomicBool,
    /// The same rules with time for a large file.
    download_agent: ureq::Agent,
    cache_dir: PathBuf,
    log: Mutex<VecDeque<RequestRecord>>,
}

fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "localhost" | "[::1]" | "::1")
}

/// Stable file name for a cached URL (FNV-1a over the bytes; the cache is not a security boundary).
fn cache_name(url: &str) -> String {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in url.bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x100000001b3);
    }
    format!("{h:016x}.json")
}

/// Percent-encodes a query value (everything but unreserved characters).
pub fn encode_query(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 8);
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

impl Net {
    /// A disabled client that may only talk to `hosts` (bare host names, no scheme). Cached replies
    /// live in `cache_dir`, which is created when first needed.
    pub fn new(cache_dir: impl Into<PathBuf>, user_agent: &str, hosts: &[&str]) -> Net {
        let build = |global: Duration| -> ureq::Agent {
            ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_connect(Some(Duration::from_secs(6)))
                .timeout_global(Some(global))
                .user_agent(user_agent)
                // Trust the operating system's certificate store (so antivirus and company proxies that
                // install their own root certificate on Windows keep working) rather than a bundled list.
                .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
                .build()
                .into()
        };
        let net = Net {
            agent: build(Duration::from_secs(15)),
            download_agent: build(Duration::from_secs(20 * 60)),
            stream_agent: ureq::Agent::config_builder()
                .http_status_as_error(false)
                .max_redirects(0)
                .timeout_connect(Some(Duration::from_secs(8)))
                .timeout_recv_response(Some(Duration::from_secs(12)))
                .timeout_global(None)
                .user_agent(user_agent)
                .tls_config(ureq::tls::TlsConfig::builder().root_certs(ureq::tls::RootCerts::PlatformVerifier).build())
                .build()
                .into(),
            hosts: hosts.iter().map(|h| h.to_ascii_lowercase()).collect(),
            server: parking_lot::RwLock::new(None),
            archive_base: parking_lot::RwLock::new(archive::OFFICIAL_BASE.to_string()),
            salt_counter: AtomicU64::new(0),
            enabled: AtomicBool::new(false),
            allow_local: AtomicBool::new(false),
            cache_dir: cache_dir.into(),
            log: Mutex::new(VecDeque::new()),
        };
        net.prune_cache();
        net
    }

    /// Where cached replies (and downloaded songs that cannot be streamed) are kept.
    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    pub fn set_enabled(&self, on: bool) {
        self.enabled.store(on, Ordering::Relaxed);
    }

    #[doc(hidden)]
    pub fn allow_local_streams(&self, on: bool) {
        self.allow_local.store(on, Ordering::Relaxed);
    }

    pub fn enabled(&self) -> bool {
        self.enabled.load(Ordering::Relaxed)
    }

    /// The most recent requests, newest first.
    pub fn log(&self) -> Vec<RequestRecord> {
        self.log.lock().iter().rev().cloned().collect()
    }

    pub fn clear_log(&self) {
        self.log.lock().clear();
    }

    /// Deletes every cached reply and returns how many there were.
    pub fn clear_cache(&self) -> usize {
        let Ok(rd) = fs::read_dir(&self.cache_dir) else { return 0 };
        rd.filter_map(|e| e.ok()).filter(|e| fs::remove_file(e.path()).is_ok()).count()
    }

    /// Number of cached replies and their total size in bytes.
    pub fn cache_stats(&self) -> (usize, u64) {
        let Ok(rd) = fs::read_dir(&self.cache_dir) else { return (0, 0) };
        rd.filter_map(|e| e.ok()).filter_map(|e| e.metadata().ok()).fold((0, 0), |(n, b), m| (n + 1, b + m.len()))
    }

    fn prune_cache(&self) {
        let Ok(rd) = fs::read_dir(&self.cache_dir) else { return };
        for e in rd.filter_map(|e| e.ok()) {
            let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > CACHE_MAX_AGE);
            if old {
                let _ = fs::remove_file(e.path());
            }
        }
    }

    /// Checks scheme and host against the rules and returns the host name.
    fn check_url(&self, url: &str) -> Result<String> {
        let uri: ureq::http::Uri = url.parse().map_err(|_| Error::Network(format!("not a valid address: {url}")))?;
        let host = uri.host().ok_or_else(|| Error::Network("address has no host".into()))?.to_ascii_lowercase();
        let scheme = uri.scheme_str().unwrap_or("");
        if !(scheme == "https" || (scheme == "http" && is_loopback(&host))) {
            return Err(Error::Network("only secure (https) addresses are allowed".into()));
        }
        if !self.hosts.iter().any(|h| *h == host) {
            return Err(Error::Network(format!("{host} is not one of the places Redshift may contact")));
        }
        Ok(host)
    }

    fn cache_path(&self, url: &str) -> PathBuf {
        self.cache_dir.join(cache_name(url))
    }

    fn cache_read(&self, url: &str, ttl: Duration, miss_ttl: Option<Duration>) -> Option<CacheEntry> {
        let text = fs::read_to_string(self.cache_path(url)).ok()?;
        let e: CacheEntry = serde_json::from_str(&text).ok()?;
        let limit = if matches!(e.s, 401 | 404) { miss_ttl.map_or(ttl, |m| m.min(ttl)) } else { ttl };
        (now_secs().saturating_sub(e.t) <= limit.as_secs()).then_some(e)
    }

    fn cache_write(&self, url: &str, status: u16, body: &str) {
        if fs::create_dir_all(&self.cache_dir).is_err() {
            return;
        }
        let Ok(text) = serde_json::to_string(&CacheEntry { t: now_secs(), s: status, b: body.to_string() }) else { return };
        let path = self.cache_path(url);
        let tmp = path.with_extension("tmp");
        if fs::write(&tmp, text).is_ok() && fs::rename(&tmp, &path).is_err() {
            let _ = fs::remove_file(&tmp);
        }
    }

    fn record(&self, req: &Request, host: &str, status: Option<u16>, bytes: usize, cached: bool, error: Option<String>) {
        let mut log = self.log.lock();
        if log.len() >= LOG_LEN {
            log.pop_front();
        }
        log.push_back(RequestRecord {
            time: now_secs(),
            purpose: req.purpose.to_string(),
            host: host.to_string(),
            url: req.url.to_string(),
            status,
            bytes,
            cached,
            error,
        });
    }

    /// Fetches `req` with GET. Fails with [`Error::Offline`] while online features are off. A reply
    /// with any HTTP status is returned as a [`Reply`]; the caller decides what 404 or 500 means.
    pub fn get(&self, req: &Request) -> Result<Reply> {
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let host = self.check_url(req.url).inspect_err(|e| self.record(req, "", None, 0, false, Some(e.to_string())))?;
        if let Some(ttl) = req.cache_ttl {
            if let Some(e) = self.cache_read(req.url, ttl, req.miss_ttl) {
                self.record(req, &host, Some(e.s), e.b.len(), true, None);
                return Ok(Reply { status: e.s, body: e.b, cached: true });
            }
        }
        // What a POST sends is in its body, so the log shows the body as well as the address.
        let shown = req.post_json.map(|b| format!("{} (POST {})", req.url, b));
        let logged = Request { url: shown.as_deref().unwrap_or(req.url), ..*req };
        let req_log = &logged;
        match self.fetch(req) {
            Ok(reply) => {
                self.record(req_log, &host, Some(reply.status), reply.body.len(), false, None);
                if req.cache_ttl.is_some() && matches!(reply.status, 200 | 401 | 404) {
                    self.cache_write(req.url, reply.status, &reply.body);
                }
                Ok(reply)
            }
            Err(e) => {
                self.record(req_log, &host, None, 0, false, Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// Downloads a file (an update) to `dest`, checking every redirect against the host list, and
    /// returns the SHA-256 of what was written as lowercase hex. `progress(done, total)` is called as
    /// the data arrives. The file is written next to `dest` first and only renamed when complete, so
    /// a broken download never leaves a half file under the real name.
    pub fn download(&self, url: &str, purpose: &str, dest: &Path, max_bytes: u64, progress: &mut dyn FnMut(u64, Option<u64>)) -> Result<String> {
        use sha2::{Digest, Sha256};
        use std::io::{Read, Write};
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let req = Request::new(url, purpose);
        let host = self.check_url(url).inspect_err(|e| self.record(&req, "", None, 0, false, Some(e.to_string())))?;
        let mut current = url.to_string();
        let mut redirects = 0u8;
        let result = (|| -> Result<(u16, u64, String)> {
            loop {
                self.check_url(&current)?;
                let mut resp = self.download_agent.get(&current).header("Accept", "application/octet-stream").call().map_err(|e| Error::Network(e.to_string()))?;
                let status = resp.status().as_u16();
                if matches!(status, 301 | 302 | 307 | 308) {
                    let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string).ok_or_else(|| Error::Network("redirect without a target".into()))?;
                    redirects += 1;
                    if redirects > MAX_REDIRECTS {
                        return Err(Error::Network("too many redirects".into()));
                    }
                    current = loc;
                    continue;
                }
                if status != 200 {
                    return Ok((status, 0, String::new()));
                }
                let total = resp.headers().get("content-length").and_then(|v| v.to_str().ok()).and_then(|v| v.parse::<u64>().ok());
                if total.is_some_and(|t| t > max_bytes) {
                    return Err(Error::Network("the file is larger than expected".into()));
                }
                if let Some(dir) = dest.parent() {
                    fs::create_dir_all(dir)?;
                }
                let part = dest.with_extension("part");
                let mut out = fs::File::create(&part)?;
                let mut reader = resp.body_mut().with_config().limit(max_bytes).reader();
                let mut hasher = Sha256::new();
                let mut buf = vec![0u8; 64 * 1024];
                let mut done = 0u64;
                let written = (|| -> Result<()> {
                    loop {
                        let n = reader.read(&mut buf).map_err(|e| Error::Network(e.to_string()))?;
                        if n == 0 {
                            break;
                        }
                        hasher.update(&buf[..n]);
                        out.write_all(&buf[..n])?;
                        done += n as u64;
                        progress(done, total);
                    }
                    out.flush()?;
                    Ok(())
                })();
                drop(out);
                if let Err(e) = written {
                    let _ = fs::remove_file(&part);
                    return Err(e);
                }
                if total.is_some_and(|t| t != done) {
                    let _ = fs::remove_file(&part);
                    return Err(Error::Network("the download ended early".into()));
                }
                fs::rename(&part, dest)?;
                let hex: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
                return Ok((200, done, hex));
            }
        })();
        match result {
            Ok((200, bytes, hex)) => {
                self.record(&req, &host, Some(200), bytes as usize, false, None);
                Ok(hex)
            }
            Ok((status, _, _)) => {
                self.record(&req, &host, Some(status), 0, false, None);
                Err(Error::Network(format!("the server answered {status}")))
            }
            Err(e) => {
                self.record(&req, &host, None, 0, false, Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// Opens a radio stream. Unlike everything else here this goes to any host: the address is one
    /// the person chose to play, and stations live all over the internet. It still needs the online
    /// switch, is logged like the rest, and accepts http as well as https (many stations have no
    /// https), which [`StreamHead::encrypted`] reports so the app can say so.
    pub fn open_stream(&self, url: &str, purpose: &str) -> Result<LiveBody> {
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let req = Request::new(url, purpose);
        let host = stream_host(url, self.allow_local.load(Ordering::Relaxed)).inspect_err(|e| self.record(&req, "", None, 0, false, Some(e.to_string())))?;
        let mut current = url.to_string();
        let mut redirects = 0u8;
        let result = (|| -> Result<(u16, LiveBody)> {
            loop {
                stream_host(&current, self.allow_local.load(Ordering::Relaxed))?;
                let resp = self
                    .stream_agent
                    .get(&current)
                    .header("Icy-MetaData", "1")
                    .header("Accept", "audio/*, application/ogg, */*;q=0.5")
                    .call()
                    .map_err(|e| Error::Network(stream_error(&e.to_string())))?;
                let status = resp.status().as_u16();
                if matches!(status, 301 | 302 | 303 | 307 | 308) {
                    let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
                    let Some(loc) = loc else { return Err(Error::Network("redirect without a target".into())) };
                    if redirects >= STREAM_REDIRECTS {
                        return Err(Error::Network("too many redirects".into()));
                    }
                    redirects += 1;
                    current = absolute_location(&current, &loc);
                    continue;
                }
                if status != 200 {
                    return Ok((status, LiveBody { head: StreamHead::default(), reader: Box::new(std::io::empty()) }));
                }
                let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string()).filter(|v| !v.is_empty());
                let head = StreamHead {
                    url: current.clone(),
                    content_type: header("content-type").unwrap_or_default().split(';').next().unwrap_or("").trim().to_ascii_lowercase(),
                    icy_name: header("icy-name"),
                    icy_genre: header("icy-genre"),
                    icy_bitrate: header("icy-br").and_then(|v| v.split(',').next().and_then(|n| n.trim().parse().ok())),
                    icy_metaint: header("icy-metaint").and_then(|v| v.parse().ok()),
                    encrypted: current.to_ascii_lowercase().starts_with("https://"),
                };
                let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
                return Ok((200, LiveBody { head, reader: Box::new(reader) }));
            }
        })();
        match result {
            Ok((200, body)) => {
                self.record(&req, &host, Some(200), 0, false, None);
                Ok(body)
            }
            Ok((status, _)) => {
                self.record(&req, &host, Some(status), 0, false, None);
                Err(Error::Network(format!("the station answered {status}")))
            }
            Err(e) => {
                self.record(&req, &host, None, 0, false, Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// One GET with redirects (checked against the host list) and a single retry after 429/503.
    fn fetch(&self, req: &Request) -> Result<Reply> {
        let mut url = req.url.to_string();
        let accept = req.accept.unwrap_or("application/json");
        let limit = req.max_body.unwrap_or(MAX_BODY_BYTES);
        let mut redirects = 0u8;
        let mut retried = false;
        loop {
            self.check_url(&url)?;
            let mut resp = match req.post_json {
                Some(body) => self.agent.post(&url).header("Accept", accept).header("Content-Type", "application/json").send(body),
                None => self.agent.get(&url).header("Accept", accept).call(),
            }
            .map_err(|e| Error::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            if matches!(status, 301 | 302 | 307 | 308) && req.post_json.is_some() {
                return Err(Error::Network("the service redirected a request that cannot be repeated".into()));
            }
            if matches!(status, 301 | 302 | 307 | 308) {
                let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string);
                let Some(loc) = loc else { return Err(Error::Network("redirect without a target".into())) };
                if redirects >= MAX_REDIRECTS {
                    return Err(Error::Network("too many redirects".into()));
                }
                redirects += 1;
                url = if loc.starts_with('/') {
                    let uri: ureq::http::Uri = url.parse().map_err(|_| Error::Network("bad address".into()))?;
                    format!("{}://{}{}", uri.scheme_str().unwrap_or("https"), uri.authority().map(|a| a.as_str()).unwrap_or(""), loc)
                } else {
                    loc
                };
                continue;
            }
            if matches!(status, 429 | 503) && !retried {
                retried = true;
                let wait = resp
                    .headers()
                    .get("retry-after")
                    .and_then(|v| v.to_str().ok())
                    .and_then(|v| v.trim().parse::<u64>().ok())
                    .unwrap_or(1)
                    .clamp(1, 3);
                std::thread::sleep(Duration::from_secs(wait));
                continue;
            }
            let body = resp
                .body_mut()
                .with_config()
                .limit(limit)
                .lossy_utf8(true)
                .read_to_string()
                .map_err(|e| Error::Network(e.to_string()))?;
            return Ok(Reply { status, body, cached: false });
        }
    }
}

/// The host of a stream address, after checking it is http(s) and not an address on this computer
/// or the local network (a station list must not be able to point Redshift at those).
fn stream_host(url: &str, allow_local: bool) -> Result<String> {
    let uri: ureq::http::Uri = url.parse().map_err(|_| Error::Network("not a valid address".into()))?;
    let scheme = uri.scheme_str().unwrap_or("").to_ascii_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(Error::Network(format!("{scheme}:// streams are not supported")));
    }
    let host = uri.host().ok_or_else(|| Error::Network("the address has no host".into()))?.to_ascii_lowercase();
    if !allow_local {
        let bare = host.trim_start_matches('[').trim_end_matches(']');
        let local_name = bare == "localhost" || bare.ends_with(".local") || bare.ends_with(".internal");
        let local_ip = bare.parse::<std::net::IpAddr>().is_ok_and(|ip| match ip {
            std::net::IpAddr::V4(v) => v.is_loopback() || v.is_private() || v.is_link_local() || v.is_unspecified(),
            std::net::IpAddr::V6(v) => v.is_loopback() || v.is_unspecified() || (v.segments()[0] & 0xfe00) == 0xfc00 || (v.segments()[0] & 0xffc0) == 0xfe80,
        });
        if local_name || local_ip {
            return Err(Error::Network("streams from this computer or the local network are not played from the radio".into()));
        }
    }
    Ok(host)
}

fn absolute_location(base: &str, loc: &str) -> String {
    if loc.starts_with('/') {
        if let Ok(uri) = base.parse::<ureq::http::Uri>() {
            return format!("{}://{}{}", uri.scheme_str().unwrap_or("http"), uri.authority().map(|a| a.as_str()).unwrap_or(""), loc);
        }
    }
    loc.to_string()
}

/// Old Shoutcast servers answer with "ICY 200 OK" instead of an HTTP status line, which the HTTP
/// client rejects; say what that means instead of showing its parser error.
fn stream_error(msg: &str) -> String {
    let lower = msg.to_ascii_lowercase();
    if lower.contains("version") || lower.contains("icy") || lower.contains("invalid") && lower.contains("status") {
        return format!("this station speaks an old streaming protocol that Redshift can't read ({msg})");
    }
    msg.to_string()
}

/// Where the cache for network replies lives inside the app's data folder.
pub fn cache_dir_in(data_dir: &Path) -> PathBuf {
    data_dir.join("net-cache")
}

#[cfg(test)]
pub(crate) mod testserver {
    //! A tiny HTTP server on the loopback address that plays back canned replies, so the network
    //! layer can be tested without the internet.
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

    pub struct Canned {
        pub status: u16,
        pub headers: Vec<(String, String)>,
        pub body: String,
    }

    pub fn reply(status: u16, body: &str) -> Canned {
        Canned { status, headers: vec![], body: body.to_string() }
    }

    pub struct Server {
        pub base: String,
        pub hits: Arc<AtomicUsize>,
        pub requests: Arc<parking_lot::Mutex<Vec<String>>>,
    }

    /// Replies are given out in order; the last one repeats.
    pub fn start(replies: Vec<Canned>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let hits = Arc::new(AtomicUsize::new(0));
        let requests = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let (h, r) = (hits.clone(), requests.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                // Read the whole request, body included. Replying and closing with part of a POST
                // still unread makes Windows reset the connection and drop the reply (os error 10054).
                let mut data: Vec<u8> = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = stream.read(&mut buf).unwrap_or(0);
                    if n == 0 {
                        break;
                    }
                    data.extend_from_slice(&buf[..n]);
                    let Some(end) = data.windows(4).position(|w| w == b"\r\n\r\n") else { continue };
                    let head = String::from_utf8_lossy(&data[..end]).to_lowercase();
                    let want = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:").and_then(|v| v.trim().parse::<usize>().ok()))
                        .unwrap_or(0);
                    if data.len() >= end + 4 + want {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&data).to_string();
                r.lock().push(text);
                let i = h.fetch_add(1, Ordering::SeqCst).min(replies.len() - 1);
                let c = &replies[i];
                let mut out = format!("HTTP/1.1 {} X\r\nContent-Length: {}\r\nConnection: close\r\nContent-Type: application/json\r\n", c.status, c.body.len());
                for (k, v) in &c.headers {
                    out.push_str(&format!("{k}: {v}\r\n"));
                }
                out.push_str("\r\n");
                out.push_str(&c.body);
                let _ = stream.write_all(out.as_bytes());
                let _ = stream.flush();
                let _ = stream.shutdown(std::net::Shutdown::Write);
            }
        });
        Server { base: format!("http://127.0.0.1:{port}"), hits, requests }
    }
}

#[cfg(test)]
mod tests {
    use super::testserver::*;
    use super::*;
    use std::sync::atomic::Ordering;

    fn net(dir: &Path) -> Net {
        let n = Net::new(dir.join("cache"), "Redshift-test/0.0", &["127.0.0.1"]);
        n.set_enabled(true);
        n
    }

    #[test]
    fn starts_disabled_and_sends_nothing() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, "{}")]);
        let n = Net::new(d.path().join("c"), "t", &["127.0.0.1"]);
        assert!(!n.enabled());
        let e = n.get(&Request::new(&format!("{}/x", s.base), "test")).unwrap_err();
        assert!(matches!(e, Error::Offline));
        assert_eq!(s.hits.load(Ordering::SeqCst), 0);
        assert!(n.log().is_empty(), "nothing was attempted, so nothing is logged");
    }

    #[test]
    fn fetches_and_logs_when_enabled() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"ok":true}"#)]);
        let n = net(d.path());
        let r = n.get(&Request::new(&format!("{}/api?q=1", s.base), "Lyrics")).unwrap();
        assert_eq!((r.status, r.body.as_str(), r.cached), (200, r#"{"ok":true}"#, false));
        let log = n.log();
        assert_eq!(log.len(), 1);
        assert_eq!((log[0].purpose.as_str(), log[0].host.as_str(), log[0].status, log[0].cached), ("Lyrics", "127.0.0.1", Some(200), false));
        assert!(log[0].url.ends_with("/api?q=1"));
        let sent = s.requests.lock()[0].to_lowercase();
        assert!(sent.contains("user-agent: redshift-test/0.0"), "{sent}");
    }

    #[test]
    fn caches_hits_and_misses() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, "one"), reply(404, "none"), reply(200, "two")]);
        let n = net(d.path());
        let ttl = Duration::from_secs(60);
        let u = format!("{}/a", s.base);
        assert_eq!(n.get(&Request::new(&u, "t").cached_for(ttl)).unwrap().body, "one");
        let again = n.get(&Request::new(&u, "t").cached_for(ttl)).unwrap();
        assert_eq!((again.body.as_str(), again.cached), ("one", true));
        assert_eq!(s.hits.load(Ordering::SeqCst), 1, "second call came from the cache");
        // a miss is remembered too, so the same missing song is not asked about again
        let m = format!("{}/missing", s.base);
        assert_eq!(n.get(&Request::new(&m, "t").cached_for(ttl)).unwrap().status, 404);
        assert_eq!(n.get(&Request::new(&m, "t").cached_for(ttl)).unwrap().status, 404);
        assert_eq!(s.hits.load(Ordering::SeqCst), 2);
        // without a ttl nothing is read from or written to the cache
        let u2 = format!("{}/b", s.base);
        n.get(&Request::new(&u2, "t")).unwrap();
        n.get(&Request::new(&u2, "t")).unwrap();
        assert_eq!(s.hits.load(Ordering::SeqCst), 4);
        assert_eq!(n.cache_stats().0, 2);
        assert_eq!(n.clear_cache(), 2);
        assert_eq!(n.cache_stats(), (0, 0));
    }

    #[test]
    fn expired_cache_entries_are_refetched() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, "old"), reply(200, "new")]);
        let n = net(d.path());
        let u = format!("{}/a", s.base);
        n.get(&Request::new(&u, "t").cached_for(Duration::from_secs(60))).unwrap();
        // a zero-second ttl treats anything older than this very second as stale
        std::thread::sleep(Duration::from_millis(1100));
        let r = n.get(&Request::new(&u, "t").cached_for(Duration::from_secs(0))).unwrap();
        assert_eq!(r.body, "new");
    }

    #[test]
    fn misses_expire_sooner_than_hits() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(404, "none"), reply(200, "now there")]);
        let n = net(d.path());
        let u = format!("{}/a", s.base);
        let req = || Request::new(&u, "t").cached_for(Duration::from_secs(3600)).misses_for(Duration::from_secs(0));
        assert_eq!(n.get(&req()).unwrap().status, 404);
        std::thread::sleep(Duration::from_millis(1100));
        assert_eq!(n.get(&req()).unwrap().body, "now there");
    }

    #[test]
    fn errors_and_server_failures_are_not_cached() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(500, "boom"), reply(200, "fine")]);
        let n = net(d.path());
        let u = format!("{}/a", s.base);
        let ttl = Duration::from_secs(60);
        assert_eq!(n.get(&Request::new(&u, "t").cached_for(ttl)).unwrap().status, 500);
        assert_eq!(n.get(&Request::new(&u, "t").cached_for(ttl)).unwrap().body, "fine");
    }

    #[test]
    fn refuses_other_hosts_and_plain_http_off_loopback() {
        let d = tempfile::tempdir().unwrap();
        let n = net(d.path());
        for u in ["https://example.com/x", "http://example.com/x", "ftp://127.0.0.1/x", "not a url", "https://127.0.0.1.evil.test/x"] {
            assert!(matches!(n.get(&Request::new(u, "t")), Err(Error::Network(_))), "{u}");
        }
        let log = n.log();
        assert!(log.iter().all(|r| r.error.is_some() && r.status.is_none()));
        let strict = Net::new(d.path().join("c2"), "t", &["lrclib.net"]);
        strict.set_enabled(true);
        let e = strict.get(&Request::new("http://lrclib.net/api/get", "t")).unwrap_err().to_string();
        assert!(e.contains("https"), "{e}");
    }

    #[test]
    fn retries_once_after_503_then_gives_up() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![
            Canned { status: 503, headers: vec![("Retry-After".into(), "1".into())], body: "busy".into() },
            reply(200, "ok"),
        ]);
        let n = net(d.path());
        let t = std::time::Instant::now();
        assert_eq!(n.get(&Request::new(&format!("{}/a", s.base), "t")).unwrap().body, "ok");
        assert!(t.elapsed() >= Duration::from_millis(900));
        assert_eq!(s.hits.load(Ordering::SeqCst), 2);

        let s2 = start(vec![Canned { status: 503, headers: vec![("Retry-After".into(), "1".into())], body: "busy".into() }]);
        let r = n.get(&Request::new(&format!("{}/a", s2.base), "t")).unwrap();
        assert_eq!(r.status, 503);
        assert_eq!(s2.hits.load(Ordering::SeqCst), 2, "one retry, no more");
    }

    #[test]
    fn follows_redirects_only_within_the_allowed_hosts() {
        let d = tempfile::tempdir().unwrap();
        let target = start(vec![reply(200, "landed")]);
        let s = start(vec![Canned { status: 301, headers: vec![("Location".into(), format!("{}/there", target.base))], body: String::new() }]);
        let n = net(d.path());
        assert_eq!(n.get(&Request::new(&format!("{}/a", s.base), "t")).unwrap().body, "landed");

        let away = start(vec![Canned { status: 302, headers: vec![("Location".into(), "https://evil.example/x".into())], body: String::new() }]);
        let e = n.get(&Request::new(&format!("{}/a", away.base), "t")).unwrap_err().to_string();
        assert!(e.contains("evil.example"), "{e}");

        let rel = start(vec![Canned { status: 308, headers: vec![("Location".into(), "/next".into())], body: String::new() }, reply(200, "rel")]);
        // the second reply is served for any path on the same server
        assert_eq!(n.get(&Request::new(&format!("{}/a", rel.base), "t")).unwrap().body, "rel");

        let looping = start(vec![Canned { status: 302, headers: vec![("Location".into(), "/again".into())], body: String::new() }]);
        let e = n.get(&Request::new(&format!("{}/a", looping.base), "t")).unwrap_err().to_string();
        assert!(e.contains("redirects"), "{e}");
    }

    #[test]
    fn oversize_replies_are_an_error() {
        let d = tempfile::tempdir().unwrap();
        let big = "x".repeat(MAX_BODY_BYTES as usize + 10);
        let s = start(vec![reply(200, &big)]);
        let n = net(d.path());
        assert!(matches!(n.get(&Request::new(&format!("{}/a", s.base), "t")), Err(Error::Network(_))));
        assert!(n.log()[0].error.is_some());
    }

    #[test]
    fn a_download_is_written_hashed_and_reported() {
        let d = tempfile::tempdir().unwrap();
        let s = testserver::start(vec![testserver::reply(200, "hello update")]);
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        let dest = d.path().join("up").join("setup.exe");
        let mut seen = Vec::new();
        let hex = n.download(&format!("{}/f", s.base), "Update download", &dest, 1024, &mut |done, total| seen.push((done, total))).unwrap();
        assert_eq!(std::fs::read_to_string(&dest).unwrap(), "hello update");
        // sha256("hello update")
        assert_eq!(hex.len(), 64);
        assert_eq!(seen.last().unwrap().0, 12);
        assert!(!dest.with_extension("part").exists());
        assert_eq!(n.log()[0].purpose, "Update download");
    }

    #[test]
    fn a_download_that_is_too_big_or_fails_leaves_nothing_behind() {
        let d = tempfile::tempdir().unwrap();
        let s = testserver::start(vec![testserver::reply(200, &"x".repeat(5000)), testserver::reply(404, "")]);
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        let dest = d.path().join("setup.exe");
        assert!(n.download(&format!("{}/f", s.base), "Update download", &dest, 1000, &mut |_, _| {}).is_err());
        assert!(!dest.exists() && !dest.with_extension("part").exists());
        assert!(n.download(&format!("{}/f", s.base), "Update download", &dest, 1000, &mut |_, _| {}).is_err());
        assert!(!dest.exists());
        n.set_enabled(false);
        assert!(matches!(n.download(&format!("{}/f", s.base), "x", &dest, 10, &mut |_, _| {}), Err(Error::Offline)));
    }

    #[test]
    fn connection_failure_is_logged_not_panicked() {
        let d = tempfile::tempdir().unwrap();
        let port = {
            let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            l.local_addr().unwrap().port()
        };
        let n = net(d.path());
        assert!(matches!(n.get(&Request::new(&format!("http://127.0.0.1:{port}/a"), "t")), Err(Error::Network(_))));
        assert_eq!(n.log().len(), 1);
    }

    #[test]
    fn log_is_newest_first_and_bounded() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, "x")]);
        let n = net(d.path());
        for i in 0..(LOG_LEN + 5) {
            n.get(&Request::new(&format!("{}/{i}", s.base), "t")).unwrap();
        }
        let log = n.log();
        assert_eq!(log.len(), LOG_LEN);
        assert!(log[0].url.ends_with(&format!("/{}", LOG_LEN + 4)));
        n.clear_log();
        assert!(n.log().is_empty());
    }

    #[test]
    fn query_values_are_percent_encoded() {
        assert_eq!(encode_query("Bohemian Rhapsody"), "Bohemian%20Rhapsody");
        assert_eq!(encode_query("AC/DC & Co."), "AC%2FDC%20%26%20Co.");
        assert_eq!(encode_query("花がら"), "%E8%8A%B1%E3%81%8C%E3%82%89");
        assert_eq!(encode_query("a-b_c.d~e"), "a-b_c.d~e");
    }

    /// Talks to the real GitHub API to prove that TLS and certificate checking work end to end: any
    /// genuine HTTP reply (even a 403 from a restricted network) means the secure connection was
    /// made. Run with `cargo test -p redshift-core live_ -- --ignored --nocapture`.
    #[test]
    #[ignore = "needs the internet"]
    fn live_tls_handshake() {
        let d = tempfile::tempdir().unwrap();
        let n = Net::new(d.path().join("c"), "Redshift-live-test", &[super::updates::HOST]);
        n.set_enabled(true);
        let r = n.get(&Request::new("https://api.github.com/repos/mysticninja487/redshift/releases/latest", "live")).unwrap();
        println!("{} {}", r.status, r.body.chars().take(120).collect::<String>());
        assert!((200..500).contains(&r.status));
        assert!(n.log()[0].error.is_none());
    }
}
