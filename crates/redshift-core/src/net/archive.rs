//! The Internet Archive (archive.org) as a source of lossless music: live concert recordings, net
//! labels, old records. Search and item details come from its public JSON interfaces; the audio is
//! the item's own file (FLAC where there is one), read in pieces like a song on the person's server.
//!
//! What is sent: the search words and filters, an item's identifier, and the app's name and version
//! (as the User-Agent), as ordinary HTTPS requests, and only when the person searches, opens an item
//! or plays something. Nothing is sent about the person. The Archive answers file requests from
//! other `archive.org` servers (a redirect), so any `*.archive.org` host over https is accepted for
//! that, and nothing else.

use std::collections::HashMap;
use std::io::Read;
use std::collections::VecDeque;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{absolute_location, encode_query, Net, RequestRecord, ServerBody};
use crate::error::{Error, Result};
use crate::util::archive_song_path;

pub const OFFICIAL_BASE: &str = "https://archive.org";
/// Items per page of search results.
pub const PAGE_SIZE: usize = 40;
const MAX_JSON: u64 = 12 * 1024 * 1024;
const REDIRECTS: u8 = 5;
/// How long the Archive gets to start answering a search or an item's details. Its search can take
/// many seconds when it is busy, and giving up early only means asking again from the start.
const JSON_PATIENCE: Duration = Duration::from_secs(45);
/// A file or picture should start arriving quickly, or another try may do better.
const FILE_PATIENCE: Duration = Duration::from_secs(12);
/// How long a remembered answer is used without asking again.
const SEARCH_FRESH: Duration = Duration::from_secs(20 * 60);
const ITEM_FRESH: Duration = Duration::from_secs(30 * 60);

/// The last few search results and item details, newest last.
#[derive(Default)]
pub(super) struct Recent {
    searches: VecDeque<(String, Instant, ArchiveSearch)>,
    items: VecDeque<(String, Instant, Value)>,
}

impl Recent {
    fn search(&self, key: &str) -> Option<ArchiveSearch> {
        self.searches.iter().find(|(k, at, _)| k == key && at.elapsed() < SEARCH_FRESH).map(|(_, _, v)| v.clone())
    }
    fn put_search(&mut self, key: String, v: ArchiveSearch) {
        self.searches.retain(|(k, _, _)| *k != key);
        self.searches.push_back((key, Instant::now(), v));
        while self.searches.len() > 24 {
            self.searches.pop_front();
        }
    }
    fn item(&self, id: &str) -> Option<Value> {
        self.items.iter().find(|(k, at, _)| k == id && at.elapsed() < ITEM_FRESH).map(|(_, _, v)| v.clone())
    }
    fn put_item(&mut self, id: String, v: Value) {
        self.items.retain(|(k, _, _)| *k != id);
        self.items.push_back((id, Instant::now(), v));
        while self.items.len() > 10 {
            self.items.pop_front();
        }
    }
}
/// The collections the Archive screen offers as a quick filter.
pub const COLLECTIONS: [&str; 3] = ["etree", "netlabels", "opensource_audio"];

/// What to look for.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveQuery {
    pub text: String,
    /// Only items that have a lossless file (FLAC, WAV, Apple Lossless).
    pub lossless_only: bool,
    /// One of [`COLLECTIONS`], or empty for everything.
    pub collection: String,
    /// `popular` (most downloaded), `newest` or `relevance`.
    pub sort: String,
    /// First page is 1.
    pub page: u32,
}

/// An item in a list of results.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArchiveHit {
    pub identifier: String,
    pub title: String,
    pub creator: String,
    pub year: Option<u32>,
    pub downloads: u64,
    /// The kinds of audio file the item has, best first ("Flac", "VBR MP3").
    pub formats: Vec<String>,
    /// The item has at least one lossless audio file.
    pub lossless: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ArchiveSearch {
    pub hits: Vec<ArchiveHit>,
    pub total: u64,
    pub page: u32,
    pub page_size: usize,
}

/// One playable song in an item: the best file the item has for that track.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ArchiveTrack {
    pub identifier: String,
    /// The file's name inside the item.
    pub file: String,
    /// The queue path (`archive://identifier/file`).
    pub path: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub track: Option<u32>,
    pub year: Option<u32>,
    pub duration_secs: f64,
    /// The Archive's own name for the kind of file ("Flac", "24bit Flac", "VBR MP3").
    pub format: String,
    /// File extension: `flac`, `mp3`, ...
    pub suffix: String,
    pub lossless: bool,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub size: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ArchiveItem {
    pub identifier: String,
    pub title: String,
    pub creator: String,
    pub date: String,
    /// The uploader's description, as plain text and shortened.
    pub description: String,
    pub license_url: Option<String>,
    pub tracks: Vec<ArchiveTrack>,
    /// Songs left out because they only come as lossy files and "Lossless only" was on.
    pub hidden_lossy: usize,
    /// `https://archive.org/details/<identifier>`, to credit the source.
    pub details_url: String,
}

