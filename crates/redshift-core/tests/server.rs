//! Songs from the person's own server: a fake Subsonic server on the loopback address (with Range
//! support) feeds the real player. The library fixtures are served as the "original files".

mod common;

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use redshift_core::engine::{OfflineConfig, Player, PlayerConfig, PlayerEvent, Status};
use redshift_core::net::{Net, ServerConfig};
use redshift_core::util::{archive_song_path, server_song_path};

struct Server {
    base: String,
    ranges: Arc<Mutex<Vec<String>>>,
    hits: Arc<AtomicUsize>,
}

/// Serves `audio` at `/rest/stream` with Range support. (The login is not checked here; the
/// library's own tests cover that.)
fn server(audio: Vec<u8>, content_type: &'static str) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let ranges = Arc::new(Mutex::new(Vec::new()));
    let hits = Arc::new(AtomicUsize::new(0));
    let (r, h) = (ranges.clone(), hits.clone());
    let audio = Arc::new(audio);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let (audio, r, h) = (audio.clone(), r.clone(), h.clone());
            std::thread::spawn(move || {
                let mut data = Vec::new();
                let mut buf = [0u8; 2048];
                while !data.windows(4).any(|w| w == b"\r\n\r\n") {
                    match stream.read(&mut buf) {
                        Ok(0) | Err(_) => return,
                        Ok(n) => data.extend_from_slice(&buf[..n]),
                    }
                }
                h.fetch_add(1, Ordering::SeqCst);
                let text = String::from_utf8_lossy(&data).to_string();
                let range = text.lines().find_map(|l| l.to_ascii_lowercase().strip_prefix("range: bytes=").map(str::to_string));
                let total = audio.len();
                let (status, extra, body): (&str, String, &[u8]) = match range.as_deref().and_then(|r| r.split_once('-')) {
                    Some((a, b)) => {
                        r.lock().push(format!("{a}-{b}"));
                        let a: usize = a.parse().unwrap_or(0);
                        let b: usize = b.parse().unwrap_or(total - 1).min(total - 1);
                        ("206 Partial Content", format!("Content-Range: bytes {a}-{b}/{total}\r\n"), &audio[a..=b])
                    }
                    None => ("200 OK", String::new(), &audio[..]),
                };
                let head = format!("HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                let _ = stream.write_all(head.as_bytes());
                let _ = stream.write_all(body);
            });
        }
    });
    Server { base, ranges, hits }
}

struct Rig {
    player: Player,
    events: Receiver<PlayerEvent>,
    seen: Vec<PlayerEvent>,
}

fn rig(base: &str, online: bool) -> Rig {
    rig_for(base, online, false)
}

/// `archive`: the loopback server stands in for archive.org instead of for a Subsonic server.
fn rig_for(base: &str, online: bool, archive: bool) -> Rig {
    let dir = tempfile::tempdir().unwrap();
    let net = Arc::new(Net::new(dir.path().join("cache"), "Redshift-test/0.0", &[]));
    net.set_enabled(online);
    if archive {
        net.set_archive_base_for_tests(base);
    } else {
        net.set_server(Some(&ServerConfig { url: base.to_string(), user: "me".into(), password: "secret".into() }));
    }
    let sink = Arc::new(Mutex::new(Vec::new()));
    let (player, events) = Player::new(PlayerConfig {
        net: Some(net),
        device: None,
        exclusive: false,
        offline: Some(OfflineConfig { sample_rate: 48_000, sink, speed: 4.0 }),
    })
    .unwrap();
    std::mem::forget(dir);
    Rig { player, events, seen: Vec::new() }
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
            assert!(Instant::now() < deadline, "timed out waiting for {what}; events {:#?}", self.seen);
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

#[test]
fn a_flac_on_the_server_plays_as_a_normal_song_with_a_length() {
    let s = server(std::fs::read(fixture("chirp_44k_stereo.flac")).unwrap(), "audio/flac");
    let mut r = rig(&s.base, true);
    r.player.set_queue(vec![server_song_path("s1")], 0, true);
    r.wait("the song to play", |p| p.status() == Status::Playing && p.current_track().is_some());
    let cur = r.player.current_track().unwrap();
    assert_eq!((cur.codec, cur.source_rate, cur.bits_per_sample.is_some()), ("flac", 44_100, true));
    assert!(cur.duration_secs.is_some_and(|d| d > 0.5), "a song from the server has a length, unlike a stream: {cur:?}");
    assert!(r.player.state().live.is_none(), "it is not a live stream");
    r.wait("the song to end", |p| p.status() != Status::Playing);
    assert!(!s.ranges.lock().is_empty(), "it was fetched in ranges");
}

#[test]
fn seeking_in_a_song_from_the_server_works() {
    let s = server(std::fs::read(fixture("chirp_44k_stereo.flac")).unwrap(), "audio/flac");
    let mut r = rig(&s.base, true);
    r.player.set_queue(vec![server_song_path("s1")], 0, true);
    r.wait("the song to play", |p| p.status() == Status::Playing);
    r.player.pause();
    r.wait("pause", |p| p.status() == Status::Paused);
    r.player.seek(0.5);
    r.wait("the seek", |p| (p.position_secs() - 0.5).abs() < 0.1);
}

#[test]
fn nothing_is_fetched_while_online_features_are_off() {
    let s = server(std::fs::read(fixture("chirp_44k_stereo.flac")).unwrap(), "audio/flac");
    let mut r = rig(&s.base, false);
    r.player.set_queue(vec![server_song_path("s1")], 0, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !r.seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })) {
        while let Ok(e) = r.events.try_recv() {
            r.seen.push(e);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(r.seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })), "an error is reported");
    assert_eq!(s.hits.load(Ordering::SeqCst), 0, "no connection was made");
}

#[test]
fn a_flac_from_the_archive_plays_with_a_length_and_seeks() {
    let s = server(std::fs::read(fixture("chirp_44k_stereo.flac")).unwrap(), "audio/flac");
    let mut r = rig_for(&s.base, true, true);
    r.player.set_queue(vec![archive_song_path("demo-live", "disc 1/01 Opening.flac")], 0, true);
    r.wait("the song to play", |p| p.status() == Status::Playing && p.current_track().is_some());
    let cur = r.player.current_track().unwrap();
    assert_eq!((cur.codec, cur.source_rate), ("flac", 44_100));
    assert!(cur.duration_secs.is_some_and(|d| d > 0.5), "an Archive song has a length: {cur:?}");
    assert!(r.player.state().live.is_none(), "it is not a live stream");
    r.player.pause();
    r.wait("pause", |p| p.status() == Status::Paused);
    r.player.seek(0.5);
    r.wait("the seek", |p| (p.position_secs() - 0.5).abs() < 0.1);
    assert!(!s.ranges.lock().is_empty(), "it was fetched in ranges");
}

#[test]
fn nothing_is_fetched_from_the_archive_while_online_features_are_off() {
    let s = server(std::fs::read(fixture("chirp_44k_stereo.flac")).unwrap(), "audio/flac");
    let mut r = rig_for(&s.base, false, true);
    r.player.set_queue(vec![archive_song_path("demo-live", "01 Opening.flac")], 0, true);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !r.seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })) {
        while let Ok(e) = r.events.try_recv() {
            r.seen.push(e);
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(r.seen.iter().any(|e| matches!(e, PlayerEvent::Error { .. })), "an error is reported");
    assert_eq!(s.hits.load(Ordering::SeqCst), 0, "no connection was made");
}
