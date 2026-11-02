//! User playlists: an ordered list of file paths with a name, stored as one JSON file, plus M3U/M3U8
//! import and export.
//!
//! Playlists hold paths rather than library entries, so they survive a library rescan and can
//! point at files outside the library. A track may appear in a playlist more than once.

use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};
use crate::library::write_json_atomic;
use crate::lyrics;
use crate::util::percent_decode;

const DB_VERSION: u32 = 1;

pub type PlaylistId = u64;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Playlist {
    pub id: PlaylistId,
    pub name: String,
    pub tracks: Vec<PathBuf>,
    /// Seconds since the epoch.
    pub created: u64,
    pub modified: u64,
}

/// A playlist without its tracks, for list views.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct PlaylistSummary {
    pub id: PlaylistId,
    pub name: String,
    pub track_count: usize,
    pub modified: u64,
}

#[derive(Serialize, Deserialize)]
struct Db {
    version: u32,
    next_id: PlaylistId,
    playlists: Vec<Playlist>,
}

pub struct PlaylistStore {
    path: PathBuf,
    next_id: PlaylistId,
    playlists: Vec<Playlist>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

fn no_such(id: PlaylistId) -> Error {
    Error::Other(format!("no playlist with id {id}"))
}

impl PlaylistStore {
    /// Loads playlists from `path`, or starts empty if there's no file. A file that can't be parsed
    /// is set aside as `.bad` rather than overwritten.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut store = Self { path, next_id: 1, playlists: Vec::new() };
        match fs::read(&store.path) {
            Ok(bytes) => match serde_json::from_slice::<Db>(&bytes) {
                Ok(db) if db.version <= DB_VERSION => {
                    // Guard against a hand-edited file reusing ids.
                    store.next_id = db.next_id.max(db.playlists.iter().map(|p| p.id + 1).max().unwrap_or(1));
                    store.playlists = db.playlists;
                }
                _ => {
                    let mut bad = store.path.clone().into_os_string();
                    bad.push(".bad");
                    let _ = fs::rename(&store.path, PathBuf::from(bad));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(store)
    }

    pub fn save(&self) -> Result<()> {
        let db = Db { version: DB_VERSION, next_id: self.next_id, playlists: self.playlists.clone() };
        write_json_atomic(&self.path, &db)
    }

    pub fn list(&self) -> Vec<PlaylistSummary> {
        self.playlists
            .iter()
            .map(|p| PlaylistSummary { id: p.id, name: p.name.clone(), track_count: p.tracks.len(), modified: p.modified })
            .collect()
    }

    pub fn get(&self, id: PlaylistId) -> Option<&Playlist> {
        self.playlists.iter().find(|p| p.id == id)
    }

    fn get_mut(&mut self, id: PlaylistId) -> Result<&mut Playlist> {
        self.playlists.iter_mut().find(|p| p.id == id).ok_or_else(|| no_such(id))
    }

    /// Creates an empty playlist. Names needn't be unique; a blank name becomes "Untitled playlist".
    pub fn create(&mut self, name: &str) -> PlaylistId {
        self.create_with(name, Vec::new())
    }

    pub fn create_with(&mut self, name: &str, tracks: Vec<PathBuf>) -> PlaylistId {
        let id = self.next_id;
        self.next_id += 1;
        let name = name.trim();
        let t = now();
        self.playlists.push(Playlist {
            id,
            name: if name.is_empty() { "Untitled playlist".into() } else { name.into() },
            tracks,
            created: t,
            modified: t,
        });
        id
    }

    pub fn rename(&mut self, id: PlaylistId, name: &str) -> Result<()> {
        let name = name.trim();
        if name.is_empty() {
            return Err(Error::Other("playlist name can't be empty".into()));
        }
        let p = self.get_mut(id)?;
        p.name = name.into();
        p.modified = now();
        Ok(())
    }

    pub fn delete(&mut self, id: PlaylistId) -> Result<()> {
        let before = self.playlists.len();
        self.playlists.retain(|p| p.id != id);
        if self.playlists.len() == before {
            return Err(no_such(id));
        }
        Ok(())
    }

    /// Moves a playlist within the list of playlists (for manual ordering in the sidebar).
    pub fn move_playlist(&mut self, id: PlaylistId, to: usize) -> Result<()> {
        let from = self.playlists.iter().position(|p| p.id == id).ok_or_else(|| no_such(id))?;
        let p = self.playlists.remove(from);
        self.playlists.insert(to.min(self.playlists.len()), p);
        Ok(())
    }

    /// Inserts `tracks` at `position` (or at the end if `None` or past the end).
    pub fn add_tracks(&mut self, id: PlaylistId, tracks: &[PathBuf], position: Option<usize>) -> Result<()> {
        let p = self.get_mut(id)?;
        let at = position.unwrap_or(p.tracks.len()).min(p.tracks.len());
        p.tracks.splice(at..at, tracks.iter().cloned());
        p.modified = now();
        Ok(())
    }

    /// Removes the entries at the given positions (duplicates and out-of-range positions are
    /// ignored). Returns how many were removed.
    pub fn remove_at(&mut self, id: PlaylistId, positions: &[usize]) -> Result<usize> {
        let p = self.get_mut(id)?;
        let mut drop = vec![false; p.tracks.len()];
        for &i in positions {
            if let Some(d) = drop.get_mut(i) {
                *d = true;
            }
        }
        let before = p.tracks.len();
        let mut flags = drop.into_iter();
        p.tracks.retain(|_| !flags.next().unwrap_or(false));
        p.modified = now();
        Ok(before - p.tracks.len())
    }

    /// Moves the entry at `from` so that it ends up at index `to`.
    pub fn move_track(&mut self, id: PlaylistId, from: usize, to: usize) -> Result<()> {
        let p = self.get_mut(id)?;
        if from >= p.tracks.len() || to >= p.tracks.len() {
            return Err(Error::Other(format!("position out of range (playlist has {} tracks)", p.tracks.len())));
        }
        let t = p.tracks.remove(from);
        p.tracks.insert(to, t);
        p.modified = now();
        Ok(())
    }

    pub fn set_tracks(&mut self, id: PlaylistId, tracks: Vec<PathBuf>) -> Result<()> {
        let p = self.get_mut(id)?;
        p.tracks = tracks;
        p.modified = now();
        Ok(())
    }

    /// Reads an M3U/M3U8 file into a new playlist named after the file. Relative entries are
    /// resolved against the file's folder. Returns the new id and the number of entries found.
    pub fn import_m3u(&mut self, file: &Path) -> Result<(PlaylistId, usize)> {
        let text = decode_m3u(&fs::read(file)?);
        let base = file.parent().unwrap_or(Path::new("."));
        let tracks = parse_m3u(&text, base);
        let name = file.file_stem().and_then(|s| s.to_str()).unwrap_or("Imported playlist");
        let n = tracks.len();
        Ok((self.create_with(name, tracks), n))
    }

    /// Writes a playlist as UTF-8 M3U8. Tracks inside the destination folder are written as
    /// relative paths so the playlist and its music can move together; others stay absolute.
    /// `info` supplies `(duration_secs, "Artist - Title")` for the `#EXTINF` lines where known.
    pub fn export_m3u(&self, id: PlaylistId, dest: &Path, info: &dyn Fn(&Path) -> Option<(f64, String)>) -> Result<()> {
        let p = self.get(id).ok_or_else(|| no_such(id))?;
        let base = dest.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
        fs::write(dest, m3u_text(&p.tracks, base, info))?;
        Ok(())
    }
}

/// Text of an M3U file from raw bytes. BOM-marked UTF-8/UTF-16 are honoured; otherwise UTF-8 if it
/// is valid, else Latin-1, which is what plain `.m3u` files from older Windows software use.
pub fn decode_m3u(bytes: &[u8]) -> String {
    if bytes.starts_with(&[0xFF, 0xFE]) || bytes.starts_with(&[0xFE, 0xFF]) || bytes.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return lyrics::decode_text(bytes);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.to_owned(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

/// Track paths from M3U text. Comments and `#EXTINF` lines are skipped, blank lines ignored,
/// `file://` URLs decoded, other URLs (streams) dropped, backslashes treated as separators, and
/// relative paths resolved against `base`.
pub fn parse_m3u(text: &str, base: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for line in text.trim_start_matches('\u{feff}').lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let entry = if let Some(rest) = strip_prefix_ci(line, "file://") {
            // file:///C:/Music/a.flac or file:///home/me/a.flac
            let decoded = percent_decode(rest);
            match decoded.strip_prefix('/') {
                Some(r) if has_drive(r) => r.to_owned(),
                _ => decoded,
            }
        } else if line.contains("://") {
            continue; // http(s)/mms streams can't be played from disk
        } else {
            line.to_owned()
        };
        let entry = if cfg!(windows) { entry } else { entry.replace('\\', "/") };
        // A drive letter means "absolute" even when parsing on a system that doesn't know drives.
        let absolute = has_drive(&entry) || has_root(Path::new(&entry));
        let path = PathBuf::from(entry);
        out.push(if absolute { path } else { normalize(&base.join(path)) });
    }
    out
}

fn m3u_text(tracks: &[PathBuf], base: &Path, info: &dyn Fn(&Path) -> Option<(f64, String)>) -> String {
    let mut s = String::from("#EXTM3U\n");
    for t in tracks {
        if let Some((secs, label)) = info(t) {
            // The label must stay on one line.
            let label = label.replace(['\r', '\n'], " ");
            s.push_str(&format!("#EXTINF:{},{}\n", secs.round() as i64, label));
        }
        match t.strip_prefix(base) {
            // Forward slashes work everywhere, including Windows players.
            Ok(rel) => s.push_str(&rel.to_string_lossy().replace('\\', "/")),
            Err(_) => s.push_str(&t.to_string_lossy()),
        }
        s.push('\n');
    }
    s
}

fn has_drive(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() >= 2 && b[0].is_ascii_alphabetic() && b[1] == b':'
}

fn has_root(p: &Path) -> bool {
    p.has_root() || p.components().next().is_some_and(|c| matches!(c, Component::Prefix(_)))
}

fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    s.get(..prefix.len()).filter(|h| h.eq_ignore_ascii_case(prefix)).map(|_| &s[prefix.len()..])
}

/// Collapses `.` and `..` lexically (without touching the file system), so `music/../a.flac`
/// becomes `a.flac` and playlists compare equal to library paths.
fn normalize(p: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::CurDir => {}
            Component::ParentDir => {
                // At a root there's nowhere further up to go, so `..` is dropped.
                if !out.pop() && !out.has_root() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn paths(names: &[&str]) -> Vec<PathBuf> {
        names.iter().map(PathBuf::from).collect()
    }

    fn store() -> (PlaylistStore, tempfile::TempDir) {
        let dir = tempfile::tempdir().unwrap();
        (PlaylistStore::open(dir.path().join("playlists.json")).unwrap(), dir)
    }

    #[test]
    fn create_rename_delete() {
        let (mut s, _d) = store();
        let a = s.create("  Road trip ");
        let b = s.create("   ");
        assert_ne!(a, b);
        assert_eq!(s.get(a).unwrap().name, "Road trip", "name is trimmed");
        assert_eq!(s.get(b).unwrap().name, "Untitled playlist");
        s.rename(a, "Night drive").unwrap();
        assert_eq!(s.get(a).unwrap().name, "Night drive");
        assert!(s.rename(a, "  ").is_err(), "blank rename is rejected");
        assert_eq!(s.get(a).unwrap().name, "Night drive");
        s.delete(a).unwrap();
        assert!(s.get(a).is_none());
        assert!(s.delete(a).is_err());
        assert!(s.rename(999, "x").is_err());
        assert_eq!(s.list().len(), 1);
    }

    #[test]
    fn ids_are_not_reused_after_delete() {
        let (mut s, _d) = store();
        let a = s.create("a");
        s.delete(a).unwrap();
        assert_ne!(s.create("b"), a);
    }

    #[test]
    fn add_tracks_at_positions_and_allows_duplicates() {
        let (mut s, _d) = store();
        let id = s.create("p");
        s.add_tracks(id, &paths(&["a", "b"]), None).unwrap();
        s.add_tracks(id, &paths(&["x", "y"]), Some(1)).unwrap();
        s.add_tracks(id, &paths(&["a"]), Some(100)).unwrap();
        s.add_tracks(id, &paths(&["first"]), Some(0)).unwrap();
        assert_eq!(s.get(id).unwrap().tracks, paths(&["first", "a", "x", "y", "b", "a"]));
        assert!(s.add_tracks(42, &paths(&["a"]), None).is_err());
    }

    #[test]
    fn remove_at_ignores_duplicates_and_bad_positions() {
        let (mut s, _d) = store();
        let id = s.create("p");
        s.add_tracks(id, &paths(&["a", "b", "c", "d"]), None).unwrap();
        assert_eq!(s.remove_at(id, &[1, 1, 3, 17]).unwrap(), 2);
        assert_eq!(s.get(id).unwrap().tracks, paths(&["a", "c"]));
        assert_eq!(s.remove_at(id, &[]).unwrap(), 0);
    }

    #[test]
    fn move_track_lands_at_target_index() {
        let (mut s, _d) = store();
        let id = s.create("p");
        s.add_tracks(id, &paths(&["a", "b", "c", "d"]), None).unwrap();
        s.move_track(id, 0, 2).unwrap();
        assert_eq!(s.get(id).unwrap().tracks, paths(&["b", "c", "a", "d"]));
        s.move_track(id, 3, 0).unwrap();
        assert_eq!(s.get(id).unwrap().tracks, paths(&["d", "b", "c", "a"]));
        s.move_track(id, 1, 1).unwrap();
        assert_eq!(s.get(id).unwrap().tracks, paths(&["d", "b", "c", "a"]));
        assert!(s.move_track(id, 4, 0).is_err());
        assert!(s.move_track(id, 0, 4).is_err());
        assert_eq!(s.get(id).unwrap().tracks.len(), 4, "failed moves change nothing");
    }

    #[test]
    fn playlists_can_be_reordered() {
        let (mut s, _d) = store();
        let (a, b, c) = (s.create("a"), s.create("b"), s.create("c"));
        s.move_playlist(c, 0).unwrap();
        let ids: Vec<_> = s.list().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![c, a, b]);
        s.move_playlist(c, 99).unwrap();
        let ids: Vec<_> = s.list().iter().map(|p| p.id).collect();
        assert_eq!(ids, vec![a, b, c]);
        assert!(s.move_playlist(99, 0).is_err());
    }

    #[test]
    fn list_summarises_without_tracks() {
        let (mut s, _d) = store();
        let id = s.create_with("p", paths(&["a", "b", "c"]));
        let l = s.list();
        assert_eq!((l[0].id, l[0].name.as_str(), l[0].track_count), (id, "p", 3));
    }

    #[test]
    fn save_and_reopen_round_trips_including_next_id() {
        let (mut s, d) = store();
        let a = s.create_with("one", paths(&["x/a.flac", "x/b.opus"]));
        let b = s.create("two");
        s.delete(b).unwrap();
        s.save().unwrap();
        let mut r = PlaylistStore::open(d.path().join("playlists.json")).unwrap();
        assert_eq!(r.get(a), s.get(a));
        assert!(r.create("three") > b, "ids keep counting up across restarts");
    }

    #[test]
    fn corrupt_file_is_set_aside() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("playlists.json");
        fs::write(&p, "nope").unwrap();
        let s = PlaylistStore::open(&p).unwrap();
        assert!(s.list().is_empty());
        assert_eq!(fs::read_to_string(d.path().join("playlists.json.bad")).unwrap(), "nope");
    }

    #[test]
    fn hand_edited_file_with_stale_next_id_cannot_reuse_ids() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("playlists.json");
        fs::write(&p, r#"{"version":1,"next_id":1,"playlists":[{"id":7,"name":"x","tracks":[],"created":0,"modified":0}]}"#).unwrap();
        let mut s = PlaylistStore::open(&p).unwrap();
        assert_eq!(s.create("y"), 8);
    }

    // ---- M3U ----

    #[test]
    fn parse_resolves_relative_paths_and_skips_comments() {
        let text = "#EXTM3U\n#EXTINF:123,Artist - Title\nsong one.flac\n\n  sub/two.opus  \n# a comment\n../up/three.mp3\n/abs/four.flac\n";
        let got = parse_m3u(text, Path::new("/music/lists"));
        assert_eq!(
            got,
            paths(&["/music/lists/song one.flac", "/music/lists/sub/two.opus", "/music/up/three.mp3", "/abs/four.flac"])
        );
    }

    #[test]
    fn parse_handles_crlf_bom_and_backslashes() {
        let text = "\u{feff}#EXTM3U\r\nArtist\\Album\\01 a.flac\r\n..\\other\\b.opus\r\n";
        let got = parse_m3u(text, Path::new("/m/lists"));
        if cfg!(windows) {
            assert_eq!(got.len(), 2);
        } else {
            assert_eq!(got, paths(&["/m/lists/Artist/Album/01 a.flac", "/m/other/b.opus"]));
        }
    }

    #[test]
    fn parse_decodes_file_urls_and_drops_streams() {
        let text = "file:///home/me/My%20Music/a%26b.flac\nhttp://radio.example/stream\nmms://x/y\nFILE:///C:/Music/c.flac\n";
        let got = parse_m3u(text, Path::new("/base"));
        assert_eq!(got.len(), 2, "streams are dropped: {got:?}");
        assert_eq!(got[0], PathBuf::from("/home/me/My Music/a&b.flac"));
        assert_eq!(got[1], PathBuf::from("C:/Music/c.flac"), "the slash before a drive letter is removed");
    }

    #[test]
    fn percent_decoding_edge_cases() {
        assert_eq!(percent_decode("a%2"), "a%2", "truncated escape stays literal");
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz", "invalid hex stays literal");
        assert_eq!(percent_decode("%E2%9C%93"), "✓", "multi-byte UTF-8");
        assert_eq!(percent_decode("%41%"), "A%");
    }

    #[test]
    fn decode_falls_back_to_latin1_for_legacy_files() {
        assert_eq!(decode_m3u("Beyoncé.flac".as_bytes()), "Beyoncé.flac");
        assert_eq!(decode_m3u(b"Beyonc\xe9.flac"), "Beyoncé.flac", "0xE9 alone isn't valid UTF-8, so it's Latin-1");
        let mut utf16 = vec![0xFF, 0xFE];
        utf16.extend("a.flac".encode_utf16().flat_map(|u| u.to_le_bytes()));
        assert_eq!(decode_m3u(&utf16), "a.flac");
        assert_eq!(decode_m3u(b"\xEF\xBB\xBFa.flac"), "a.flac");
    }

    #[test]
    fn export_writes_relative_paths_and_extinf() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (mut s, _st) = store();
        let inside = root.join("Album/01 a.flac");
        let outside = PathBuf::from("/elsewhere/b.opus");
        let id = s.create_with("mix", vec![inside.clone(), outside.clone(), root.join("nolabel.mp3")]);
        let dest = root.join("mix.m3u8");
        s.export_m3u(id, &dest, &|p| {
            if p == inside {
                Some((200.4, "The Artist - A Song\nwith newline".into()))
            } else if p == outside {
                Some((59.6, "B".into()))
            } else {
                None
            }
        })
        .unwrap();
        let text = fs::read_to_string(&dest).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(
            lines,
            vec![
                "#EXTM3U",
                "#EXTINF:200,The Artist - A Song with newline",
                "Album/01 a.flac",
                "#EXTINF:60,B",
                "/elsewhere/b.opus",
                "nolabel.mp3",
            ]
        );
        assert!(s.export_m3u(77, &dest, &|_| None).is_err());
    }