// ---- reading the Archive's JSON ----

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.trim().to_string(),
        Some(Value::Number(n)) => n.to_string(),
        // Several creators, titles or descriptions come as a list.
        Some(Value::Array(a)) => a.iter().filter_map(|x| x.as_str()).map(str::trim).filter(|s| !s.is_empty()).collect::<Vec<_>>().join(", "),
        _ => String::new(),
    }
}

fn first_text(v: &Value, keys: &[&str]) -> String {
    keys.iter().map(|k| text(v, k)).find(|s| !s.is_empty()).unwrap_or_default()
}

fn number(v: &Value, key: &str) -> Option<u64> {
    match v.get(key)? {
        Value::Number(n) => n.as_u64().or_else(|| n.as_f64().map(|f| f as u64)),
        Value::String(s) => s.trim().parse::<u64>().ok().or_else(|| s.trim().parse::<f64>().ok().map(|f| f as u64)),
        _ => None,
    }
}

/// `245.31`, `4:05` or `1:02:03` as seconds.
fn parse_length(s: &str) -> f64 {
    let s = s.trim();
    if s.contains(':') {
        s.split(':').try_fold(0.0, |acc, part| part.trim().parse::<f64>().ok().map(|p| acc * 60.0 + p)).unwrap_or(0.0)
    } else {
        s.parse::<f64>().unwrap_or(0.0)
    }
}

/// Plain text from the uploader's HTML description.
fn plain_text(html: &str, max: usize) -> String {
    let mut out = String::with_capacity(html.len().min(max + 16));
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => {
                in_tag = false;
                out.push(' ');
            }
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    let out = out.replace("&nbsp;", " ").replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&#39;", "'").replace("&amp;", "&");
    let collapsed = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() <= max {
        collapsed
    } else {
        let cut: String = collapsed.chars().take(max).collect();
        format!("{}…", cut.trim_end())
    }
}

/// What a kind of file is worth: higher is better.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Kind {
    rank: u8,
    lossless: bool,
    depth: Option<u32>,
}

/// What the Archive's name for a kind of file means to Redshift, or `None` when it is not something
/// the player can decode (or not a song at all: a sample, a picture, a spectrogram).
fn kind_of(format: &str, file: &str) -> Option<Kind> {
    let ext = file.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
    if !matches!(ext.as_str(), "flac" | "wav" | "m4a" | "mp3" | "ogg" | "oga" | "opus" | "aac") {
        return None;
    }
    let f = format.trim().to_ascii_lowercase();
    let lossless = |rank, depth| Some(Kind { rank, lossless: true, depth });
    let lossy = |rank| Some(Kind { rank, lossless: false, depth: None });
    if f.contains("sample") {
        return None;
    }
    if f.contains("24bit") && f.contains("flac") {
        lossless(100, Some(24))
    } else if f.contains("flac") {
        lossless(90, None)
    } else if f.contains("apple lossless") {
        lossless(80, None)
    } else if f == "wave" || f == "wav" {
        lossless(70, None)
    } else if f == "opus" {
        lossy(55)
    } else if f == "vbr mp3" {
        lossy(50)
    } else if f == "ogg vorbis" {
        lossy(45)
    } else if f.contains("advanced audio coding") || f == "aac" {
        lossy(40)
    } else if f == "mp3" {
        lossy(35)
    } else if f.contains("128kbps") {
        lossy(30)
    } else if f.contains("64kbps") {
        lossy(20)
    } else if f.contains("mp3") {
        lossy(30)
    } else {
        None
    }
}

/// Whether any of the formats an item lists is a lossless one the player can use.
pub fn formats_have_lossless(formats: &[String]) -> bool {
    formats.iter().any(|f| {
        let f = f.to_ascii_lowercase();
        f.contains("flac") || f.contains("apple lossless") || f == "wave" || f == "wav"
    })
}

/// Words that mean the same in the Archive's search syntax and out of it: letters and digits only,
/// so nothing a person types can be read as an operator.
fn search_words(text: &str) -> Vec<String> {
    let cleaned: String = text.chars().map(|c| if c.is_alphanumeric() || c == '\'' { c } else { ' ' }).collect();
    cleaned.split_whitespace().map(|w| w.to_lowercase()).take(12).collect()
}

