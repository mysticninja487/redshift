//! A Navidrome (or any Subsonic-compatible) server: the person's own music, played over the network.
//!
//! This is the "your own server" source. It asks the server for lists (albums, artists, playlists,
//! search) and plays songs from it as the **original files** (`format=raw`, no transcoding), so a
//! lossless library stays lossless. Everything goes through [`Net`], so it needs the online switch,
//! it is logged (without the login token) and it may only talk to the one server the person set.
//!
//! The login is the Subsonic token scheme: each request carries the user name, a random salt and
//! `md5(password + salt)`, never the password itself.

use std::io::Read;
use std::sync::atomic::Ordering;
use std::time::Duration;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{encode_query, without_query, Net, RequestRecord, ServerAccess, ServerBody, ServerConfig};
use crate::error::{Error, Result};

const API_VERSION: &str = "1.16.1";
const CLIENT_NAME: &str = "Redshift";
/// The biggest list reply accepted (a page of albums or a big playlist).
const MAX_JSON: u64 = 16 * 1024 * 1024;

/// A song on the server, as the lists show it.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerSong {
    pub id: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_id: Option<String>,
    pub track: Option<u32>,
    pub disc: Option<u32>,
    pub year: Option<u32>,
    pub genre: Option<String>,
    pub duration_secs: f64,
    pub bitrate_kbps: Option<u32>,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u32>,
    pub channels: Option<u32>,
    /// File extension: `flac`, `mp3`, `m4a`, ...
    pub suffix: String,
    pub size: u64,
    pub cover_art: Option<String>,
    /// True when the file is lossless: FLAC, WAV, AIFF, APE, WavPack, or an M4A that carries a bit depth.
    pub lossless: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerAlbum {
    pub id: String,
    pub name: String,
    pub artist: String,
    pub year: Option<u32>,
    pub song_count: u32,
    pub duration_secs: f64,
    pub cover_art: Option<String>,
    pub genre: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerArtist {
    pub id: String,
    pub name: String,
    pub album_count: u32,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ServerPlaylist {
    pub id: String,
    pub name: String,
    pub song_count: u32,
    pub duration_secs: f64,
}

/// What a search found.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ServerSearch {
    pub artists: Vec<ServerArtist>,
    pub albums: Vec<ServerAlbum>,
    pub songs: Vec<ServerSong>,
}

/// What a connection test found out.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct ServerStatus {
    pub server_name: String,
    pub server_version: String,
    pub api_version: String,
    /// The address is plain http: the login token and the music cross the network unencrypted.
    pub encrypted: bool,
}

/// The ways albums can be listed.
pub const ALBUM_LISTS: [&str; 6] = ["newest", "recent", "frequent", "random", "alphabeticalByName", "starred"];

fn text(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Number(n)) => n.to_string(),
        _ => String::new(),
    }
}

fn opt_text(v: &Value, key: &str) -> Option<String> {
    Some(text(v, key)).filter(|s| !s.is_empty())
}

fn number(v: &Value, key: &str) -> Option<u32> {
    v.get(key).and_then(|n| n.as_u64().or_else(|| n.as_str().and_then(|s| s.parse().ok()))).and_then(|n| u32::try_from(n).ok()).filter(|n| *n > 0)
}

fn float(v: &Value, key: &str) -> f64 {
    v.get(key).and_then(|n| n.as_f64()).unwrap_or(0.0)
}

fn list<'a>(v: &'a Value, key: &str) -> &'a [Value] {
    v.get(key).and_then(|a| a.as_array()).map(Vec::as_slice).unwrap_or(&[])
}

/// A genre may be a plain string or, in newer servers, a list of `{name}` objects.
fn genre_of(v: &Value) -> Option<String> {
    opt_text(v, "genre").or_else(|| list(v, "genres").first().and_then(|g| opt_text(g, "name")))
}

