//! A small fake Internet Archive on the loopback address, for tests: search and item answers in the
//! shape the real ones have, `/download/...` that redirects the way the real one hands files to
//! another server, and the audio with or without Range support.

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

pub struct Fake {
    /// `http://127.0.0.1:port`
    pub base: String,
    /// Every request line (`GET /metadata/demo-live HTTP/1.1`).
    pub requests: Arc<Mutex<Vec<String>>>,
    /// The `Range` header of each audio request.
    pub ranges: Arc<Mutex<Vec<String>>>,
    pub hits: Arc<AtomicUsize>,
}

fn unescape(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                let hex = std::str::from_utf8(&b[i + 1..i + 3]).unwrap_or("00");
                out.push(u8::from_str_radix(hex, 16).unwrap_or(b'?'));
                i += 3;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

const SEARCH: &str = r#"{"responseHeader":{"status":0},"response":{"numFound":2,"start":0,"docs":[
{"identifier":"demo-live","title":"Live at the Hall","creator":["The Band"],"year":"1977","downloads":1234,"format":["Flac","VBR MP3","JPEG","Metadata","Item Tile"]},
{"identifier":"lossy-only","title":"Old Radio Show","downloads":7,"format":["VBR MP3","Ogg Vorbis"]}]}}"#;

const DEMO_LIVE: &str = r#"{"files":[
{"name":"d1t01.flac","source":"original","format":"Flac","size":"2000000","length":"12.5","track":"1","title":"Opening"},
{"name":"d1t01.mp3","source":"derivative","format":"VBR MP3","size":"300000","length":"12.5","track":"1","bitrate":"190"},
{"name":"d1t01.ogg","source":"derivative","format":"Ogg Vorbis","size":"200000"},
{"name":"Disc 2/d2 t01 Café.flac","source":"original","format":"Flac","size":"2000000","length":"1:00","track":"1"},
{"name":"d1t03.mp3","source":"original","format":"VBR MP3","size":"900000","length":"200","track":"3"},
{"name":"d1t03_sample.mp3","source":"derivative","format":"MP3 Sample","size":"10000"},
{"name":"cover.jpg","source":"original","format":"JPEG","size":"50000"},
{"name":"demo-live_meta.xml","source":"original","format":"Metadata"}],
"metadata":{"identifier":"demo-live","title":"Live at the Hall","creator":"The Band","date":"1977-05-08","description":"<p>Recorded &amp; mixed.</p>","licenseurl":"http://creativecommons.org/licenses/by-nc-sa/3.0/"}}"#;

fn item(id: &str) -> String {
    match id {
        "demo-live" => DEMO_LIVE.to_string(),
        "lossy-only" => r#"{"files":[{"name":"show.mp3","source":"original","format":"VBR MP3","size":"900000","length":"3:00"}],"metadata":{"identifier":"lossy-only","title":"Old Radio Show"}}"#.to_string(),
        "taken-down" => r#"{"files":[],"metadata":{"identifier":"taken-down","title":"x"},"is_dark":true}"#.to_string(),
        "borrow-only" => r#"{"files":[],"metadata":{"identifier":"borrow-only","title":"x","access-restricted-item":"true"}}"#.to_string(),
        _ => "{}".to_string(),
    }
}

type Log = (Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>);

fn handle(mut stream: TcpStream, audio: &[u8], honor_range: bool, fake: &Log) {
    let mut data: Vec<u8> = Vec::new();
    let mut buf = [0u8; 4096];
    while !data.windows(4).any(|w| w == b"\r\n\r\n") {
        match stream.read(&mut buf) {
            Ok(0) | Err(_) => return,
            Ok(n) => data.extend_from_slice(&buf[..n]),
        }
    }
    let text = String::from_utf8_lossy(&data).into_owned();
    let mut lines = text.lines();
    let first = lines.next().unwrap_or("").to_string();
    let range = lines.find_map(|l| l.strip_prefix("Range: ").or_else(|| l.strip_prefix("range: "))).map(str::to_string);
    fake.2.fetch_add(1, Ordering::Relaxed);
    fake.0.lock().push(first.clone());
    let target = first.split(' ').nth(1).unwrap_or("/").to_string();
    let (path, _query) = target.split_once('?').unwrap_or((target.as_str(), ""));
    let path = unescape(path);

    let respond = |stream: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8]| {
        let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    };

    if path == "/advancedsearch.php" {
        respond(&mut stream, "200 OK", "application/json", "", SEARCH.as_bytes());
    } else if let Some(id) = path.strip_prefix("/metadata/") {
        respond(&mut stream, "200 OK", "application/json", "", item(id).as_bytes());
    } else if let Some(id) = path.strip_prefix("/services/img/") {
        if id == "no-picture" {
            respond(&mut stream, "404 Not Found", "text/plain", "", b"");
        } else {
            respond(&mut stream, "200 OK", "image/png", "", b"\x89PNG\r\n\x1a\nfake");
        }
    } else if let Some(rest) = path.strip_prefix("/download/") {
        // The real one sends you to the server that holds the item.
        let location = if rest.starts_with("evil/") { "http://example.com/stolen.flac".to_string() } else { format!("/dl/{rest}") };
        let encoded: String = location.bytes().map(|b| if b == b' ' { "%20".to_string() } else if b.is_ascii() { (b as char).to_string() } else { format!("%{b:02X}") }).collect();
        respond(&mut stream, "302 Found", "text/plain", &format!("Location: {encoded}\r\n"), b"");
    } else if let Some(rest) = path.strip_prefix("/dl/") {
        if rest.ends_with("missing.flac") {
            respond(&mut stream, "404 Not Found", "text/plain", "", b"");
            return;
        }
        if rest.ends_with("forbidden.flac") {
            respond(&mut stream, "403 Forbidden", "text/plain", "", b"");
            return;
        }
        fake.1.lock().push(range.clone().unwrap_or_default());
        let total = audio.len();
        match range.as_deref().and_then(|r| r.strip_prefix("bytes=")).and_then(|r| r.split_once('-')) {
            Some((a, b)) if honor_range => {
                let a: usize = a.parse().unwrap_or(0);
                let b: usize = b.parse().unwrap_or(total - 1).min(total - 1);
                if a >= total {
                    respond(&mut stream, "416 Range Not Satisfiable", "text/plain", &format!("Content-Range: bytes */{total}\r\n"), b"");
                    return;
                }
                respond(&mut stream, "206 Partial Content", "audio/flac", &format!("Content-Range: bytes {a}-{b}/{total}\r\nAccept-Ranges: bytes\r\n"), &audio[a..=b]);
            }
            _ => respond(&mut stream, "200 OK", "audio/flac", "", audio),
        }
    } else {
        respond(&mut stream, "404 Not Found", "text/plain", "", b"");
    }
}

/// Starts the fake. `audio` is what every file under `/download/` turns out to be (except names
/// ending `missing.flac` and `forbidden.flac`).
pub fn start(audio: Vec<u8>, honor_range: bool) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let state: Log = (requests.clone(), ranges.clone(), hits.clone());
    let audio = Arc::new(audio);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(stream) = stream else { break };
            let (audio, state) = (audio.clone(), state.clone());
            std::thread::spawn(move || handle(stream, &audio, honor_range, &state));
        }
    });
    Fake { base: format!("http://127.0.0.1:{port}"), requests, ranges, hits }
}
