//! Finding radio stations in the Radio Browser directory (radio-browser.info), a free, community
//! run list of stations that needs no account or key.
//!
//! What is sent: the search words, filters and the app's name and version, as the address of an
//! ordinary HTTPS request. Only when the person searches. What station plays is a separate request,
//! to the station itself (see `Net::open_stream`). Several servers share the directory; if one
//! doesn't answer, the next is tried.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{encode_query, Net, Request};
use crate::error::{Error, Result};
use crate::radio::{codec_is_lossless, Station};

pub const HOSTS: [&str; 3] = ["de1.api.radio-browser.info", "de2.api.radio-browser.info", "fi1.api.radio-browser.info"];
const TTL: Duration = Duration::from_secs(10 * 60);

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Query {
    pub name: String,
    pub country: String,
    pub tag: String,
    /// Only stations with a lossless codec (FLAC).
    pub lossless_only: bool,
    pub limit: usize,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Item {
    stationuuid: String,
    name: String,
    url: String,
    url_resolved: String,
    homepage: String,
    country: String,
    tags: String,
    codec: String,
    bitrate: u32,
    hls: u8,
    lastcheckok: u8,
    votes: u32,
}

fn station(i: Item) -> Option<Station> {
    let url = if i.url_resolved.trim().is_empty() { i.url.trim().to_string() } else { i.url_resolved.trim().to_string() };
    let lower = url.to_ascii_lowercase();
    if i.name.trim().is_empty() || !(lower.starts_with("http://") || lower.starts_with("https://")) || i.lastcheckok == 0 {
        return None;
    }
    let codec = i.codec.trim().to_string();
    let lossless = codec_is_lossless(&codec);
    let heaac = codec.eq_ignore_ascii_case("AAC+");
    let _ = i.votes;
    Some(Station {
        id: i.stationuuid,
        name: i.name.trim().to_string(),
        secure: lower.starts_with("https://"),
        url,
        homepage: i.homepage.trim().to_string(),
        country: i.country.trim().to_string(),
        tags: i.tags.trim().to_string(),
        codec,
        bitrate: i.bitrate,
        lossless,
        supported: i.hls == 0 && !heaac,
    })
}

fn path_for(q: &Query) -> String {
    let limit = q.limit.clamp(1, 100);
    let mut s = format!("/json/stations/search?hidebroken=true&order=clickcount&reverse=true&limit={limit}");
    if !q.name.trim().is_empty() {
        s.push_str(&format!("&name={}", encode_query(q.name.trim())));
    }
    if !q.country.trim().is_empty() {
        s.push_str(&format!("&country={}", encode_query(q.country.trim())));
    }
    if !q.tag.trim().is_empty() {
        s.push_str(&format!("&tag={}", encode_query(q.tag.trim())));
    }
    if q.lossless_only {
        s.push_str("&codec=FLAC");
    }
    s
}

/// Searches the directory. Stations that are known to be down are left out, and so are repeats of
/// the same stream address.
pub fn search(net: &Net, q: &Query) -> Result<Vec<Station>> {
    let path = path_for(q);
    let mut last = Error::Network("no radio directory answered".into());
    for host in HOSTS {
        let url = format!("https://{host}{path}");
        match net.get(&Request::new(&url, "Radio search").cached_for(TTL)) {
            Ok(r) if r.status == 200 => {
                let items: Vec<Item> = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable station list: {e}")))?;
                let mut seen = std::collections::HashSet::new();
                return Ok(items
                    .into_iter()
                    .filter_map(station)
                    .filter(|s| !q.lossless_only || s.lossless)
                    .filter(|s| seen.insert(s.url.clone()))
                    .collect());
            }
            Ok(r) => last = Error::Network(format!("the directory answered {}", r.status)),
            Err(Error::Offline) => return Err(Error::Offline),
            Err(e) => last = e,
        }
    }
    Err(last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stations_are_read_and_down_ones_dropped() {
        let list = r#"[
          {"stationuuid":"a","name":"Paradise (FLAC)","url":"http://x/pls","url_resolved":"https://stream.example/flac","codec":"FLAC","bitrate":1411,"hls":0,"lastcheckok":1,"country":"US","tags":"eclectic,rock","homepage":"https://p.example"},
          {"stationuuid":"b","name":"Down","url":"http://x/d","url_resolved":"http://x/d","codec":"MP3","lastcheckok":0},
          {"stationuuid":"c","name":"HE","url":"http://x/h","url_resolved":"http://x/h","codec":"AAC+","bitrate":64,"lastcheckok":1},
          {"stationuuid":"d","name":"Live HLS","url":"http://x/l.m3u8","url_resolved":"http://x/l.m3u8","codec":"AAC","hls":1,"lastcheckok":1},
          {"stationuuid":"e","name":"Odd scheme","url":"rtsp://x","url_resolved":"","codec":"MP3","lastcheckok":1}]"#;
        let items: Vec<Item> = serde_json::from_str(list).unwrap();
        let got: Vec<Station> = items.into_iter().filter_map(station).collect();
        assert_eq!(got.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["Paradise (FLAC)", "HE", "Live HLS"]);
        let flac = &got[0];
        assert!(flac.lossless && flac.secure && flac.supported);
        assert_eq!(flac.url, "https://stream.example/flac", "the resolved address wins over the playlist one");
        assert!(got.iter().find(|s| s.name == "HE").is_some_and(|s| !s.supported));
        assert!(got.iter().find(|s| s.name == "Live HLS").is_some_and(|s| !s.supported));
        assert!(!got.iter().any(|s| s.name == "Down" || s.name == "Odd scheme"));
    }

    #[test]
    fn the_query_is_encoded() {
        let p = path_for(&Query { name: "Radio Paradise".into(), country: "Germany".into(), tag: "jazz & blues".into(), lossless_only: true, limit: 500 });
        assert!(p.contains("name=Radio%20Paradise") && p.contains("tag=jazz%20%26%20blues") && p.contains("codec=FLAC") && p.contains("limit=100"), "{p}");
    }
}