pub fn song_from_json(v: &Value) -> Option<ServerSong> {
    let id = text(v, "id");
    if id.is_empty() || v.get("isDir").and_then(|d| d.as_bool()).unwrap_or(false) {
        return None;
    }
    let suffix = text(v, "suffix").to_ascii_lowercase();
    let bit_depth = number(v, "bitDepth");
    let lossless = matches!(suffix.as_str(), "flac" | "wav" | "wave" | "aif" | "aiff" | "ape" | "wv" | "alac" | "dsf" | "dff") || (matches!(suffix.as_str(), "m4a" | "mp4") && bit_depth.is_some());
    Some(ServerSong {
        id,
        title: text(v, "title"),
        artist: text(v, "artist"),
        album: text(v, "album"),
        album_id: opt_text(v, "albumId"),
        track: number(v, "track"),
        disc: number(v, "discNumber"),
        year: number(v, "year"),
        genre: genre_of(v),
        duration_secs: float(v, "duration"),
        bitrate_kbps: number(v, "bitRate"),
        sample_rate: number(v, "samplingRate"),
        bit_depth,
        channels: number(v, "channelCount"),
        suffix,
        size: v.get("size").and_then(|n| n.as_u64()).unwrap_or(0),
        cover_art: opt_text(v, "coverArt"),
        lossless,
    })
}

pub fn album_from_json(v: &Value) -> Option<ServerAlbum> {
    let id = text(v, "id");
    if id.is_empty() {
        return None;
    }
    Some(ServerAlbum {
        id,
        name: opt_text(v, "name").or_else(|| opt_text(v, "title")).unwrap_or_default(),
        artist: text(v, "artist"),
        year: number(v, "year"),
        song_count: number(v, "songCount").unwrap_or(0),
        duration_secs: float(v, "duration"),
        cover_art: opt_text(v, "coverArt"),
        genre: genre_of(v),
    })
}

pub fn artist_from_json(v: &Value) -> Option<ServerArtist> {
    let id = text(v, "id");
    (!id.is_empty()).then(|| ServerArtist { id, name: text(v, "name"), album_count: number(v, "albumCount").unwrap_or(0) })
}

pub fn playlist_from_json(v: &Value) -> Option<ServerPlaylist> {
    let id = text(v, "id");
    (!id.is_empty()).then(|| ServerPlaylist { id, name: text(v, "name"), song_count: number(v, "songCount").unwrap_or(0), duration_secs: float(v, "duration") })
}

/// The `subsonic-response` object, or the server's own error message.
fn unwrap_response(body: &Value) -> Result<&Value> {
    let r = body.get("subsonic-response").ok_or_else(|| Error::Network("that address did not answer like a Subsonic server (is it the server's address?)".into()))?;
    if r.get("status").and_then(|s| s.as_str()) == Some("ok") {
        return Ok(r);
    }
    let code = r.get("error").and_then(|e| e.get("code")).and_then(|c| c.as_i64()).unwrap_or(0);
    let msg = r.get("error").and_then(|e| e.get("message")).and_then(|m| m.as_str()).unwrap_or("the server reported an error");
    Err(Error::Network(match code {
        40 | 41 => "the user name or password was not accepted".to_string(),
        44 => "the server did not accept the login token (turn on the legacy password setting or use an account whose password it can check)".to_string(),
        50 => "this user may not do that".to_string(),
        70 => "the server could not find that".to_string(),
        _ => msg.to_string(),
    }))
}

/// What a server's error document (sent where a song should be) says.
pub(super) fn error_from_document(bytes: &[u8]) -> Error {
    match serde_json::from_slice::<Value>(bytes) {
        Ok(v) => match unwrap_response(&v) {
            Err(e) => e,
            Ok(_) => Error::Network("the server sent a document instead of the song".into()),
        },
        Err(_) => Error::Network("the server sent an error instead of the song".into()),
    }
}

