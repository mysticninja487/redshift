//! Album cover pictures for the Discord card, from the iTunes Search service (itunes.apple.com),
//! which needs no account or key.
//!
//! What is sent: the artist and the title (or album) of the song as the search text of an ordinary
//! HTTPS request, with the IP address and the app's name and version, as with any web request. It is
//! only ever asked when the Discord card is switched on with a cover picture and online features are
//! on. The picture itself is never downloaded by Redshift: Discord fetches it from its address.

use std::time::Duration;

use serde::Deserialize;

use super::{encode_query, Net, Request};
use crate::error::{Error, Result};

pub const HOST: &str = "itunes.apple.com";
const BASE: &str = "https://itunes.apple.com";
const HIT_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const MISS_TTL: Duration = Duration::from_secs(24 * 3600);

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: String,
}

#[derive(Debug, Deserialize, Default)]
#[serde(default)]
struct Reply {
    results: Vec<Item>,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
struct Item {
    track_name: Option<String>,
    artist_name: Option<String>,
    collection_name: Option<String>,
    artwork_url100: Option<String>,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// A bigger picture than the 100 pixel one the service lists, on Apple's own picture host only.
fn large(url: &str) -> Option<String> {
    let rest = url.strip_prefix("https://")?;
    let host = rest.split('/').next()?;
    if !(host == "mzstatic.com" || host.ends_with(".mzstatic.com")) {
        return None;
    }
    let big = url.replace("100x100bb", "600x600bb");
    (big.len() <= 256).then_some(big)
}

/// The closest hit that is certainly this artist and this song or album.
fn best(items: Vec<Item>, q: &Query) -> Option<String> {
    let (artist, title, album) = (norm(&q.artist), norm(&q.title), norm(&q.album));
    let same_artist = |i: &Item| i.artist_name.as_deref().map(norm).is_some_and(|a| !a.is_empty() && (a == artist || a.contains(&artist) || artist.contains(&a)));
    let same_album = |i: &Item| !album.is_empty() && i.collection_name.as_deref().map(norm).is_some_and(|c| c == album);
    let same_title = |i: &Item| i.track_name.as_deref().map(norm).is_some_and(|t| t == title);
    let usable = |i: &&Item| i.artwork_url100.as_deref().and_then(large).is_some();
    let items: Vec<Item> = items.into_iter().filter(|i| same_artist(i)).collect();
    // the right album first, then the right song on any release
    items
        .iter()
        .filter(usable)
        .find(|i| same_album(i) && same_title(i))
        .or_else(|| items.iter().filter(usable).find(|i| same_album(i)))
        .or_else(|| items.iter().filter(usable).find(|i| same_title(i)))
        .and_then(|i| i.artwork_url100.as_deref().and_then(large))
}

/// Looks the cover up. `Ok(None)` means nothing certain was found.
pub fn lookup(net: &Net, q: &Query) -> Result<Option<String>> {
    lookup_at(net, BASE, q)
}

pub(crate) fn lookup_at(net: &Net, base: &str, q: &Query) -> Result<Option<String>> {
    if q.artist.trim().is_empty() || (q.title.trim().is_empty() && q.album.trim().is_empty()) {
        return Ok(None);
    }
    let term = format!("{} {}", q.artist.trim(), if q.title.trim().is_empty() { q.album.trim() } else { q.title.trim() });
    let url = format!("{base}/search?term={}&media=music&entity=song&limit=10", encode_query(&term));
    let r = net.get(&Request::new(&url, "Album cover for Discord").cached_for(HIT_TTL).misses_for(MISS_TTL))?;
    match r.status {
        200 => {
            let reply: Reply = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable cover reply: {e}")))?;
            Ok(best(reply.results, q))
        }
        404 => Ok(None),
        s => Err(Error::Network(format!("the cover service answered {s}"))),
    }
}

#[cfg(test)]
mod tests {
    use super::super::testserver::*;
    use super::*;

    fn net(dir: &std::path::Path) -> Net {
        let n = Net::new(dir.join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        n
    }

    fn q() -> Query {
        Query { title: "Carrier Wave".into(), artist: "Paper Satellites".into(), album: "Signal & Noise".into() }
    }

    fn item(track: &str, artist: &str, album: &str, art: &str) -> String {
        format!(r#"{{"trackName":"{track}","artistName":"{artist}","collectionName":"{album}","artworkUrl100":"{art}"}}"#)
    }

    #[test]
    fn the_matching_album_wins_and_the_picture_is_made_bigger() {
        let d = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"results":[{},{},{}]}}"#,
            item("Carrier Wave", "Someone Else", "Signal & Noise", "https://is1-ssl.mzstatic.com/a/100x100bb.jpg"),
            item("Carrier Wave (Live)", "Paper Satellites", "Live", "https://is1-ssl.mzstatic.com/b/100x100bb.jpg"),
            item("Carrier Wave", "Paper Satellites", "Signal & Noise", "https://is1-ssl.mzstatic.com/c/100x100bb.jpg"),
        );
        let s = start(vec![reply(200, &body)]);
        let n = net(d.path());
        let got = lookup_at(&n, &s.base, &q()).unwrap();
        assert_eq!(got.as_deref(), Some("https://is1-ssl.mzstatic.com/c/600x600bb.jpg"));
        let sent = s.requests.lock()[0].clone();
        assert!(sent.starts_with("GET /search?term=Paper%20Satellites%20Carrier%20Wave&media=music&entity=song&limit=10 "), "{sent}");
    }

    #[test]
    fn a_wrong_artist_or_a_picture_from_somewhere_else_is_not_used() {
        let d = tempfile::tempdir().unwrap();
        let body = format!(
            r#"{{"results":[{},{}]}}"#,
            item("Carrier Wave", "Someone Else", "Signal & Noise", "https://is1-ssl.mzstatic.com/a/100x100bb.jpg"),
            item("Carrier Wave", "Paper Satellites", "Signal & Noise", "https://evil.example.com/x/100x100bb.jpg"),
        );
        let s = start(vec![reply(200, &body)]);
        assert_eq!(lookup_at(&net(d.path()), &s.base, &q()).unwrap(), None);
    }

    #[test]
    fn nothing_is_asked_without_an_artist_and_while_offline() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![]);
        let n = net(d.path());
        assert_eq!(lookup_at(&n, &s.base, &Query { artist: " ".into(), ..q() }).unwrap(), None);
        n.set_enabled(false);
        assert!(matches!(lookup_at(&n, &s.base, &q()), Err(Error::Offline)));
    }

    #[test]
    fn an_error_from_the_service_is_an_error() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(500, "oops")]);
        assert!(lookup_at(&net(d.path()), &s.base, &q()).is_err());
    }
}