    #[test]
    fn export_then_import_round_trips_including_unicode_names() {
        let d = tempfile::tempdir().unwrap();
        let root = d.path();
        let (mut s, _st) = store();
        let tracks = vec![root.join("Björk/Homogenic/01 Hunter.flac"), root.join("日本語/曲.opus"), PathBuf::from("/abs/else where/x.mp3")];
        let id = s.create_with("rt", tracks.clone());
        let file = root.join("my mix.m3u8");
        s.export_m3u(id, &file, &|_| None).unwrap();
        let (new_id, n) = s.import_m3u(&file).unwrap();
        assert_eq!(n, 3);
        assert_ne!(new_id, id);
        let imported = s.get(new_id).unwrap();
        assert_eq!(imported.name, "my mix", "named after the file");
        assert_eq!(imported.tracks, tracks);
    }

    #[test]
    fn import_of_missing_file_is_an_error_and_creates_nothing() {
        let (mut s, d) = store();
        assert!(s.import_m3u(&d.path().join("nope.m3u")).is_err());
        assert!(s.list().is_empty());
    }

    #[test]
    fn normalize_collapses_dots_without_touching_the_filesystem() {
        assert_eq!(normalize(Path::new("/a/b/../c/./d")), PathBuf::from("/a/c/d"));
        assert_eq!(normalize(Path::new("a/../../b")), PathBuf::from("../b"));
        assert_eq!(normalize(Path::new("/../a")), PathBuf::from("/a"), "can't go above the root");
    }
}
