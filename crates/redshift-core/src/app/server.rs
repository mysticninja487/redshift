//! The person's own Navidrome / Subsonic server as a source: its settings, browsing, search, and
//! what the rest of the app needs to treat its songs like any other song (tags, covers).
//!
//! A song on the server rides through the queue, playlists, history and stats as the path
//! `subsonic://<id>` (see [`crate::util::server_song_path`]).

use std::collections::{HashMap, VecDeque};

use serde::Serialize;

use super::*;
use crate::net::subsonic::{ServerAlbum, ServerArtist, ServerPlaylist, ServerSearch, ServerSong, ServerStatus};
use crate::net::ServerConfig;
use crate::util::{server_song_id, server_song_path};

pub(super) const SERVER_FILE: &str = "server.json";
const SONGS_FILE: &str = "server-songs.json";
/// How many songs' details are remembered, so a queue restored at launch can be shown before the
/// server is asked.
const SONG_CACHE_MAX: usize = 4000;
const COVER_CACHE_BYTES: usize = 24 * 1024 * 1024;

#[derive(Default)]
pub(super) struct ServerState {
    config: ServerConfig,
    songs: HashMap<String, ServerSong>,
    order: VecDeque<String>,
    dirty: bool,
    /// Covers by cover id and size; `None` remembers "the server has none".
    covers: VecDeque<(String, u32, Option<Arc<Cover>>)>,
}

