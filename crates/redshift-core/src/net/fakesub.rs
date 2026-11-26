//! A small fake Subsonic server on the loopback address, for tests: it checks the login token like a
//! real one, answers the list endpoints with a tiny fixed library and serves one audio file with or
//! without Range support.

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use md5::{Digest, Md5};
use parking_lot::Mutex;

pub const USER: &str = "me";
pub const PASSWORD: &str = "secret";

pub struct Fake {
    /// `http://127.0.0.1:port`
    pub base: String,
    /// Every request line (`GET /rest/ping?...`).
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
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            c => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn ok(body: &str) -> String {
    format!(r#"{{"subsonic-response":{{"status":"ok","version":"1.16.1","type":"fakesonic","serverVersion":"0.1",{body}}}}}"#)
}

fn failed(code: i32, msg: &str) -> String {
    format!(r#"{{"subsonic-response":{{"status":"failed","version":"1.16.1","error":{{"code":{code},"message":"{msg}"}}}}}}"#)
}

const SONG1: &str = r#"{"id":"s1","title":"One","artist":"Ada","album":"First Light","albumId":"al1","track":1,"year":2020,"duration":12,"bitRate":900,"samplingRate":44100,"bitDepth":16,"channelCount":2,"suffix":"flac","size":2000000,"coverArt":"al-al1","genre":"Ambient"}"#;
const SONG2: &str = r#"{"id":"s2","title":"Two","artist":"Ada","album":"First Light","albumId":"al1","track":2,"year":2020,"duration":200,"bitRate":320,"suffix":"mp3","size":8000000,"coverArt":"al-al1"}"#;
const ALBUM: &str = r#"{"id":"al1","name":"First Light","artist":"Ada","year":2020,"songCount":2,"duration":212,"coverArt":"al-al1"}"#;

fn route(endpoint: &str, params: &HashMap<String, String>) -> Option<String> {
    Some(match endpoint {
        "ping" => ok(r#""x":0"#),
        "getAlbumList2" => ok(&format!(r#""albumList2":{{"album":[{ALBUM}]}}"#)),
        "getArtists" => ok(r#""artists":{"index":[{"name":"A","artist":[{"id":"ar1","name":"Ada","albumCount":1}]}]}"#),
        "getArtist" => ok(&format!(r#""artist":{{"id":"ar1","name":"Ada","albumCount":1,"album":[{ALBUM}]}}"#)),
        "getAlbum" => {
            if params.get("id").map(String::as_str) != Some("al1") {
                return Some(failed(70, "Album not found"));
            }
            ok(&format!(r#""album":{{"id":"al1","name":"First Light","artist":"Ada","year":2020,"songCount":2,"duration":212,"coverArt":"al-al1","song":[{SONG1},{SONG2}]}}"#))
        }
        "getSong" => match params.get("id").map(String::as_str) {
            Some("s1") => ok(&format!(r#""song":{SONG1}"#)),
            Some("s2") => ok(&format!(r#""song":{SONG2}"#)),
            _ => failed(70, "Song not found"),
        },
        "getPlaylists" => ok(r#""playlists":{"playlist":[{"id":"p1","name":"Evening","songCount":2,"duration":212}]}"#),
        "getPlaylist" => ok(&format!(r#""playlist":{{"id":"p1","name":"Evening","songCount":2,"duration":212,"entry":[{SONG1},{SONG2}]}}"#)),
        "search3" => ok(&format!(r#""searchResult3":{{"artist":[{{"id":"ar1","name":"Ada","albumCount":1}}],"album":[{ALBUM}],"song":[{SONG1}]}}"#)),
        _ => return None,
    })
}

fn handle(mut stream: TcpStream, audio: &[u8], honor_range: bool, fake: &(Arc<Mutex<Vec<String>>>, Arc<Mutex<Vec<String>>>, Arc<AtomicUsize>)) {
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
    let target = first.split(' ').nth(1).unwrap_or("/");
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    let params: HashMap<String, String> = query.split('&').filter(|p| !p.is_empty()).map(|p| {
        let (k, v) = p.split_once('=').unwrap_or((p, ""));
        (unescape(k), unescape(v))
    }).collect();
    let endpoint = path.rsplit('/').next().unwrap_or("");

    let respond = |stream: &mut TcpStream, status: &str, ctype: &str, extra: &str, body: &[u8]| {
        let head = format!("HTTP/1.1 {status}\r\nContent-Type: {ctype}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(body);
        let _ = stream.flush();
    };

    let good_login = params.get("u").map(String::as_str) == Some(USER)
        && matches!((params.get("t"), params.get("s")), (Some(t), Some(s)) if *t == md5_hex(&format!("{PASSWORD}{s}")));
    if !good_login {
        respond(&mut stream, "200 OK", "application/json", "", failed(40, "Wrong username or password").as_bytes());
        return;
    }
    match endpoint {
        "stream" => {
            if params.get("id").map(String::as_str) == Some("missing") {
                respond(&mut stream, "200 OK", "application/json", "", failed(70, "Song not found").as_bytes());
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
        }
        "getCoverArt" => respond(&mut stream, "200 OK", "image/png", "", b"\x89PNG\r\n\x1a\nfake"),
        other => match route(other, &params) {
            Some(json) => respond(&mut stream, "200 OK", "application/json", "", json.as_bytes()),
            None => respond(&mut stream, "200 OK", "application/json", "", failed(0, "Unknown endpoint").as_bytes()),
        },
    }
}

/// Starts the fake. `audio` is what `stream` serves for any id except `missing`.
pub fn start(audio: Vec<u8>, honor_range: bool) -> Fake {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let state = (requests.clone(), ranges.clone(), hits.clone());
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

/// A 16-bit stereo WAV of a quiet tone, `seconds` long: real audio the decoder can play and seek in.
pub fn wav(seconds: u32, rate: u32) -> Vec<u8> {
    let frames = seconds * rate;
    let data_len = frames * 4;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend_from_slice(b"RIFF");
    v.extend_from_slice(&(36 + data_len).to_le_bytes());
    v.extend_from_slice(b"WAVEfmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&2u16.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * 4).to_le_bytes());
    v.extend_from_slice(&4u16.to_le_bytes());
    v.extend_from_slice(&16u16.to_le_bytes());
    v.extend_from_slice(b"data");
    v.extend_from_slice(&data_len.to_le_bytes());
    for i in 0..frames {
        let s = ((i as f32 * 440.0 * std::f32::consts::TAU / rate as f32).sin() * 3000.0) as i16;
        v.extend_from_slice(&s.to_le_bytes());
        v.extend_from_slice(&s.to_le_bytes());
    }
    v
}