fn build_query(q: &ArchiveQuery) -> String {
    let mut parts = vec!["mediatype:audio".to_string()];
    let words = search_words(&q.text);
    if !words.is_empty() {
        parts.push(format!("({})", words.join(" AND ")));
    }
    if COLLECTIONS.contains(&q.collection.as_str()) {
        parts.push(format!("collection:{}", q.collection));
    }
    if q.lossless_only {
        parts.push(r#"format:("Flac" OR "24bit Flac" OR "WAVE" OR "Apple Lossless Audio")"#.to_string());
    }
    parts.join(" AND ")
}

fn valid_identifier(id: &str) -> bool {
    !id.is_empty() && id.len() <= 200 && id.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
}

/// A path inside an item, percent-encoded piece by piece (the `/` between folders stays).
fn encode_file_path(file: &str) -> String {
    file.split('/').map(encode_query).collect::<Vec<_>>().join("/")
}

/// The leading number of `"03"` or `"3/12"`.
fn track_number(s: &str) -> Option<u32> {
    let digits: String = s.trim().chars().take_while(|c| c.is_ascii_digit()).collect();
    digits.parse().ok().filter(|n| *n > 0)
}

/// Compares names so that `t2` comes before `t10`.
fn natural_cmp(a: &str, b: &str) -> std::cmp::Ordering {
    let chunks = |s: &str| -> Vec<(bool, String)> {
        let mut out: Vec<(bool, String)> = Vec::new();
        for c in s.to_lowercase().chars() {
            let d = c.is_ascii_digit();
            match out.last_mut() {
                Some((kind, run)) if *kind == d => run.push(c),
                _ => out.push((d, c.to_string())),
            }
        }
        out
    };
    let (x, y) = (chunks(a), chunks(b));
    for (p, q) in x.iter().zip(y.iter()) {
        let o = if p.0 && q.0 {
            let (pn, qn) = (p.1.trim_start_matches('0'), q.1.trim_start_matches('0'));
            pn.len().cmp(&qn.len()).then_with(|| pn.cmp(qn))
        } else {
            p.1.cmp(&q.1)
        };
        if o != std::cmp::Ordering::Equal {
            return o;
        }
    }
    x.len().cmp(&y.len())
}

/// A readable title from a file name when the item has no title for the track.
fn title_from_file(file: &str) -> String {
    let base = file.rsplit('/').next().unwrap_or(file);
    let stem = base.rsplit_once('.').map_or(base, |(s, _)| s);
    stem.replace('_', " ").trim().to_string()
}

struct ItemFacts<'a> {
    identifier: &'a str,
    title: &'a str,
    creator: &'a str,
    year: Option<u32>,
}

/// The songs in an item's `files`: one per track, using the best file there is for it.
fn tracks_from(files: &[Value], facts: &ItemFacts, lossless_only: bool) -> (Vec<ArchiveTrack>, usize) {
    // The same song is often there as FLAC, MP3 and Ogg with one name apart from the ending.
    let mut best: HashMap<String, (Kind, &Value)> = HashMap::new();
    for f in files {
        let name = text(f, "name");
        if name.is_empty() {
            continue;
        }
        let Some(kind) = kind_of(&text(f, "format"), &name) else { continue };
        let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s).to_lowercase();
        match best.get(&stem) {
            Some((k, _)) if k.rank >= kind.rank => {}
            _ => {
                best.insert(stem, (kind, f));
            }
        }
    }
    let mut hidden = 0;
    let mut tracks: Vec<ArchiveTrack> = Vec::new();
    for (kind, f) in best.into_values() {
        if lossless_only && !kind.lossless {
            hidden += 1;
            continue;
        }
        let file = text(f, "name");
        let bitrate = number(f, "bitrate").map(|b| if b > 10_000 { b / 1000 } else { b }).and_then(|b| u32::try_from(b).ok()).filter(|b| *b > 0 && !kind.lossless);
        tracks.push(ArchiveTrack {
            identifier: facts.identifier.to_string(),
            path: archive_song_path(facts.identifier, &file).to_string_lossy().into_owned(),
            title: Some(text(f, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| title_from_file(&file)),
            artist: Some(first_text(f, &["creator", "artist"])).filter(|t| !t.is_empty()).unwrap_or_else(|| facts.creator.to_string()),
            album: Some(text(f, "album")).filter(|t| !t.is_empty()).unwrap_or_else(|| facts.title.to_string()),
            track: track_number(&text(f, "track")),
            year: facts.year,
            duration_secs: parse_length(&text(f, "length")),
            format: text(f, "format"),
            suffix: file.rsplit('.').next().unwrap_or("").to_ascii_lowercase(),
            lossless: kind.lossless,
            bitrate_kbps: bitrate,
            sample_rate: number(f, "sample_rate").and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0),
            bit_depth: kind.depth,
            size: number(f, "size").unwrap_or(0),
            file,
        });
    }
    // Track numbers decide the order when every song has its own; otherwise the file names do
    // (`d1t01`, `d1t02`, `d2t01` ...), because numbers that restart on each disc would interleave.
    let numbered_uniquely = !tracks.is_empty() && tracks.iter().all(|t| t.track.is_some()) && {
        let mut seen: Vec<u32> = tracks.iter().filter_map(|t| t.track).collect();
        seen.sort_unstable();
        seen.windows(2).all(|w| w[0] != w[1])
    };
    if numbered_uniquely {
        tracks.sort_by_key(|t| t.track);
    } else {
        tracks.sort_by(|a, b| natural_cmp(&a.file, &b.file));
    }
    (tracks, hidden)
}

fn year_of(date: &str) -> Option<u32> {
    date.trim().get(..4).and_then(|y| y.parse().ok()).filter(|y| (1000..=2999).contains(y))
}

impl Net {
    /// Points the Internet Archive requests somewhere else, for tests that run a fake on this
    /// computer. Never called by the app.
    #[doc(hidden)]
    pub fn set_archive_base_for_tests(&self, base: &str) {
        *self.archive_base.write() = base.trim_end_matches('/').to_string();
    }