impl ServerState {
    pub(super) fn load(data_dir: &Path) -> (Self, ServerConfig) {
        let config: ServerConfig = fs::read(data_dir.join(SERVER_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let list: Vec<ServerSong> = fs::read(data_dir.join(SONGS_FILE)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
        let mut st = ServerState { config: config.clone(), ..ServerState::default() };
        for s in list {
            st.insert(s);
        }
        st.dirty = false;
        (st, config)
    }

    fn insert(&mut self, s: ServerSong) {
        if self.songs.insert(s.id.clone(), s.clone()).is_none() {
            self.order.push_back(s.id);
            while self.order.len() > SONG_CACHE_MAX {
                if let Some(old) = self.order.pop_front() {
                    self.songs.remove(&old);
                }
            }
        }
        self.dirty = true;
    }
}

/// What the settings screen shows: everything but the password.
#[derive(Debug, Clone, Serialize)]
pub struct ServerSettings {
    pub url: String,
    pub user: String,
    pub has_password: bool,
    /// An address and a user name have been entered.
    pub configured: bool,
    /// The address is https. Without it the login token and the music cross the network unencrypted.
    pub encrypted: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerAlbumView {
    pub album: ServerAlbum,
    pub songs: Vec<ServerSong>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerArtistView {
    pub artist: ServerArtist,
    pub albums: Vec<ServerAlbum>,
}

#[derive(Debug, Clone, Serialize)]
pub struct ServerPlaylistView {
    pub playlist: ServerPlaylist,
    pub songs: Vec<ServerSong>,
}

fn sniff_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG") {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.len() > 12 && &data[..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

impl App {
    pub fn server_settings(&self) -> ServerSettings {
        let st = self.server.lock();
        ServerSettings {
            url: st.config.url.clone(),
            user: st.config.user.clone(),
            has_password: !st.config.password.is_empty(),
            configured: st.config.is_set(),
            encrypted: st.config.is_set() && st.config.base_url().to_ascii_lowercase().starts_with("https://"),
        }
    }

    /// Saves the server's address and login. `password` of `None` keeps the one already saved.
    /// Empty address and user name forget the server.
    pub fn set_server_settings(&self, url: &str, user: &str, password: Option<&str>) -> Result<()> {
        let mut st = self.server.lock();
        let password = password.map_or_else(|| st.config.password.clone(), str::to_string);
        let new = ServerConfig { url: url.trim().to_string(), user: user.trim().to_string(), password };
        let path = self.data_dir.join(SERVER_FILE);
        if new.url.is_empty() && new.user.is_empty() {
            let _ = fs::remove_file(&path);
        } else {
            write_json_atomic(&path, &new)?;
        }
        let other_server = !new.is_set() || !st.config.is_set() || new.base_url() != st.config.base_url() || new.user != st.config.user;
        if other_server {
            // The ids of one server mean nothing on another.
            st.songs.clear();
            st.order.clear();
            st.covers.clear();
            st.dirty = true;
        }
        st.config = new.clone();
        drop(st);
        self.net.set_server(Some(&new));
        Ok(())
    }

    fn server_gate(&self) -> Result<()> {
        let o = self.online();
        if !o.enabled || !o.server {
            return Err(Error::Offline);
        }
        if !self.net.server_is_set() {
            return Err(Error::Other("no server has been set up yet (Settings > Online)".into()));
        }
        Ok(())
    }

    fn remember_songs(&self, songs: &[ServerSong]) {
        let mut st = self.server.lock();
        for s in songs {
            st.insert(s.clone());
        }
    }

    /// Checks the address and login.
    pub fn server_test(&self) -> Result<ServerStatus> {
        self.server_gate()?;
        self.net.server_ping()
    }

    pub fn server_albums(&self, kind: &str, offset: usize, size: usize) -> Result<Vec<ServerAlbum>> {
        self.server_gate()?;
        self.net.server_albums(kind, offset, size)
    }

    pub fn server_artists(&self) -> Result<Vec<ServerArtist>> {
        self.server_gate()?;
        self.net.server_artists()
    }

    pub fn server_artist(&self, id: &str) -> Result<ServerArtistView> {
        self.server_gate()?;
        let (artist, albums) = self.net.server_artist_albums(id)?;
        Ok(ServerArtistView { artist, albums })
    }

    pub fn server_album(&self, id: &str) -> Result<ServerAlbumView> {
        self.server_gate()?;
        let (album, songs) = self.net.server_album(id)?;
        self.remember_songs(&songs);
        Ok(ServerAlbumView { album, songs })
    }

    pub fn server_playlists(&self) -> Result<Vec<ServerPlaylist>> {
        self.server_gate()?;
        self.net.server_playlists()
    }

    pub fn server_playlist(&self, id: &str) -> Result<ServerPlaylistView> {
        self.server_gate()?;
        let (playlist, songs) = self.net.server_playlist(id)?;
        self.remember_songs(&songs);
        Ok(ServerPlaylistView { playlist, songs })
    }

    pub fn server_search(&self, query: &str) -> Result<ServerSearch> {
        self.server_gate()?;
        let query = query.trim();
        if query.is_empty() {
            return Ok(ServerSearch::default());
        }
        let found = self.net.server_search(query)?;
        self.remember_songs(&found.songs);
        Ok(found)
    }

    /// Plays songs from the server (replacing the queue), starting at `start`.
    pub fn server_play(&self, ids: &[String], start: usize) -> Result<()> {
        self.server_gate()?;
        if ids.is_empty() {
            return Ok(());
        }
        let paths: Vec<PathBuf> = ids.iter().map(|id| server_song_path(id)).collect();
        self.play_queue(paths, start.min(ids.len() - 1), true);
        Ok(())
    }

    /// The details of a song on the server, from what was seen before or else from the server.
    pub(super) fn server_song_cached(&self, id: &str) -> Option<ServerSong> {
        self.server.lock().songs.get(id).cloned()
    }

    /// Tags for a song on the server. Never fails: a song the server cannot be asked about is
    /// shown as just "a song from your server".
    pub(super) fn server_meta(&self, path: &Path) -> TrackMeta {
        let id = server_song_id(path).unwrap_or_default();
        let song = self.server_song_cached(&id).or_else(|| {
            let fetched = self.server_gate().ok().and_then(|()| self.net.server_song(&id).ok())?;
            self.remember_songs(std::slice::from_ref(&fetched));
            Some(fetched)
        });
        let Some(s) = song else {
            return TrackMeta { path: path.to_path_buf(), title: "Song from your server".into(), ..TrackMeta::default() };
        };
        let artist = if s.artist.is_empty() { crate::meta::UNKNOWN_ARTIST.to_string() } else { s.artist.clone() };
        TrackMeta {
            path: path.to_path_buf(),
            title: s.title.clone(),
            album: if s.album.is_empty() { crate::meta::UNKNOWN_ALBUM.to_string() } else { s.album.clone() },
            album_artist: artist.clone(),
            artist,
            genre: s.genre.clone().unwrap_or_default(),
            year: s.year.and_then(|y| u16::try_from(y).ok()),
            track_no: s.track,
            disc_no: s.disc,
            duration_secs: s.duration_secs,
            sample_rate: s.sample_rate,
            bit_depth: s.bit_depth.and_then(|b| u8::try_from(b).ok()),
            channels: s.channels.and_then(|c| u8::try_from(c).ok()),
            bitrate_kbps: s.bitrate_kbps,
            format: s.suffix.clone(),
            size: s.size,
            ..TrackMeta::default()
        }
    }

    /// The cover of a song on the server, scaled by the server to about `px` pixels.
    pub(super) fn server_cover_of(&self, path: &Path, px: u32) -> Option<Arc<Cover>> {
        if let Some(cover_id) = crate::util::server_cover_id(path) {
            return self.server_cover_by_id(&cover_id, px);
        }
        let id = server_song_id(path)?;
        let song = self.server_song_cached(&id).or_else(|| {
            let fetched = self.server_gate().ok().and_then(|()| self.net.server_song(&id).ok())?;
            self.remember_songs(std::slice::from_ref(&fetched));
            Some(fetched)
        })?;
        let cover_id = song.cover_art?;
        self.server_cover_by_id(&cover_id, px)
    }

    /// A cover by the server's cover id (albums have one too).
    pub fn server_cover_by_id(&self, cover_id: &str, px: u32) -> Option<Arc<Cover>> {
        if let Some((_, _, hit)) = self.server.lock().covers.iter().find(|(c, p, _)| c == cover_id && *p == px) {
            return hit.clone();
        }
        self.server_gate().ok()?;
        // Fetched outside the lock: it is a network request.
        let found = self.net.server_cover(cover_id, px).ok().filter(|b| !b.is_empty()).map(|data| Arc::new(Cover { mime: sniff_mime(&data).to_string(), data }));
        let mut st = self.server.lock();
        st.covers.push_back((cover_id.to_string(), px, found.clone()));
        let mut bytes: usize = st.covers.iter().map(|(_, _, c)| c.as_ref().map_or(0, |c| c.data.len())).sum();
        while bytes > COVER_CACHE_BYTES && st.covers.len() > 1 {
            if let Some((_, _, old)) = st.covers.pop_front() {
                bytes -= old.map_or(0, |c| c.data.len());
            }
        }
        found
    }

    /// Writes the remembered song details if they changed.
    pub(super) fn save_server_songs(&self) -> Result<()> {
        let snapshot = {
            let mut st = self.server.lock();
            if !st.dirty {
                return Ok(());
            }
            st.dirty = false;
            st.order.iter().filter_map(|id| st.songs.get(id).cloned()).collect::<Vec<_>>()
        };
        if let Err(e) = write_json_atomic(&self.data_dir.join(SONGS_FILE), &snapshot) {
            self.server.lock().dirty = true;
            return Err(e);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::OfflineConfig;
    use crate::net::fakesub;

    fn open(dir: &Path) -> App {
        let cfg = PlayerConfig { device: None, offline: Some(OfflineConfig { sample_rate: 48_000, sink: Arc::new(Mutex::new(Vec::new())), speed: 4.0 }), ..Default::default() };
        App::new(dir, cfg).unwrap().0
    }

    fn online(app: &App, server: bool) {
        app.set_online(OnlineSettings { enabled: true, server, ..OnlineSettings::default() });
    }

    #[test]
    fn the_server_needs_the_online_switches_and_a_setup() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        assert!(matches!(app.server_albums("newest", 0, 10), Err(Error::Offline)), "main switch off");
        online(&app, false);
        assert!(matches!(app.server_albums("newest", 0, 10), Err(Error::Offline)), "server switch off");
        online(&app, true);
        assert!(app.server_albums("newest", 0, 10).unwrap_err().to_string().contains("no server"), "nothing set up");
    }

    #[test]
    fn it_browses_remembers_songs_and_shows_them_as_normal_tracks() {
        let f = fakesub::start(vec![0; 10], true);
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        online(&app, true);
        app.set_server_settings(&f.base, "me", Some("secret")).unwrap();
        assert_eq!(app.server_test().unwrap().server_name, "fakesonic");
        let album = app.server_albums("newest", 0, 10).unwrap();
        let view = app.server_album(&album[0].id).unwrap();
        assert_eq!(view.songs.len(), 2);
        let m = app.track_meta(&server_song_path("s1")).unwrap();
        assert_eq!((m.title.as_str(), m.artist.as_str(), m.album.as_str(), m.format.as_str(), m.bit_depth, m.sample_rate), ("One", "Ada", "First Light", "flac", Some(16), Some(44100)));
        assert_eq!(app.paths_exist(&[server_song_path("s1"), PathBuf::from("/no/such.flac")]), vec![true, false]);
        let cover = app.cover_sized(&server_song_path("s1"), 300).expect("a cover comes from the server");
        assert_eq!(cover.mime, "image/png");
        // Asked again, it comes from memory.
        let before = f.hits.load(Ordering::Relaxed);
        assert!(app.cover_sized(&server_song_path("s1"), 300).is_some());
        assert_eq!(f.hits.load(Ordering::Relaxed), before);
    }

    #[test]
    fn a_song_not_seen_yet_is_looked_up_and_remembered_across_restarts() {
        let f = fakesub::start(vec![0; 10], true);
        let d = tempfile::tempdir().unwrap();
        {
            let app = open(d.path());
            online(&app, true);
            app.set_server_settings(&f.base, "me", Some("secret")).unwrap();
            assert_eq!(app.track_meta(&server_song_path("s2")).unwrap().title, "Two");
            app.flush().unwrap();
        }
        // Offline after a restart: what was seen is still shown, what was not is named plainly.
        let app = open(d.path());
        assert_eq!(app.track_meta(&server_song_path("s2")).unwrap().title, "Two");
        assert_eq!(app.track_meta(&server_song_path("zzz")).unwrap().title, "Song from your server");
        assert!(app.server_settings().configured && app.server_settings().has_password);
    }

    #[test]
    fn the_password_is_kept_when_only_the_address_is_edited_and_the_forgotten_server_leaves_nothing() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        app.set_server_settings("http://nas.local:4533", "me", Some("secret")).unwrap();
        app.set_server_settings("http://nas.local:4533/", "me2", None).unwrap();
        let s = app.server_settings();
        assert_eq!((s.user.as_str(), s.has_password, s.encrypted), ("me2", true, false));
        assert!(d.path().join(SERVER_FILE).exists());
        app.set_server_settings("", "", None).unwrap();
        assert!(!app.server_settings().configured);
        assert!(!d.path().join(SERVER_FILE).exists());
    }

    #[test]
    fn server_songs_stay_in_the_stats_and_come_back_with_the_queue() {
        let d = tempfile::tempdir().unwrap();
        let app = open(d.path());
        let p = server_song_path("s1");
        app.with_stats(|s| s.record_play(&p));
        assert_eq!(app.play_stats().len(), 1);
        app.player.restore_queue(vec![p.clone()], 0, 0.0);
        assert!(!app.paths_exist(&[p]).contains(&false));
    }
}
