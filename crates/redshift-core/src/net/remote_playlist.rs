//! Reads a public playlist from a link (Deezer, Spotify or YouTube / YouTube Music) so it can be
//! matched against the library; see `playlist_import`.
//!
//! What is sent: the playlist's own address, as an ordinary HTTPS request to the service, with the IP
//! address and the app's name and version, as with any web request. No account, key or cookie is
//! used, so only playlists that anyone with the link can open work. Nothing is asked until the user
//! pastes a link and presses the button, and only while online features are on. Nothing is uploaded:
//! the playlist is read, never changed.
//!
//! * **Deezer** has an open JSON interface and is the dependable one (any length).
//! * **Spotify** and **YouTube** have no open interface for this, so the public page that the
//!   service itself shows is read. Spotify's shows the first 100 songs; YouTube's the first 100
//!   videos. These page formats are not promised by the services and can change; when they do, the
//!   import says it could not read the page rather than guessing.

use std::time::Duration;

use serde_json::Value;

use super::{Net, Request};
use crate::error::{Error, Result};

pub const DEEZER_HOST: &str = "api.deezer.com";
pub const SPOTIFY_HOST: &str = "open.spotify.com";
pub const YOUTUBE_HOST: &str = "www.youtube.com";

/// Hosts that [`fetch`] talks to, for the network layer's allow-list.
pub const HOSTS: [&str; 3] = [DEEZER_HOST, SPOTIFY_HOST, YOUTUBE_HOST];

const DEEZER: &str = "https://api.deezer.com";
const SPOTIFY: &str = "https://open.spotify.com";
const YOUTUBE: &str = "https://www.youtube.com";

/// Playlists are read for a few minutes only: long enough for "try again" not to ask twice.
const TTL: Duration = Duration::from_secs(5 * 60);
/// Most songs taken from one playlist.
pub const MAX_ENTRIES: usize = 2000;
const DEEZER_PAGE: usize = 100;

/// One song as the other service lists it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Entry {
    pub title: String,
    pub artist: String,
    /// 0 when unknown.
    pub duration_secs: u32,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct RemoteList {
    /// "Deezer", "Spotify", "YouTube", or "Pasted list".
    pub source: String,
    pub name: String,
    pub entries: Vec<Entry>,
    /// The service only showed part of a longer playlist.
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Source {
    Deezer(String),
    Spotify(String),
    YouTube(String),
}

fn split_link(link: &str) -> Option<(String, Vec<String>, String)> {
    let link = link.trim();
    let rest = link.strip_prefix("https://").or_else(|| link.strip_prefix("http://")).unwrap_or(link);
    if rest.contains(char::is_whitespace) {
        return None;
    }
    let rest = rest.split('#').next()?;
    let (before, query) = rest.split_once('?').unwrap_or((rest, ""));
    let mut parts = before.split('/');
    let host = parts.next()?.to_ascii_lowercase();
    let host = host.strip_prefix("www.").unwrap_or(&host).to_string();
    Some((host, parts.filter(|s| !s.is_empty()).map(str::to_string).collect(), query.to_string()))
}

/// Which service a link belongs to and the playlist's id in it. `None` for anything else, so a
/// stray address is never fetched.
pub fn detect(link: &str) -> Option<Source> {
    let (host, path, query) = split_link(link)?;
    let after_playlist = || path.iter().position(|s| s == "playlist").and_then(|i| path.get(i + 1)).cloned();
    match host.as_str() {
        "deezer.com" => after_playlist().filter(|id| !id.is_empty() && id.chars().all(|c| c.is_ascii_digit())).map(Source::Deezer),
        "open.spotify.com" => after_playlist().filter(|id| id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric())).map(Source::Spotify),
        "youtube.com" | "music.youtube.com" | "m.youtube.com" => query
            .split('&')
            .find_map(|kv| kv.strip_prefix("list="))
            .filter(|id| id.len() >= 10 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'))
            .map(|id| Source::YouTube(id.to_string())),
        _ => None,
    }
}

/// Reads the playlist behind a link that [`detect`] recognised.
pub fn fetch(net: &Net, source: &Source) -> Result<RemoteList> {
    match source {
        Source::Deezer(id) => deezer_at(net, DEEZER, id),
        Source::Spotify(id) => spotify_at(net, SPOTIFY, id),
        Source::YouTube(id) => youtube_at(net, YOUTUBE, id),
    }
}

fn get(net: &Net, url: &str, purpose: &str) -> Result<String> {
    let r = net.get(&Request::new(url, purpose).cached_for(TTL).misses_for(TTL))?;
    match r.status {
        200 => Ok(r.body),
        404 | 403 => Err(Error::Network("that playlist could not be opened. It may be private or deleted; only public playlists work.".into())),
        s => Err(Error::Network(format!("the service answered {s}"))),
    }
}

fn unreadable(what: &str) -> Error {
    Error::Network(format!("could not read the {what} playlist page. The service may have changed it; paste the list as text instead."))
}

fn text<'a>(v: &'a Value, key: &str) -> &'a str {
    v.get(key).and_then(Value::as_str).unwrap_or("").trim()
}