    fn archive_base(&self) -> String {
        self.archive_base.read().clone()
    }

    /// Only archive.org and the servers it hands files to (`*.archive.org`), over https.
    fn check_archive_url(&self, url: &str) -> Result<()> {
        let uri: ureq::http::Uri = url.parse().map_err(|_| Error::Network("not a valid address".into()))?;
        let host = uri.host().unwrap_or("").to_ascii_lowercase();
        if uri.scheme_str() == Some("https") && (host == "archive.org" || host.ends_with(".archive.org")) {
            return Ok(());
        }
        let base = self.archive_base.read();
        if *base != OFFICIAL_BASE {
            if let Ok(b) = base.parse::<ureq::http::Uri>() {
                if b.authority() == uri.authority() && b.scheme() == uri.scheme() {
                    return Ok(());
                }
            }
        }
        Err(Error::Network("that address is not part of the Internet Archive".into()))
    }

    fn record_archive(&self, purpose: &str, url: &str, status: Option<u16>, bytes: usize, error: Option<String>) {
        let host = url.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("").to_string();
        let mut log = self.log.lock();
        if log.len() >= super::LOG_LEN {
            log.pop_front();
        }
        log.push_back(RequestRecord { time: super::now_secs(), purpose: purpose.to_string(), host, url: url.to_string(), status, bytes, cached: false, error });
    }

    /// Opens an Archive address for reading, optionally only the bytes `start..=end`, following
    /// redirects (each one is checked). `log` adds a line to the request log; the many small reads
    /// while a song plays pass false.
    pub fn archive_open(&self, url: &str, accept: &str, range: Option<(u64, u64)>, purpose: &str, log: bool) -> Result<ServerBody> {
        self.archive_open_within(url, accept, range, purpose, log, FILE_PATIENCE)
    }