fn md5_hex(s: &str) -> String {
    let mut h = Md5::new();
    h.update(s.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

impl Net {
    /// Sets (or with `None` or an empty config, clears) the one server Redshift may play from.
    pub fn set_server(&self, cfg: Option<&ServerConfig>) {
        let access = cfg.filter(|c| c.is_set()).and_then(|c| {
            let base = c.base_url();
            let uri: ureq::http::Uri = base.parse().ok()?;
            let scheme = uri.scheme_str()?.to_ascii_lowercase();
            if scheme != "http" && scheme != "https" {
                return None;
            }
            let authority = uri.authority()?.as_str().to_ascii_lowercase();
            // A login typed into the address (user:pass@host) is not used; drop it from the authority.
            let authority = authority.rsplit('@').next().unwrap_or(&authority).to_string();
            Some(ServerAccess { base, authority, user: c.user.trim().to_string(), password: c.password.clone() })
        });
        *self.server.write() = access;
    }

    pub fn server_is_set(&self) -> bool {
        self.server.read().is_some()
    }

    /// Whether the server's address is https (the login token and the music are then encrypted).
    pub fn server_is_encrypted(&self) -> bool {
        self.server.read().as_ref().is_some_and(|s| s.base.to_ascii_lowercase().starts_with("https://"))
    }

    fn salt(&self) -> String {
        let n = self.salt_counter.fetch_add(1, Ordering::Relaxed);
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
        let seed = format!("{t}:{n}:{:p}", self);
        md5_hex(&seed)[..12].to_string()
    }

    /// A signed address for a Subsonic endpoint (`ping`, `stream`, `getAlbum`, ...).
    pub fn server_url(&self, endpoint: &str, params: &[(&str, &str)]) -> Result<String> {
        let guard = self.server.read();
        let s = guard.as_ref().ok_or_else(|| Error::Network("no server has been set up".into()))?;
        let salt = self.salt();
        let token = md5_hex(&format!("{}{salt}", s.password));
        let mut url = format!("{}/rest/{endpoint}?u={}&t={token}&s={salt}&v={API_VERSION}&c={CLIENT_NAME}&f=json", s.base, encode_query(&s.user));
        for (k, v) in params {
            url.push('&');
            url.push_str(k);
            url.push('=');
            url.push_str(&encode_query(v));
        }
        Ok(url)
    }

    /// Checks an address is the configured server (same host and port), so a redirect or a saved
    /// path can never send the login anywhere else.
    fn check_server_url(&self, url: &str) -> Result<()> {
        let guard = self.server.read();
        let s = guard.as_ref().ok_or_else(|| Error::Network("no server has been set up".into()))?;
        let uri: ureq::http::Uri = url.parse().map_err(|_| Error::Network("not a valid address".into()))?;
        let authority = uri.authority().map(|a| a.as_str().to_ascii_lowercase()).unwrap_or_default();
        let same_scheme = url.to_ascii_lowercase().starts_with(&s.base.to_ascii_lowercase()[..s.base.find("://").unwrap_or(0) + 3]);
        if authority != s.authority || !same_scheme {
            return Err(Error::Network("that address is not your server".into()));
        }
        Ok(())
    }

    fn record_server(&self, purpose: &str, url: &str, status: Option<u16>, bytes: usize, error: Option<String>) {
        let shown = without_query(url);
        let host = shown.split("://").nth(1).and_then(|r| r.split('/').next()).unwrap_or("").to_string();
        let mut log = self.log.lock();
        if log.len() >= super::LOG_LEN {
            log.pop_front();
        }
        log.push_back(RequestRecord { time: super::now_secs(), purpose: purpose.to_string(), host, url: shown, status, bytes, cached: false, error });
    }

    /// Asks the server for a Subsonic JSON answer and returns its `subsonic-response`.
    pub fn server_json(&self, endpoint: &str, params: &[(&str, &str)], purpose: &str) -> Result<Value> {
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let url = self.server_url(endpoint, params)?;
        let result = (|| -> Result<(u16, String)> {
            self.check_server_url(&url)?;
            let mut resp = self.agent.get(&url).header("Accept", "application/json").call().map_err(|e| Error::Network(e.to_string()))?;
            let status = resp.status().as_u16();
            let body = resp.body_mut().with_config().limit(MAX_JSON).lossy_utf8(true).read_to_string().map_err(|e| Error::Network(e.to_string()))?;
            Ok((status, body))
        })();
        match result {
            Ok((status, body)) => {
                self.record_server(purpose, &url, Some(status), body.len(), None);
                if status == 401 || status == 403 {
                    return Err(Error::Network("the server refused the login".into()));
                }
                if status != 200 {
                    return Err(Error::Network(format!("the server answered {status}")));
                }
                let v: Value = serde_json::from_str(&body).map_err(|_| Error::Network("that address did not answer like a Subsonic server (is it the server's address?)".into()))?;
                unwrap_response(&v).cloned()
            }
            Err(e) => {
                self.record_server(purpose, &url, None, 0, Some(e.to_string()));
                Err(e)
            }
        }
    }

    /// Opens an address on the server for reading, optionally only the bytes `start..=end`.
    /// `log` adds a line to the request log; the many small reads while a song plays pass false.
    pub fn server_open(&self, url: &str, range: Option<(u64, u64)>, purpose: &str, log: bool) -> Result<ServerBody> {
        if !self.enabled() {
            return Err(Error::Offline);
        }
        let result = (|| -> Result<ServerBody> {
            let mut current = url.to_string();
            let mut redirects = 0;
            loop {
                self.check_server_url(&current)?;
                let mut req = self.stream_agent.get(&current).header("Accept", "audio/*, */*");
                if let Some((a, b)) = range {
                    req = req.header("Range", &format!("bytes={a}-{b}"));
                }
                let resp = req.call().map_err(|e| Error::Network(e.to_string()))?;
                let status = resp.status().as_u16();
                if matches!(status, 301 | 302 | 303 | 307 | 308) && redirects < 3 {
                    let loc = resp.headers().get("location").and_then(|v| v.to_str().ok()).map(str::to_string).ok_or_else(|| Error::Network("redirect without a target".into()))?;
                    current = super::absolute_location(&current, &loc);
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
            Ok(b) if log => self.record_server(purpose, url, Some(b.status), 0, None),
            Err(e) if log => self.record_server(purpose, url, None, 0, Some(e.to_string())),
            _ => {}
        }
        result
    }

    /// Reads a whole (small) reply, such as a cover picture.
    pub fn server_bytes(&self, endpoint: &str, params: &[(&str, &str)], purpose: &str, max: u64) -> Result<Vec<u8>> {
        let url = self.server_url(endpoint, params)?;
        let mut body = self.server_open(&url, None, purpose, true)?;
        if body.status != 200 {
            return Err(Error::Network(format!("the server answered {}", body.status)));
        }
        if body.content_type.contains("json") || body.content_type.contains("xml") {
            return Err(Error::Network("the server has no picture for that".into()));
        }
        let mut out = Vec::new();
        body.reader.by_ref().take(max).read_to_end(&mut out).map_err(|e| Error::Network(e.to_string()))?;
        Ok(out)
    }

    // --- The questions worth asking a Subsonic server ---

    /// Tests the address and login.
    pub fn server_ping(&self) -> Result<ServerStatus> {
        let r = self.server_json("ping", &[], "Server: test connection")?;
        Ok(ServerStatus {
            server_name: opt_text(&r, "type").unwrap_or_else(|| "Subsonic".into()),
            server_version: text(&r, "serverVersion"),
            api_version: text(&r, "version"),
            encrypted: self.server_is_encrypted(),
        })
    }

    /// A page of albums: `kind` is one of [`ALBUM_LISTS`].
    pub fn server_albums(&self, kind: &str, offset: usize, size: usize) -> Result<Vec<ServerAlbum>> {
        let kind = if ALBUM_LISTS.contains(&kind) { kind } else { "newest" };
        let r = self.server_json("getAlbumList2", &[("type", kind), ("offset", &offset.to_string()), ("size", &size.clamp(1, 500).to_string())], "Server: albums")?;
        Ok(list(r.get("albumList2").unwrap_or(&Value::Null), "album").iter().filter_map(album_from_json).collect())
    }

    /// Every artist, in the server's order.
    pub fn server_artists(&self) -> Result<Vec<ServerArtist>> {
        let r = self.server_json("getArtists", &[], "Server: artists")?;
        let mut out = Vec::new();
        for index in list(r.get("artists").unwrap_or(&Value::Null), "index") {
            out.extend(list(index, "artist").iter().filter_map(artist_from_json));
        }
        Ok(out)
    }

    pub fn server_artist_albums(&self, id: &str) -> Result<(ServerArtist, Vec<ServerAlbum>)> {
        let r = self.server_json("getArtist", &[("id", id)], "Server: artist")?;
        let a = r.get("artist").unwrap_or(&Value::Null);
        let artist = artist_from_json(a).ok_or_else(|| Error::Network("the server has no such artist".into()))?;
        Ok((artist, list(a, "album").iter().filter_map(album_from_json).collect()))
    }

    pub fn server_album(&self, id: &str) -> Result<(ServerAlbum, Vec<ServerSong>)> {
        let r = self.server_json("getAlbum", &[("id", id)], "Server: album")?;
        let a = r.get("album").unwrap_or(&Value::Null);
        let album = album_from_json(a).ok_or_else(|| Error::Network("the server has no such album".into()))?;
        Ok((album, list(a, "song").iter().filter_map(song_from_json).collect()))
    }

    pub fn server_song(&self, id: &str) -> Result<ServerSong> {
        let r = self.server_json("getSong", &[("id", id)], "Server: song")?;
        r.get("song").and_then(song_from_json).ok_or_else(|| Error::Network("the server has no such song".into()))
    }

    pub fn server_playlists(&self) -> Result<Vec<ServerPlaylist>> {
        let r = self.server_json("getPlaylists", &[], "Server: playlists")?;
        Ok(list(r.get("playlists").unwrap_or(&Value::Null), "playlist").iter().filter_map(playlist_from_json).collect())
    }

    pub fn server_playlist(&self, id: &str) -> Result<(ServerPlaylist, Vec<ServerSong>)> {
        let r = self.server_json("getPlaylist", &[("id", id)], "Server: playlist")?;
        let p = r.get("playlist").unwrap_or(&Value::Null);
        let pl = playlist_from_json(p).ok_or_else(|| Error::Network("the server has no such playlist".into()))?;
        Ok((pl, list(p, "entry").iter().filter_map(song_from_json).collect()))
    }

    pub fn server_search(&self, query: &str) -> Result<ServerSearch> {
        let r = self.server_json("search3", &[("query", query), ("artistCount", "10"), ("albumCount", "30"), ("songCount", "60")], "Server: search")?;
        let s = r.get("searchResult3").unwrap_or(&Value::Null);
        Ok(ServerSearch {
            artists: list(s, "artist").iter().filter_map(artist_from_json).collect(),
            albums: list(s, "album").iter().filter_map(album_from_json).collect(),
            songs: list(s, "song").iter().filter_map(song_from_json).collect(),
        })
    }

    /// The cover picture for a cover id, scaled by the server to about `size` pixels.
    pub fn server_cover(&self, cover_id: &str, size: u32) -> Result<Vec<u8>> {
        self.server_bytes("getCoverArt", &[("id", cover_id), ("size", &size.to_string())], "Server: cover", 8 * 1024 * 1024)
    }

    /// The address that plays the original file of a song.
    pub fn server_stream_url(&self, id: &str) -> Result<String> {
        self.server_url("stream", &[("id", id), ("format", "raw")])
    }

    /// How long a block of a song may take before playback gives up.
    pub fn server_read_timeout() -> Duration {
        Duration::from_secs(12)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_flac_song_is_lossless_and_an_mp3_is_not() {
        let flac = song_from_json(&json!({"id": "1", "title": "A", "suffix": "flac", "duration": 200, "bitRate": 900, "samplingRate": 44100, "bitDepth": 16, "track": 3, "coverArt": "al-1"})).unwrap();
        assert!(flac.lossless);
        assert_eq!((flac.track, flac.bit_depth, flac.sample_rate, flac.cover_art.as_deref()), (Some(3), Some(16), Some(44100), Some("al-1")));
        let mp3 = song_from_json(&json!({"id": "2", "title": "B", "suffix": "MP3", "bitRate": 320})).unwrap();
        assert!(!mp3.lossless);
        assert_eq!(mp3.suffix, "mp3");
    }

    #[test]
    fn m4a_is_lossless_only_when_it_carries_a_bit_depth() {
        let alac = song_from_json(&json!({"id": "1", "suffix": "m4a", "bitDepth": 24})).unwrap();
        let aac = song_from_json(&json!({"id": "2", "suffix": "m4a", "bitRate": 256})).unwrap();
        assert!(alac.lossless);
        assert!(!aac.lossless);
    }

    #[test]
    fn ids_may_be_text_or_numbers_and_folders_are_not_songs() {
        assert_eq!(song_from_json(&json!({"id": 42, "title": "N"})).unwrap().id, "42");
        assert!(song_from_json(&json!({"id": "d1", "isDir": true})).is_none());
        assert!(song_from_json(&json!({"title": "no id"})).is_none());
    }

    #[test]
    fn genres_come_from_either_field() {
        assert_eq!(album_from_json(&json!({"id": "a", "name": "X", "genre": "Jazz"})).unwrap().genre.as_deref(), Some("Jazz"));
        assert_eq!(album_from_json(&json!({"id": "a", "name": "X", "genres": [{"name": "Ambient"}]})).unwrap().genre.as_deref(), Some("Ambient"));
    }

    #[test]
    fn a_failed_response_says_what_went_wrong() {
        let bad = json!({"subsonic-response": {"status": "failed", "error": {"code": 40, "message": "Wrong username or password"}}});
        assert!(unwrap_response(&bad).unwrap_err().to_string().contains("not accepted"));
        assert!(unwrap_response(&json!({"hello": 1})).unwrap_err().to_string().contains("Subsonic server"));
        assert!(unwrap_response(&json!({"subsonic-response": {"status": "ok"}})).is_ok());
    }

    #[test]
    fn the_token_is_md5_of_password_and_salt() {
        // The example from the Subsonic API documentation: password "sesame", salt "c19b2d".
        assert_eq!(md5_hex("sesamec19b2d"), "26719a1196d2a940705a59634eb18eab");
    }

    #[test]
    fn urls_are_signed_without_the_password_and_only_for_the_set_server() {
        let n = Net::new(std::env::temp_dir().join("redshift-subsonic-test"), "t", &[]);
        assert!(n.server_url("ping", &[]).is_err());
        n.set_server(Some(&ServerConfig { url: "music.example.com:4533/".into(), user: "me@x".into(), password: "hunter2".into() }));
        let url = n.server_url("getAlbum", &[("id", "a b")]).unwrap();
        assert!(url.starts_with("https://music.example.com:4533/rest/getAlbum?u=me%40x&t="), "{url}");
        assert!(url.contains("&id=a%20b") && !url.contains("hunter2"));
        assert!(n.check_server_url(&url).is_ok());
        assert!(n.check_server_url("https://evil.example.com/rest/ping").is_err());
        assert!(n.check_server_url("http://music.example.com:4533/rest/ping").is_err(), "a different scheme is not the same server");
        n.set_server(None);
        assert!(!n.server_is_set());
    }

    fn live(tag: &str) -> (super::super::fakesub::Fake, Net) {
        use super::super::fakesub;
        let f = fakesub::start(vec![1, 2, 3], true);
        let n = Net::new(std::env::temp_dir().join(format!("redshift-subsonic-{tag}")), "t", &[]);
        n.set_enabled(true);
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: fakesub::USER.into(), password: fakesub::PASSWORD.into() }));
        (f, n)
    }

    #[test]
    fn it_logs_in_and_lists_the_library() {
        let (_f, n) = live("lists");
        let st = n.server_ping().unwrap();
        assert_eq!((st.server_name.as_str(), st.encrypted), ("fakesonic", false));
        let albums = n.server_albums("newest", 0, 20).unwrap();
        assert_eq!((albums.len(), albums[0].name.as_str(), albums[0].song_count), (1, "First Light", 2));
        assert_eq!(n.server_artists().unwrap()[0].name, "Ada");
        let (artist, its) = n.server_artist_albums("ar1").unwrap();
        assert_eq!((artist.name.as_str(), its.len()), ("Ada", 1));
        let (album, songs) = n.server_album("al1").unwrap();
        assert_eq!(album.id, "al1");
        assert_eq!(songs.iter().map(|s| (s.title.as_str(), s.lossless)).collect::<Vec<_>>(), vec![("One", true), ("Two", false)]);
        assert_eq!(n.server_song("s2").unwrap().bitrate_kbps, Some(320));
        let (pl, entries) = n.server_playlist("p1").unwrap();
        assert_eq!((pl.name.as_str(), entries.len()), ("Evening", 2));
        assert_eq!(n.server_playlists().unwrap().len(), 1);
        let found = n.server_search("ada").unwrap();
        assert_eq!((found.artists.len(), found.albums.len(), found.songs.len()), (1, 1, 1));
        assert!(n.server_cover("al-al1", 300).unwrap().starts_with(b"\x89PNG"));
    }

    #[test]
    fn a_missing_item_and_a_wrong_login_are_said_in_words() {
        let (f, n) = live("errors");
        assert!(n.server_album("nope").unwrap_err().to_string().contains("could not find"));
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: "me".into(), password: "wrong".into() }));
        assert!(n.server_ping().unwrap_err().to_string().contains("not accepted"));
    }

    #[test]
    fn it_says_so_when_the_address_is_not_a_subsonic_server() {
        let f = super::super::testserver::start(vec![super::super::testserver::reply(200, "<html>hello</html>")]);
        let n = Net::new(std::env::temp_dir().join("redshift-subsonic-html"), "t", &[]);
        n.set_enabled(true);
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: "u".into(), password: "p".into() }));
        assert!(n.server_ping().unwrap_err().to_string().contains("Subsonic server"));
    }

    #[test]
    fn nothing_is_asked_while_online_features_are_off_and_the_request_log_has_no_token() {
        let (f, n) = live("offline");
        n.set_enabled(false);
        assert!(matches!(n.server_ping(), Err(Error::Offline)));
        assert_eq!(f.hits.load(std::sync::atomic::Ordering::Relaxed), 0);
        n.set_enabled(true);
        n.server_ping().unwrap();
        let log = n.log();
        assert_eq!(log.len(), 1);
        assert!(log[0].url.ends_with("/rest/ping") && log[0].purpose.starts_with("Server"), "{:?}", log[0]);
    }

    #[test]
    fn the_log_never_shows_the_login_token() {
        let n = Net::new(std::env::temp_dir().join("redshift-subsonic-test2"), "t", &[]);
        n.set_server(Some(&ServerConfig { url: "https://m.example.com".into(), user: "u".into(), password: "p".into() }));
        let url = n.server_url("ping", &[]).unwrap();
        n.record_server("Server: test", &url, Some(200), 10, None);
        let log = n.log();
        assert_eq!(log[0].url, "https://m.example.com/rest/ping");
        assert_eq!(log[0].host, "m.example.com");
    }
}