// ---- Deezer ----

pub(crate) fn deezer_at(net: &Net, base: &str, id: &str) -> Result<RemoteList> {
    let head: Value = serde_json::from_str(&get(net, &format!("{base}/playlist/{id}"), "Playlist import (Deezer)")?).map_err(|_| unreadable("Deezer"))?;
    if head.get("error").is_some() {
        return Err(Error::Network("that Deezer playlist could not be opened. It may be private or deleted; only public playlists work.".into()));
    }
    let mut out = RemoteList { source: "Deezer".into(), name: text(&head, "title").to_string(), ..Default::default() };
    let total = head.get("nb_tracks").and_then(Value::as_u64).unwrap_or(0) as usize;
    let mut index = 0;
    while index < MAX_ENTRIES {
        let url = format!("{base}/playlist/{id}/tracks?index={index}&limit={DEEZER_PAGE}");
        let page: Value = serde_json::from_str(&get(net, &url, "Playlist import (Deezer)")?).map_err(|_| unreadable("Deezer"))?;
        let Some(items) = page.get("data").and_then(Value::as_array) else { break };
        if items.is_empty() {
            break;
        }
        for t in items {
            let title = text(t, "title");
            if title.is_empty() {
                continue;
            }
            let artist = t.get("artist").map(|a| text(a, "name")).unwrap_or("");
            out.entries.push(Entry { title: title.into(), artist: artist.into(), duration_secs: t.get("duration").and_then(Value::as_u64).unwrap_or(0) as u32 });
        }
        index += items.len();
        if page.get("next").is_none() {
            break;
        }
    }
    out.entries.truncate(MAX_ENTRIES);
    out.truncated = total > out.entries.len();
    if out.entries.is_empty() {
        return Err(Error::Network("that Deezer playlist has no songs that can be listed".into()));
    }
    Ok(out)
}

// ---- Spotify ----

/// The JSON that a page carries in `<script id="…">`.
fn script_json(html: &str, id: &str) -> Option<Value> {
    let at = html.find(&format!("id=\"{id}\""))?;
    let from = at + html[at..].find('>')? + 1;
    let to = from + html[from..].find("</script>")?;
    serde_json::from_str(&html[from..to]).ok()
}

/// The first value stored under `key`, anywhere in the document.
fn find_key<'a>(v: &'a Value, key: &str) -> Option<&'a Value> {
    match v {
        Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|c| find_key(c, key))),
        Value::Array(a) => a.iter().find_map(|c| find_key(c, key)),
        _ => None,
    }
}

fn find_all<'a>(v: &'a Value, key: &str, out: &mut Vec<&'a Value>) {
    match v {
        Value::Object(m) => {
            for (k, c) in m {
                if k == key {
                    out.push(c);
                } else {
                    find_all(c, key, out);
                }
            }
        }
        Value::Array(a) => a.iter().for_each(|c| find_all(c, key, out)),
        _ => {}
    }
}