    /// [`archive_open`](Self::archive_open) with a chosen time for the Archive to start answering.
    fn archive_open_within(&self, url: &str, accept: &str, range: Option<(u64, u64)>, purpose: &str, log: bool, patience: Duration) -> Result<ServerBody> {
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let result = (|| -> Result<ServerBody> {
            let mut current = url.to_string();
            let mut redirects = 0;
            loop {
                self.check_archive_url(&current)?;
                let mut req = self.stream_agent.get(&current).config().timeout_recv_response(Some(patience)).build().header("Accept", accept);
                if let Some((a, b)) = range {
                    req = req.header("Range", &format!("bytes={a}-{b}"));
                }
                let resp = req.call().map_err(|e| Error::Network(e.to_string()))?;
                let status = resp.status().as_u16();
                if matches!(status, 301 | 302 | 303 | 307 | 308) && redirects < REDIRECTS {
                    let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string).ok_or_else(|| Error::Network("redirect without a target".into()))?;
                    current = absolute_location(&current, &loc);
                    redirects += 1;
                    continue;
                }
                let header = |name: &str| resp.headers().get(name).and_then(|v| v.to_str().ok()).map(|v| v.trim().to_string());
                let content_type = header("content-type").unwrap_or_default().split(';').next().unwrap_or("").trim().to_ascii_lowercase();
                let total = header("content-range")
                    .and_then(|v| v.rsplit('/').next().and_then(|n| n.trim().parse::<u64>().ok()))
                    .or_else(|| if status == 200 { header("content-length").and_then(|n| n.parse().ok()) } else { None });
                let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
                return Ok(ServerBody { status, content_type, total, reader: Box::new(reader) });
            }
        })();
        match &result {
            Ok(b) if log => self.record_archive(purpose, url, Some(b.status), 0, None),
            Err(e) if log => self.record_archive(purpose, url, None, 0, Some(e.to_string())),
            _ => {}
        }
        result
    }

    /// Reads a JSON answer. One more try after a pause when the Archive says it is busy.
    fn archive_json(&self, url: &str, purpose: &str) -> Result<Value> {
        let mut last = Error::Network("no answer".into());
        for attempt in 0..2 {
            if attempt > 0 {
                std::thread::sleep(Duration::from_millis(1500));
            }
            let mut body = match self.archive_open_within(url, "application/json", None, purpose, false, JSON_PATIENCE) {
                Ok(b) => b,
                Err(Error::Offline) => return Err(Error::Offline),
                Err(e) => {
                    self.record_archive(purpose, url, None, 0, Some(e.to_string()));
                    // A search that already took this long will not do better at once: say so
                    // instead of making the person wait through the same time again.
                    let slow = e.to_string().to_ascii_lowercase().contains("timeout") || e.to_string().to_ascii_lowercase().contains("timed out");
                    if slow {
                        return Err(Error::Network("the Internet Archive took too long to answer. It is sometimes slow; try again in a moment".into()));
                    }
                    last = e;
                    continue;
                }
            };
            let mut buf = Vec::new();
            let read = body.reader.by_ref().take(MAX_JSON + 1).read_to_end(&mut buf);
            self.record_archive(purpose, url, Some(body.status), buf.len(), read.as_ref().err().map(|e| e.to_string()));
            if let Err(e) = read {
                last = Error::Network(e.to_string());
                continue;
            }
            match body.status {
                200 => {
                    if buf.len() as u64 > MAX_JSON {
                        return Err(Error::Network("the Internet Archive's answer was too large".into()));
                    }
                    return serde_json::from_slice(&buf).map_err(|_| Error::Network("the Internet Archive sent something that is not a list".into()));
                }
                429 | 500 | 502 | 503 | 504 => last = Error::Network(format!("the Internet Archive is busy ({})", body.status)),
                404 => return Err(Error::Network("the Internet Archive does not have that".into())),
                s => return Err(Error::Network(format!("the Internet Archive answered {s}"))),
            }
        }
        Err(last)
    }

    /// The address of a file inside an item.
    pub fn archive_stream_url(&self, identifier: &str, file: &str) -> Result<String> {
        if !valid_identifier(identifier) || file.is_empty() || file.contains("..") {
            return Err(Error::Network("not a file on the Internet Archive".into()));
        }
        Ok(format!("{}/download/{identifier}/{}", self.archive_base(), encode_file_path(file)))
    }

    /// Looks for audio items.
    pub fn archive_search(&self, q: &ArchiveQuery) -> Result<ArchiveSearch> {
        let page = q.page.clamp(1, 500);
        let has_words = !search_words(&q.text).is_empty();
        let sort = match q.sort.as_str() {
            "newest" => "addeddate desc",
            "relevance" if has_words => "",
            _ => "downloads desc",
        };
        let mut url = format!(
            "{}/advancedsearch.php?q={}&fl%5B%5D=identifier&fl%5B%5D=title&fl%5B%5D=creator&fl%5B%5D=year&fl%5B%5D=downloads&fl%5B%5D=format&rows={PAGE_SIZE}&page={page}&output=json",
            self.archive_base(),
            encode_query(&build_query(q))
        );
        if !sort.is_empty() {
            url.push_str(&format!("&sort%5B%5D={}", encode_query(sort)));
        }
        let cache_key = url.clone();
        if let Some(hit) = self.archive_cache.lock().search(&cache_key) {
            return Ok(hit);
        }
        let v = self.archive_json(&url, "Internet Archive: search")?;
        let response = v.get("response").ok_or_else(|| Error::Network("the Internet Archive's answer had no results part".into()))?;
        let hits = response
            .get("docs")
            .and_then(Value::as_array)
            .map(|docs| {
                docs.iter()
                    .filter_map(|d| {
                        let identifier = text(d, "identifier");
                        if !valid_identifier(&identifier) {
                            return None;
                        }
                        let mut formats: Vec<String> = d.get("format").and_then(Value::as_array).map(|a| a.iter().filter_map(|x| x.as_str()).map(str::to_string).collect()).unwrap_or_default();
                        // Only the kinds of file that are songs, best first.
                        formats.retain(|f| kind_of(f, &format!("x.{}", format_ext(f))).is_some());
                        formats.sort_by_key(|f| std::cmp::Reverse(kind_of(f, &format!("x.{}", format_ext(f))).map_or(0, |k| k.rank)));
                        formats.dedup();
                        Some(ArchiveHit {
                            title: Some(text(d, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| identifier.clone()),
                            creator: text(d, "creator"),
                            year: number(d, "year").and_then(|y| u32::try_from(y).ok()).filter(|y| *y > 0),
                            downloads: number(d, "downloads").unwrap_or(0),
                            lossless: formats_have_lossless(&formats),
                            formats,
                            identifier,
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();
        let result = ArchiveSearch { hits, total: number(response, "numFound").unwrap_or(0), page, page_size: PAGE_SIZE };
        self.archive_cache.lock().put_search(cache_key, result.clone());
        Ok(result)
    }

    /// An item and its songs.
    pub fn archive_item(&self, identifier: &str, lossless_only: bool) -> Result<ArchiveItem> {
        if !valid_identifier(identifier) {
            return Err(Error::Network("not an item on the Internet Archive".into()));
        }
        let cached = self.archive_cache.lock().item(identifier);
        let v = match cached {
            Some(v) => v,
            None => {
                let v = self.archive_json(&format!("{}/metadata/{identifier}", self.archive_base()), "Internet Archive: item")?;
                // Only a real item is worth remembering (not "not found").
                if v.get("metadata").is_some_and(Value::is_object) {
                    self.archive_cache.lock().put_item(identifier.to_string(), v.clone());
                }
                v
            }
        };
        let meta = v.get("metadata").filter(|m| m.is_object()).ok_or_else(|| Error::Network("the Internet Archive does not have that item any more".into()))?;
        if v.get("is_dark").and_then(Value::as_bool).unwrap_or(false) {
            return Err(Error::Network("that item has been taken down from the Internet Archive".into()));
        }
        if text(meta, "access-restricted-item").eq_ignore_ascii_case("true") {
            return Err(Error::Network("that item is restricted (it can only be borrowed), so it cannot be played here".into()));
        }
        let title = Some(text(meta, "title")).filter(|t| !t.is_empty()).unwrap_or_else(|| identifier.to_string());
        let creator = text(meta, "creator");
        let date = first_text(meta, &["date", "year"]);
        let files = v.get("files").and_then(Value::as_array).map_or(&[][..], Vec::as_slice);
        let facts = ItemFacts { identifier, title: &title, creator: &creator, year: year_of(&date) };
        let (tracks, hidden_lossy) = tracks_from(files, &facts, lossless_only);
        Ok(ArchiveItem {
            identifier: identifier.to_string(),
            description: plain_text(&text(meta, "description"), 700),
            license_url: Some(text(meta, "licenseurl")).filter(|u| u.starts_with("http")),
            details_url: format!("{OFFICIAL_BASE}/details/{identifier}"),
            title,
            creator,
            date,
            tracks,
            hidden_lossy,
        })
    }

    /// An item's picture (the Archive makes a small one for every item).
    pub fn archive_cover(&self, identifier: &str) -> Result<Vec<u8>> {
        if !valid_identifier(identifier) {
            return Err(Error::Network("not an item on the Internet Archive".into()));
        }
        let url = format!("{}/services/img/{identifier}", self.archive_base());
        let mut body = self.archive_open(&url, "image/*", None, "Internet Archive: cover", true)?;
        if body.status != 200 || !body.content_type.starts_with("image/") {
            return Err(Error::Network("the Internet Archive has no picture for that item".into()));
        }
        let mut out = Vec::new();
        body.reader.by_ref().take(4 * 1024 * 1024).read_to_end(&mut out).map_err(|e| Error::Network(e.to_string()))?;
        Ok(out)
    }
}

/// A file extension that goes with the Archive's name for a kind of file, so a bare format name can
/// be judged by [`kind_of`].
fn format_ext(format: &str) -> &'static str {
    let f = format.to_ascii_lowercase();
    if f.contains("flac") {
        "flac"
    } else if f.contains("apple lossless") || f.contains("advanced audio") {
        "m4a"
    } else if f == "wave" || f == "wav" {
        "wav"
    } else if f.contains("mp3") {
        "mp3"
    } else if f.contains("ogg") {
        "ogg"
    } else if f == "opus" {
        "opus"
    } else {
        "bin"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn facts<'a>() -> ItemFacts<'a> {
        ItemFacts { identifier: "demo", title: "Live at the Hall", creator: "The Band", year: Some(1977) }
    }

    fn file(name: &str, format: &str, extra: Value) -> Value {
        let mut v = json!({"name": name, "format": format, "source": "original", "size": "1000"});
        for (k, x) in extra.as_object().cloned().unwrap_or_default() {
            v[k] = x;
        }
        v
    }

    #[test]
    fn kinds_of_file() {
        assert_eq!(kind_of("Flac", "a.flac").map(|k| (k.lossless, k.depth)), Some((true, None)));
        assert_eq!(kind_of("24bit Flac", "a.flac").map(|k| (k.lossless, k.depth)), Some((true, Some(24))));
        assert_eq!(kind_of("WAVE", "a.wav").map(|k| k.lossless), Some(true));
        assert_eq!(kind_of("Apple Lossless Audio", "a.m4a").map(|k| k.lossless), Some(true));
        assert_eq!(kind_of("VBR MP3", "a.mp3").map(|k| k.lossless), Some(false));
        assert!(kind_of("MP3 Sample", "a_sample.mp3").is_none(), "a preview is not a song");
        assert!(kind_of("Shorten", "a.shn").is_none() && kind_of("AIFF", "a.aiff").is_none(), "formats the player cannot decode");
        assert!(kind_of("Flac", "a.txt").is_none(), "judged by the file too");
        assert!(kind_of("JPEG", "cover.jpg").is_none() && kind_of("Metadata", "x_meta.xml").is_none());
    }

    #[test]
    fn the_best_file_is_chosen_for_each_song() {
        let files = vec![
            file("t01.mp3", "VBR MP3", json!({"track": "1", "title": "Opening"})),
            file("t01.flac", "Flac", json!({"track": "1", "title": "Opening", "length": "1:05"})),
            file("t01.ogg", "Ogg Vorbis", json!({})),
            file("t02.mp3", "VBR MP3", json!({"track": "2", "bitrate": "256"})),
            file("t02_sample.mp3", "MP3 Sample", json!({})),
            file("cover.jpg", "JPEG", json!({})),
            file("notes.txt", "Text", json!({})),
        ];
        let (all, hidden) = tracks_from(&files, &facts(), false);
        assert_eq!(hidden, 0);
        assert_eq!(all.iter().map(|t| (t.file.as_str(), t.lossless)).collect::<Vec<_>>(), vec![("t01.flac", true), ("t02.mp3", false)]);
        assert_eq!(all[0].title, "Opening");
        assert_eq!(all[0].duration_secs, 65.0);
        assert_eq!((all[0].artist.as_str(), all[0].album.as_str(), all[0].year), ("The Band", "Live at the Hall", Some(1977)));
        assert_eq!(all[0].path, "archive://demo/t01.flac");
        assert_eq!(all[1].title, "t02", "no title: the file name");
        assert_eq!(all[1].bitrate_kbps, Some(256));
        let (only, hidden) = tracks_from(&files, &facts(), true);
        assert_eq!((only.len(), hidden), (1, 1), "the song that only exists as MP3 is left out and counted");
    }

    #[test]
    fn songs_come_in_the_right_order() {
        // Track numbers that restart on each disc must not interleave the discs.
        let files = vec![
            file("d2t01.flac", "Flac", json!({"track": "1"})),
            file("d1t10.flac", "Flac", json!({"track": "10"})),
            file("d1t02.flac", "Flac", json!({"track": "2"})),
            file("d1t01.flac", "Flac", json!({"track": "1"})),
        ];
        let (t, _) = tracks_from(&files, &facts(), true);
        assert_eq!(t.iter().map(|t| t.file.as_str()).collect::<Vec<_>>(), vec!["d1t01.flac", "d1t02.flac", "d1t10.flac", "d2t01.flac"]);
        // Unique track numbers decide, even when the names say otherwise.
        let files = vec![file("b.flac", "Flac", json!({"track": "1"})), file("a.flac", "Flac", json!({"track": "2"}))];
        let (t, _) = tracks_from(&files, &facts(), true);
        assert_eq!(t[0].file, "b.flac");
        assert!(natural_cmp("t2", "t10").is_lt() && natural_cmp("t02", "t2").is_eq() && natural_cmp("a", "b").is_lt());
    }

    #[test]
    fn lengths_and_descriptions() {
        assert_eq!(parse_length("245.31"), 245.31);
        assert_eq!(parse_length("4:05"), 245.0);
        assert_eq!(parse_length("1:02:03"), 3723.0);
        assert_eq!(parse_length("soon"), 0.0);
        assert_eq!(plain_text("<p>Recorded <b>live</b> &amp; mixed<br/>by&nbsp;us</p>", 100), "Recorded live & mixed by us");
        let long = plain_text(&"word ".repeat(400), 50);
        assert!(long.ends_with('…') && long.chars().count() <= 51, "{long}");
        assert_eq!(year_of("1977-05-08"), Some(1977));
        assert_eq!(year_of("n.d."), None);
    }

    #[test]
    fn what_a_person_types_cannot_become_search_syntax() {
        assert_eq!(search_words("Miles Davis: \"Kind of Blue\" OR (x) -y *"), vec!["miles", "davis", "kind", "of", "blue", "or", "x", "y"]);
        let q = ArchiveQuery { text: "grateful dead".into(), lossless_only: true, collection: "etree".into(), ..Default::default() };
        assert_eq!(build_query(&q), r#"mediatype:audio AND (grateful AND dead) AND collection:etree AND format:("Flac" OR "24bit Flac" OR "WAVE" OR "Apple Lossless Audio")"#);
        let q = ArchiveQuery { text: "x".into(), collection: "something-else".into(), ..Default::default() };
        assert_eq!(build_query(&q), "mediatype:audio AND (x)", "an unknown collection is ignored");
        assert_eq!(build_query(&ArchiveQuery::default()), "mediatype:audio");
        assert!(search_words(&"word ".repeat(40)).len() <= 12);
    }

    #[test]
    fn addresses_and_names() {
        let n = Net::new(std::env::temp_dir().join("redshift-archive-test"), "t", &[]);
        assert_eq!(n.archive_stream_url("demo", "Disc 1/01 Café.flac").unwrap(), "https://archive.org/download/demo/Disc%201/01%20Caf%C3%A9.flac");
        assert!(n.archive_stream_url("de mo", "a.flac").is_err() && n.archive_stream_url("demo", "../x").is_err() && n.archive_stream_url("", "a").is_err());
        assert!(n.check_archive_url("https://archive.org/download/x/y.flac").is_ok());
        assert!(n.check_archive_url("https://ia800000.us.archive.org/12/items/x/y.flac").is_ok());
        for bad in ["http://archive.org/x", "https://archive.org.evil.com/x", "https://evilarchive.org/x", "https://archive.org@evil.com/x", "https://example.com/archive.org", "ftp://archive.org/x"] {
            assert!(n.check_archive_url(bad).is_err(), "{bad} must not be allowed");
        }
        assert!(matches!(n.archive_open("https://archive.org/x", "*/*", None, "t", false), Err(Error::Offline)), "off until the online switch is on");
    }

    // ---- against the fake Archive ----

    use crate::net::fakearchive;
    use std::sync::Arc;

    fn live(tag: &str) -> (fakearchive::Fake, Arc<Net>) {
        let f = fakearchive::start(vec![0u8; 10], true);
        let n = Arc::new(Net::new(std::env::temp_dir().join(format!("redshift-archive-{tag}")), "t", &[]));
        n.set_enabled(true);
        n.set_archive_base_for_tests(&f.base);
        (f, n)
    }

    #[test]
    fn the_same_search_or_item_is_not_asked_for_twice() {
        let (f, n) = live("recent");
        let q = ArchiveQuery { text: "live hall".into(), page: 1, ..Default::default() };
        let a = n.archive_search(&q).unwrap();
        let b = n.archive_search(&q).unwrap();
        assert_eq!(a, b);
        let first = n.archive_item("demo-live", false).unwrap();
        let again = n.archive_item("demo-live", true).unwrap();
        assert_eq!(first.tracks.len(), again.tracks.len() + again.hidden_lossy, "the remembered answer still honours Lossless only");
        let log = f.requests.lock().clone();
        assert_eq!(log.iter().filter(|l| l.contains("advancedsearch")).count(), 1, "{log:?}");
        assert_eq!(log.iter().filter(|l| l.contains("/metadata/demo-live")).count(), 1, "{log:?}");
        // A different search is a different question.
        n.archive_search(&ArchiveQuery { text: "old radio".into(), page: 1, ..Default::default() }).unwrap();
        assert_eq!(f.requests.lock().iter().filter(|l| l.contains("advancedsearch")).count(), 2);
    }

    #[test]
    fn search_reads_the_results() {
        let (f, n) = live("search");
        let r = n.archive_search(&ArchiveQuery { text: "live hall".into(), lossless_only: true, page: 1, ..Default::default() }).unwrap();
        assert_eq!((r.total, r.page, r.hits.len()), (2, 1, 2));
        let a = &r.hits[0];
        assert_eq!((a.identifier.as_str(), a.title.as_str(), a.creator.as_str(), a.year, a.downloads), ("demo-live", "Live at the Hall", "The Band", Some(1977), 1234));
        assert_eq!(a.formats, vec!["Flac", "VBR MP3"], "only kinds of song, best first");
        assert!(a.lossless && !r.hits[1].lossless);
        assert_eq!(r.hits[1].title, "Old Radio Show");
        let line = f.requests.lock().iter().find(|l| l.contains("advancedsearch")).cloned().unwrap();
        assert!(line.contains("q=mediatype%3Aaudio%20AND%20%28live%20AND%20hall%29"), "{line}");
        assert!(line.contains("sort%5B%5D=downloads%20desc") && line.contains("rows=40") && line.contains("output=json"), "{line}");
        // What was sent is in the request log, in full.
        let log = n.log();
        assert!(log.iter().any(|r| r.purpose == "Internet Archive: search" && r.url.contains("q=mediatype") && r.status == Some(200)), "{log:?}");
    }

    #[test]
    fn an_item_lists_its_best_songs() {
        let (_f, n) = live("item");
        let item = n.archive_item("demo-live", true).unwrap();
        assert_eq!((item.title.as_str(), item.creator.as_str(), item.date.as_str()), ("Live at the Hall", "The Band", "1977-05-08"));
        assert_eq!(item.description, "Recorded & mixed.");
        assert_eq!(item.license_url.as_deref(), Some("http://creativecommons.org/licenses/by-nc-sa/3.0/"));
        assert_eq!(item.details_url, "https://archive.org/details/demo-live");
        assert_eq!(item.tracks.iter().map(|t| t.file.as_str()).collect::<Vec<_>>(), vec!["d1t01.flac", "Disc 2/d2 t01 Café.flac"]);
        assert_eq!(item.hidden_lossy, 1);
        assert!(item.tracks.iter().all(|t| t.lossless && t.year == Some(1977)));
        assert_eq!(item.tracks[0].title, "Opening");
        assert_eq!(item.tracks[1].duration_secs, 60.0);
        let all = n.archive_item("demo-live", false).unwrap();
        assert_eq!((all.tracks.len(), all.hidden_lossy), (3, 0));
        let mp3 = all.tracks.iter().find(|t| t.file == "d1t03.mp3").expect("the song that only has an MP3 is listed");
        assert!(!mp3.lossless && mp3.format == "VBR MP3");
        assert_eq!(n.archive_item("lossy-only", true).unwrap().tracks.len(), 0);
    }

    #[test]
    fn items_that_cannot_be_played_say_why() {
        let (_f, n) = live("gone");
        let say = |id: &str| n.archive_item(id, true).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(say("nothing-here").contains("any more"), "{}", say("nothing-here"));
        assert!(say("taken-down").contains("taken down"));
        assert!(say("borrow-only").contains("restricted"));
        assert!(say("not an id").contains("not an item"));
    }

    #[test]
    fn covers_and_the_rules_about_where_files_come_from() {
        let (f, n) = live("cover");
        assert!(n.archive_cover("demo-live").unwrap().starts_with(b"\x89PNG"));
        assert!(n.archive_cover("no-picture").is_err());
        // A redirect away from the Archive is not followed.
        let err = n.archive_open(&format!("{}/download/evil/x.flac", f.base), "*/*", None, "t", false).err().expect("must refuse").to_string();
        assert!(err.contains("not part of the Internet Archive"), "{err}");
        n.set_enabled(false);
        assert!(matches!(n.archive_search(&ArchiveQuery::default()), Err(Error::Offline)));
        assert!(!f.requests.lock().iter().any(|l| l.contains("example.com")));
    }
}
