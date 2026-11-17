//! Lyrics from LRCLIB (https://lrclib.net), a free community lyrics database with no account or key.
//!
//! What is sent: the song's title, artist, album (when known) and length in seconds, as the query
//! of an ordinary HTTPS request, together with the IP address and the app's name and version, as
//! with any web request. Nothing else about the song or the library leaves the computer.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{encode_query, Net, Request};
use crate::error::{Error, Result};

pub const HOST: &str = "lrclib.net";
const BASE: &str = "https://lrclib.net/api";
/// How long a found answer is kept, and how long a "nothing there" is believed.
const HIT_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const MISS_TTL: Duration = Duration::from_secs(2 * 24 * 3600);
/// A search result whose length differs from the song's by more than this is a different recording.
const MAX_DURATION_DIFF_SECS: f64 = 4.0;

/// What to look for.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

/// What was found. `synced` is LRC text with timestamps, `plain` is the same words without.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Found {
    pub synced: Option<String>,
    pub plain: Option<String>,
    pub instrumental: bool,
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Item {
    track_name: Option<String>,
    artist_name: Option<String>,
    album_name: Option<String>,
    duration: Option<f64>,
    instrumental: bool,
    plain_lyrics: Option<String>,
    synced_lyrics: Option<String>,
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|t| !t.trim().is_empty())
}

impl Item {
    fn into_found(self, q: &Query) -> Option<Found> {
        let synced = nonempty(self.synced_lyrics);
        let plain = nonempty(self.plain_lyrics);
        if synced.is_none() && plain.is_none() && !self.instrumental {
            return None;
        }
        Some(Found {
            synced,
            plain,
            instrumental: self.instrumental,
            title: self.track_name.unwrap_or_else(|| q.title.clone()),
            artist: self.artist_name.unwrap_or_else(|| q.artist.clone()),
            album: self.album_name,
            duration_secs: self.duration,
        })
    }

    fn has_lyrics(&self) -> bool {
        self.instrumental || self.synced_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty()) || self.plain_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty())
    }
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// Picks the best search result for `q`: it must be the same song (title and artist match once
/// punctuation and case are ignored) and about the same length; synced beats plain, and a matching
/// album breaks ties.
fn best(items: Vec<Item>, q: &Query) -> Option<Item> {
    let (title, artist) = (norm(&q.title), norm(&q.artist));
    let album = q.album.as_deref().map(norm).filter(|a| !a.is_empty());
    items
        .into_iter()
        .filter(|i| i.has_lyrics())
        .filter(|i| i.track_name.as_deref().map(norm).is_some_and(|t| t == title))
        .filter(|i| i.artist_name.as_deref().map(norm).is_some_and(|a| a == artist || a.contains(&artist) || artist.contains(&a)))
        .filter(|i| match (q.duration_secs, i.duration) {
            (Some(want), Some(got)) => (want - got).abs() <= MAX_DURATION_DIFF_SECS,
            _ => true,
        })
        .max_by_key(|i| {
            let synced = i.synced_lyrics.as_deref().is_some_and(|t| !t.trim().is_empty()) as u8;
            let album_ok = (album.is_some() && i.album_name.as_deref().map(norm) == album) as u8;
            let close = q.duration_secs.zip(i.duration).map_or(0, |(w, g)| 100 - ((w - g).abs() * 10.0) as i64);
            (synced, album_ok, close)
        })
}

fn get_query(q: &Query) -> String {
    let mut s = format!("track_name={}&artist_name={}", encode_query(&q.title), encode_query(&q.artist));
    if let Some(a) = q.album.as_deref().filter(|a| !a.trim().is_empty()) {
        s.push_str(&format!("&album_name={}", encode_query(a)));
    }
    if let Some(d) = q.duration_secs.filter(|d| (1.0..=3600.0).contains(d)) {
        s.push_str(&format!("&duration={}", d.round() as u32));
    }
    s
}

/// Looks the song up. `Ok(None)` means the service has nothing for it.
pub fn lookup(net: &Net, q: &Query) -> Result<Option<Found>> {
    lookup_at(net, BASE, q)
}