pub(crate) fn spotify_at(net: &Net, base: &str, id: &str) -> Result<RemoteList> {
    let html = get(net, &format!("{base}/embed/playlist/{id}"), "Playlist import (Spotify)")?;
    let data = script_json(&html, "__NEXT_DATA__").ok_or_else(|| unreadable("Spotify"))?;
    let list = find_key(&data, "trackList").and_then(Value::as_array).ok_or_else(|| unreadable("Spotify"))?;
    let name = find_key(&data, "entity").map(|e| if text(e, "name").is_empty() { text(e, "title") } else { text(e, "name") }).unwrap_or("").to_string();
    let entries: Vec<Entry> = list
        .iter()
        .filter_map(|t| {
            let title = text(t, "title");
            (!title.is_empty()).then(|| Entry {
                title: title.into(),
                // "A, B" with a no-break space after the comma
                artist: text(t, "subtitle").replace('\u{a0}', " "),
                duration_secs: (t.get("duration").and_then(Value::as_u64).unwrap_or(0) / 1000) as u32,
            })
        })
        .collect();
    if entries.is_empty() {
        return Err(unreadable("Spotify"));
    }
    // The page lists 100 songs at most.
    let truncated = entries.len() >= 100;
    Ok(RemoteList { source: "Spotify".into(), name, entries, truncated })
}

// ---- YouTube ----

fn runs_text(v: Option<&Value>) -> String {
    let Some(v) = v else { return String::new() };
    if let Some(s) = v.get("simpleText").and_then(Value::as_str) {
        return s.trim().to_string();
    }
    v.get("runs").and_then(Value::as_array).map(|r| r.iter().filter_map(|x| x.get("text").and_then(Value::as_str)).collect::<String>().trim().to_string()).unwrap_or_default()
}

