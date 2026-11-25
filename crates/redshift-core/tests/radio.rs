//! Streams from the internet: a fake radio station on the loopback address feeds the real player.
//! Each connection sends the fixture once and closes, so keeping the music going also exercises the
//! reconnect path.

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use redshift_core::engine::{OfflineConfig, Player, PlayerConfig, PlayerEvent, Status};
use redshift_core::net::Net;

struct Station {
    url: String,
    hits: Arc<AtomicUsize>,
    /// What the client sent in its request headers, one entry per connection.
    requests: Arc<Mutex<Vec<String>>>,
}

/// A station that plays `body` from the start to every listener, then hangs up. With `metaint`, a
/// title is sent inline after the first block of audio, the way Icecast does when asked.
fn station(body: Vec<u8>, content_type: &'static str, metaint: Option<usize>, title: &'static str, status: u16) -> Station {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://127.0.0.1:{}/live", listener.local_addr().unwrap().port());
    let hits = Arc::new(AtomicUsize::new(0));
    let requests = Arc::new(Mutex::new(Vec::new()));
    let (h, r) = (hits.clone(), requests.clone());
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            h.fetch_add(1, Ordering::SeqCst);
            let body = body.clone();
            let r = r.clone();
            std::thread::spawn(move || {
                let mut data = Vec::new();
                let mut buf = [0u8; 1024];
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                }
                let request = String::from_utf8_lossy(&data).to_string();
                let wants_meta = request.to_ascii_lowercase().contains("icy-metadata: 1");
                r.lock().push(request);
                if status != 200 {
                    let _ = write!(stream, "HTTP/1.1 {status} No\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                    return;
                }
                let metaint = metaint.filter(|_| wants_meta);
                let mut head = format!("HTTP/1.0 200 OK\r\nContent-Type: {content_type}\r\nicy-name: Test FM\r\nicy-genre: Test\r\nicy-br: 128\r\n");
                if let Some(m) = metaint {
                    head.push_str(&format!("icy-metaint: {m}\r\n"));
                }
                head.push_str("Connection: close\r\n\r\n");
                let mut out = head.into_bytes();
                match metaint {
                    Some(m) => {
                        for (i, chunk) in body.chunks(m).enumerate() {
                            out.extend_from_slice(chunk);
                            if chunk.len() < m {
                                break;
                            }
                            if i == 0 {
                                let mut block = format!("StreamTitle='{title}';").into_bytes();
                                block.resize(block.len().div_ceil(16) * 16, 0);
                                out.push((block.len() / 16) as u8);
                                out.extend(block);
                            } else {
                                out.push(0);
                            }
                        }
                    }
                    None => out.extend_from_slice(&body),
                }
                let _ = stream.write_all(&out);
            });
        }
    });
    Station { url, hits, requests }
}

struct Rig {
    player: Player,
    events: Receiver<PlayerEvent>,
    seen: Vec<PlayerEvent>,
    _net: Arc<Net>,
}

fn rig(online: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let net = Arc::new(Net::new(dir.path().join("cache"), "Redshift-test/0.0", &[]));
    net.set_enabled(online);
    net.allow_local_streams(true);
    let sink = Arc::new(Mutex::new(Vec::new()));
    let (player, events) = Player::new(PlayerConfig {
        net: Some(net.clone()),
        device: None,
        exclusive: false,
        offline: Some(OfflineConfig { sample_rate: 48_000, sink, speed: 4.0 }),
    })
    .unwrap();
    std::mem::forget(dir);
    Rig { player, events, seen: Vec::new(), _net: net }
}