pub(crate) fn lookup_at(net: &Net, base: &str, q: &Query) -> Result<Option<Found>> {
    if q.title.trim().is_empty() || q.artist.trim().is_empty() {
        return Ok(None);
    }
    let exact = format!("{base}/get?{}", get_query(q));
    let r = net.get(&Request::new(&exact, "Lyrics").cached_for(HIT_TTL).misses_for(MISS_TTL))?;
    match r.status {
        200 => {
            let item: Item = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable lyrics reply: {e}")))?;
            return Ok(item.into_found(q));
        }
        404 => {}
        s => return Err(Error::Network(format!("the lyrics service answered {s}"))),
    }
    // Not under that exact album and length: search by name and pick the closest recording.
    let search = format!("{base}/search?track_name={}&artist_name={}", encode_query(&q.title), encode_query(&q.artist));
    let r = net.get(&Request::new(&search, "Lyrics search").cached_for(HIT_TTL).misses_for(MISS_TTL))?;
    match r.status {
        200 => {
            let items: Vec<Item> = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable lyrics reply: {e}")))?;
            Ok(best(items, q).and_then(|i| i.into_found(q)))
        }
        404 => Ok(None),
        s => Err(Error::Network(format!("the lyrics service answered {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testserver::*;
    use super::*;
    use std::sync::atomic::Ordering;

    fn net(dir: &std::path::Path) -> Net {
        let n = Net::new(dir.join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        n
    }

    fn q() -> Query {
        Query { title: "Limbo".into(), artist: "Haruno".into(), album: Some("Limbo".into()), duration_secs: Some(201.4) }
    }

    const SYNCED: &str = r#"{"id":1,"trackName":"Limbo","artistName":"Haruno","albumName":"Limbo","duration":201.0,"instrumental":false,"plainLyrics":"hello\nworld","syncedLyrics":"[00:01.00]hello\n[00:05.00]world"}"#;

    #[test]
    fn exact_match_is_returned_and_the_query_is_encoded() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, SYNCED)]);
        let n = net(d.path());
        let f = lookup_at(&n, &format!("{}/api", s.base), &q()).unwrap().unwrap();
        assert!(f.synced.as_deref().unwrap().starts_with("[00:01.00]hello"));
        assert_eq!(f.plain.as_deref(), Some("hello\nworld"));
        let sent = s.requests.lock()[0].clone();
        assert!(sent.starts_with("GET /api/get?track_name=Limbo&artist_name=Haruno&album_name=Limbo&duration=201 "), "{sent}");
    }

    #[test]
    fn falls_back_to_search_and_picks_the_closest_recording() {
        let d = tempfile::tempdir().unwrap();
        let list = r#"[
          {"trackName":"Limbo (Live)","artistName":"Haruno","duration":201.0,"plainLyrics":"live","syncedLyrics":null},
          {"trackName":"Limbo","artistName":"Someone Else","duration":201.0,"plainLyrics":"wrong artist","syncedLyrics":null},
          {"trackName":"Limbo","artistName":"Haruno","albumName":"Other","duration":260.0,"plainLyrics":"too long","syncedLyrics":null},
          {"trackName":"limbo","artistName":"Haruno","albumName":"Limbo","duration":199.0,"plainLyrics":"plain only","syncedLyrics":null},
          {"trackName":"Limbo","artistName":"Haruno","albumName":"Limbo","duration":202.0,"plainLyrics":"plain","syncedLyrics":"[00:02.00]synced"}
        ]"#;
        let s = start(vec![reply(404, r#"{"statusCode":404}"#), reply(200, list)]);
        let n = net(d.path());
        let f = lookup_at(&n, &format!("{}/api", s.base), &q()).unwrap().unwrap();
        assert_eq!(f.synced.as_deref(), Some("[00:02.00]synced"));
        assert_eq!(s.hits.load(Ordering::SeqCst), 2);
        assert!(s.requests.lock()[1].starts_with("GET /api/search?track_name=Limbo&artist_name=Haruno "));
    }

    #[test]
    fn a_different_song_is_never_returned() {
        let d = tempfile::tempdir().unwrap();
        let list = r#"[{"trackName":"Limbo","artistName":"Haruno","duration":400.0,"plainLyrics":"x"},{"trackName":"Other","artistName":"Haruno","duration":201.0,"plainLyrics":"y"}]"#;
        let s = start(vec![reply(404, "{}"), reply(200, list)]);
        let n = net(d.path());
        assert_eq!(lookup_at(&n, &format!("{}/api", s.base), &q()).unwrap(), None);
    }

    #[test]
    fn nothing_found_is_none_and_remembered() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(404, "{}"), reply(200, "[]")]);
        let n = net(d.path());
        let base = format!("{}/api", s.base);
        assert_eq!(lookup_at(&n, &base, &q()).unwrap(), None);
        assert_eq!(lookup_at(&n, &base, &q()).unwrap(), None);
        assert_eq!(s.hits.load(Ordering::SeqCst), 2, "the second lookup came from the cache");
    }

    #[test]
    fn instrumental_tracks_are_reported_as_such() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"trackName":"Limbo","artistName":"Haruno","instrumental":true,"plainLyrics":null,"syncedLyrics":null}"#)]);
        let n = net(d.path());
        let f = lookup_at(&n, &format!("{}/api", s.base), &q()).unwrap().unwrap();
        assert!(f.instrumental && f.synced.is_none() && f.plain.is_none());
    }

    #[test]
    fn empty_lyrics_count_as_nothing() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"trackName":"Limbo","artistName":"Haruno","instrumental":false,"plainLyrics":"  ","syncedLyrics":""}"#)]);
        let n = net(d.path());
        assert_eq!(lookup_at(&n, &format!("{}/api", s.base), &q()).unwrap(), None);
    }

    #[test]
    fn missing_title_or_artist_asks_nobody() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, SYNCED)]);
        let n = net(d.path());
        let base = format!("{}/api", s.base);
        assert_eq!(lookup_at(&n, &base, &Query { title: "Limbo".into(), ..Default::default() }).unwrap(), None);
        assert_eq!(lookup_at(&n, &base, &Query { artist: "Haruno".into(), ..Default::default() }).unwrap(), None);
        assert_eq!(s.hits.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn server_errors_and_garbage_are_errors_not_empty_results() {
        let d = tempfile::tempdir().unwrap();
        let n = net(d.path());
        let s = start(vec![reply(500, "oops")]);
        assert!(matches!(lookup_at(&n, &format!("{}/api", s.base), &q()), Err(Error::Network(_))));
        let s = start(vec![reply(200, "<html>")]);
        assert!(matches!(lookup_at(&n, &format!("{}/api", s.base), &q()), Err(Error::Network(_))));
    }

    #[test]
    fn offline_stays_offline() {
        let d = tempfile::tempdir().unwrap();
        let n = Net::new(d.path().join("c"), "t", &["127.0.0.1"]);
        assert!(matches!(lookup_at(&n, "http://127.0.0.1:1/api", &q()), Err(Error::Offline)));
    }

    #[test]
    fn duration_is_left_out_when_unknown_or_silly() {
        let mut query = q();
        query.duration_secs = None;
        assert!(!get_query(&query).contains("duration"));
        query.duration_secs = Some(99999.0);
        assert!(!get_query(&query).contains("duration"));
        query.album = Some("  ".into());
        assert!(!get_query(&query).contains("album_name"));
    }
}