/// Drops the decoration that video titles carry: "(Official Video)", "[Lyrics]", "HD" and so on.
pub fn clean_video_title(title: &str) -> String {
    const JUNK: [&str; 7] = ["official", "video", "audio", "lyric", "visuali", "explicit", "clip"];
    const JUNK_EXACT: [&str; 5] = ["hd", "hq", "4k", "mv", "m/v"];
    let mut out = String::new();
    let mut depth_close: Option<char> = None;
    let mut group = String::new();
    for c in title.chars() {
        match (depth_close, c) {
            (None, '(') => (depth_close, group) = (Some(')'), String::new()),
            (None, '[') => (depth_close, group) = (Some(']'), String::new()),
            (Some(close), c) if c == close => {
                let g = group.to_lowercase();
                if !JUNK.iter().any(|j| g.split(|ch: char| !ch.is_alphanumeric() && ch != '/').any(|w| w.starts_with(j)))
                    && !JUNK_EXACT.iter().any(|j| g.split(|ch: char| !ch.is_alphanumeric() && ch != '/').any(|w| w == *j))
                {
                    out.push(if close == ')' { '(' } else { '[' });
                    out.push_str(&group);
                    out.push(close);
                }
                depth_close = None;
            }
            (Some(_), c) => group.push(c),
            (None, c) => out.push(c),
        }
    }
    if depth_close.is_some() {
        out.push('(');
        out.push_str(&group);
    }
    out.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Artist and title for a video: "Artist - Topic" channels carry the song's own title; otherwise
/// "Artist - Title" is the common way to name a video.
pub fn video_entry(title: &str, channel: &str, secs: u32) -> Entry {
    let title = clean_video_title(title);
    let channel = channel.trim();
    if let Some(artist) = channel.strip_suffix(" - Topic") {
        return Entry { title, artist: artist.trim().into(), duration_secs: secs };
    }
    for sep in [" - ", " – ", " — "] {
        if let Some((a, t)) = title.split_once(sep) {
            if !a.trim().is_empty() && !t.trim().is_empty() {
                return Entry { title: t.trim().into(), artist: a.trim().into(), duration_secs: secs };
            }
        }
    }
    let artist = channel.trim_end_matches("VEVO").trim_end_matches("Official").trim();
    Entry { title, artist: artist.into(), duration_secs: secs }
}

pub(crate) fn youtube_at(net: &Net, base: &str, id: &str) -> Result<RemoteList> {
    let html = get(net, &format!("{base}/playlist?list={id}"), "Playlist import (YouTube)")?;
    let at = html.find("ytInitialData").ok_or_else(|| unreadable("YouTube"))?;
    let from = at + html[at..].find('{').ok_or_else(|| unreadable("YouTube"))?;
    let data: Value = serde_json::Deserializer::from_str(&html[from..]).into_iter::<Value>().next().and_then(|r| r.ok()).ok_or_else(|| unreadable("YouTube"))?;
    let name = find_key(&data, "playlistMetadataRenderer").map(|m| text(m, "title").to_string()).unwrap_or_default();
    let mut found = Vec::new();
    find_all(&data, "playlistVideoRenderer", &mut found);
    let entries: Vec<Entry> = found
        .into_iter()
        .filter_map(|r| {
            let title = runs_text(r.get("title"));
            let low = title.to_lowercase();
            if title.is_empty() || low == "[deleted video]" || low == "[private video]" {
                return None;
            }
            let secs = r.get("lengthSeconds").and_then(Value::as_str).and_then(|s| s.parse().ok()).unwrap_or(0);
            Some(video_entry(&title, &runs_text(r.get("shortBylineText")), secs))
        })
        .collect();
    if entries.is_empty() {
        return Err(unreadable("YouTube"));
    }
    let truncated = entries.len() >= 100;
    Ok(RemoteList { source: "YouTube".into(), name, entries, truncated })
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

    #[test]
    fn links_are_recognised_and_strays_are_not() {
        let d = |s: &str| detect(s);
        assert_eq!(d("https://www.deezer.com/en/playlist/1313621735"), Some(Source::Deezer("1313621735".into())));
        assert_eq!(d("deezer.com/playlist/42?utm_source=x"), Some(Source::Deezer("42".into())));
        assert_eq!(d("https://open.spotify.com/playlist/37i9dQZF1DXcBWIGoYBM5M?si=abc"), Some(Source::Spotify("37i9dQZF1DXcBWIGoYBM5M".into())));
        assert_eq!(d("https://open.spotify.com/intl-de/playlist/37i9dQZF1DXcBWIGoYBM5M"), Some(Source::Spotify("37i9dQZF1DXcBWIGoYBM5M".into())));
        assert_eq!(d("https://music.youtube.com/playlist?list=PLabcdefghijk123&si=1"), Some(Source::YouTube("PLabcdefghijk123".into())));
        assert_eq!(d("https://www.youtube.com/watch?v=xyz&list=PLabcdefghijk123"), Some(Source::YouTube("PLabcdefghijk123".into())));
        assert_eq!(d("https://www.youtube.com/watch?v=xyz"), None);
        assert_eq!(d("https://evil.example.com/playlist/123"), None);
        assert_eq!(d("https://deezer.com.evil.example/playlist/123"), None);
        assert_eq!(d("https://open.spotify.com/album/37i9dQZF1DXcBWIGoYBM5M"), None);
        assert_eq!(d("Artist - Song\nOther - Thing"), None);
    }

    #[test]
    fn deezer_pages_are_followed() {
        let d = tempfile::tempdir().unwrap();
        let head = r#"{"id":1,"title":"Road Trip","nb_tracks":3}"#;
        let p1 = r#"{"data":[{"title":"One","duration":181,"artist":{"name":"A"}},{"title":"Two","duration":200,"artist":{"name":"B"}}],"total":3,"next":"https://api.deezer.com/x"}"#;
        let p2 = r#"{"data":[{"title":"Three","duration":90,"artist":{"name":"C"}}],"total":3}"#;
        let s = start(vec![reply(200, head), reply(200, p1), reply(200, p2)]);
        let got = deezer_at(&net(d.path()), &s.base, "1").unwrap();
        assert_eq!(got.name, "Road Trip");
        assert_eq!(got.source, "Deezer");
        assert_eq!(got.entries.len(), 3);
        assert_eq!(got.entries[0], Entry { title: "One".into(), artist: "A".into(), duration_secs: 181 });
        assert!(!got.truncated);
        let sent = s.requests.lock()[1].clone();
        assert!(sent.starts_with("GET /playlist/1/tracks?index=0&limit=100 "), "{sent}");
    }

    #[test]
    fn deezer_errors_say_private_or_deleted() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, r#"{"error":{"type":"DataException","message":"no data","code":800}}"#)]);
        let e = deezer_at(&net(d.path()), &s.base, "1").unwrap_err().to_string();
        assert!(e.contains("private or deleted"), "{e}");
    }

    #[test]
    fn spotify_embed_pages_are_read() {
        let d = tempfile::tempdir().unwrap();
        let json = r#"{"props":{"pageProps":{"state":{"data":{"entity":{"name":"Night Drive","trackList":[
            {"title":"Carrier Wave","subtitle":"Paper Satellites, Someone","duration":215000},
            {"title":"Low Orbit","subtitle":"Paper Satellites","duration":180500}]}}}}}}"#;
        let html = format!(r#"<html><body><script id="__NEXT_DATA__" type="application/json">{json}</script></body></html>"#);
        let s = start(vec![reply(200, &html)]);
        let got = spotify_at(&net(d.path()), &s.base, "37i9dQZF1DXcBWIGoYBM5M").unwrap();
        assert_eq!(got.name, "Night Drive");
        assert_eq!(got.entries.len(), 2);
        assert_eq!(got.entries[0], Entry { title: "Carrier Wave".into(), artist: "Paper Satellites, Someone".into(), duration_secs: 215 });
        assert!(!got.truncated);
    }

    #[test]
    fn a_page_in_a_format_nobody_expected_is_reported_not_guessed() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, "<html>nothing here</html>")]);
        let e = spotify_at(&net(d.path()), &s.base, "37i9dQZF1DXcBWIGoYBM5M").unwrap_err().to_string();
        assert!(e.contains("paste the list as text"), "{e}");
    }

    #[test]
    fn youtube_pages_are_read_and_titles_cleaned() {
        let d = tempfile::tempdir().unwrap();
        let data = r#"{"metadata":{"playlistMetadataRenderer":{"title":"Mix"}},"contents":[
          {"playlistVideoRenderer":{"title":{"runs":[{"text":"Paper Satellites - Carrier Wave (Official Video) [HD]"}]},"shortBylineText":{"runs":[{"text":"PaperSatVEVO"}]},"lengthSeconds":"215"}},
          {"playlistVideoRenderer":{"title":{"runs":[{"text":"Low Orbit"}]},"shortBylineText":{"runs":[{"text":"Paper Satellites - Topic"}]},"lengthSeconds":"180"}},
          {"playlistVideoRenderer":{"title":{"runs":[{"text":"[Private video]"}]},"shortBylineText":{"runs":[]},"lengthSeconds":"0"}}]}"#;
        let html = format!("<html><script>var ytInitialData = {data};</script><script>other()</script></html>");
        let s = start(vec![reply(200, &html)]);
        let got = youtube_at(&net(d.path()), &s.base, "PLabcdefghijk123").unwrap();
        assert_eq!(got.name, "Mix");
        assert_eq!(got.entries, vec![
            Entry { title: "Carrier Wave".into(), artist: "Paper Satellites".into(), duration_secs: 215 },
            Entry { title: "Low Orbit".into(), artist: "Paper Satellites".into(), duration_secs: 180 },
        ]);
    }

    #[test]
    fn video_titles_lose_their_decoration_but_keep_remixes() {
        assert_eq!(clean_video_title("Song (Official Music Video)"), "Song");
        assert_eq!(clean_video_title("Song [Lyrics] (HD)"), "Song");
        assert_eq!(clean_video_title("Song (Club Remix)"), "Song (Club Remix)");
        assert_eq!(clean_video_title("Song (feat. Someone)"), "Song (feat. Someone)");
        assert_eq!(clean_video_title("Unclosed (bracket"), "Unclosed (bracket");
    }

    #[test]
    fn a_private_playlist_is_explained() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(404, "")]);
        let e = youtube_at(&net(d.path()), &s.base, "PLabcdefghijk123").unwrap_err().to_string();
        assert!(e.contains("private"), "{e}");
    }

    #[test]
    fn offline_stays_offline() {
        let d = tempfile::tempdir().unwrap();
        let n = net(d.path());
        n.set_enabled(false);
        assert!(matches!(fetch(&n, &Source::Deezer("1".into())), Err(Error::Offline)));
    }
}