impl Rig {
    fn wait(&mut self, what: &str, mut ok: impl FnMut(&Player) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            while let Ok(e) = self.events.try_recv() {
                self.seen.push(e);
            }
            if ok(&self.player) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; events {:#?}; state {:#?}", self.seen, self.player.state().live);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

impl Rig {
    /// Waits until an event matching `pred` has been seen.
    fn wait_event(&mut self, what: &str, pred: impl Fn(&PlayerEvent) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            while let Ok(e) = self.events.try_recv() {
                self.seen.push(e);
            }
            if self.seen.iter().any(&pred) {
                return;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what}; events {:#?}", self.seen);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn fixture_bytes(name: &str) -> Vec<u8> {
    std::fs::read(fixture(name)).unwrap()
}

fn queue_url(r: &Rig, url: &str) {
    r.player.set_queue(vec![PathBuf::from(url)], 0, true);
}

#[test]
fn an_mp3_station_plays_and_its_song_title_is_shown() {
    let s = station(fixture_bytes("chirp_44k_stereo.mp3"), "audio/mpeg", Some(8192), "Paper Satellites - Carrier Wave", 200);
    let mut r = rig(true);
    queue_url(&r, &s.url);
    r.wait("the station to play", |p| p.status() == Status::Playing && p.state().live.is_some_and(|l| l.title == "Paper Satellites - Carrier Wave"));
    let live = r.player.state().live.unwrap();
    assert_eq!((live.codec.as_str(), live.name.as_str(), live.bitrate_kbps, live.lossless, live.secure), ("mp3", "Test FM", Some(128), false, false));
    assert!(s.requests.lock()[0].to_ascii_lowercase().contains("icy-metadata: 1"), "asks for song titles");
    // The song is over in a second; the stream carries on by reconnecting.
    r.wait("a second connection", |p| s.hits.load(Ordering::SeqCst) >= 2 && p.status() == Status::Playing);
    assert!(r.player.current_track().unwrap().duration_secs.is_none(), "a stream has no length");
}

#[test]
fn a_flac_station_is_marked_lossless() {
    let s = station(fixture_bytes("chirp_44k_stereo.flac"), "audio/flac", None, "", 200);
    let mut r = rig(true);
    queue_url(&r, &s.url);
    r.wait("the station to play", |p| p.status() == Status::Playing && p.state().live.is_some_and(|l| l.sample_rate > 0));
    let live = r.player.state().live.unwrap();
    assert!(live.lossless, "{live:?}");
    assert_eq!((live.codec.as_str(), live.sample_rate, live.bits_per_sample.is_some()), ("flac", 44_100, true));
}

#[test]
fn an_opus_station_plays() {
    let s = station(fixture_bytes("chirp_48k_stereo_opus.ogg"), "application/ogg", None, "", 200);
    let mut r = rig(true);
    queue_url(&r, &s.url);
    r.wait("the station to play", |p| p.status() == Status::Playing && p.state().live.is_some_and(|l| l.codec == "opus"));
    assert!(!r.player.state().live.unwrap().lossless);
}

#[test]
fn nothing_is_fetched_while_online_features_are_off() {
    let s = station(fixture_bytes("chirp_44k_stereo.mp3"), "audio/mpeg", None, "", 200);
    let mut r = rig(false);
    queue_url(&r, &s.url);
    r.wait_event("an error", |e| matches!(e, PlayerEvent::Error { .. }));
    assert_eq!(s.hits.load(Ordering::SeqCst), 0, "no connection was made");
}

#[test]
fn a_station_that_answers_with_an_error_is_reported() {
    let s = station(Vec::new(), "audio/mpeg", None, "", 404);
    let mut r = rig(true);
    queue_url(&r, &s.url);
    r.wait_event("an error", |e| matches!(e, PlayerEvent::Error { message } if message.contains("404")));
}

#[test]
fn seeking_a_live_stream_does_nothing() {
    let s = station(fixture_bytes("chirp_44k_stereo.flac"), "audio/flac", None, "", 200);
    let mut r = rig(true);
    queue_url(&r, &s.url);
    r.wait("the station to play", |p| p.status() == Status::Playing && p.state().live.is_some());
    r.player.seek(30.0);
    std::thread::sleep(Duration::from_millis(300));
    r.wait("still playing", |p| p.status() == Status::Playing);
    assert!(!r.seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })), "{:#?}", r.seen);
}
