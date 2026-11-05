//! The music library: which folders to watch, what's in them, and ways to browse it.
//!
//! Scanning is a pure function of (folders, previously known tracks), so it can run on a worker
//! thread without holding any lock the UI needs; the result is then applied to the [`Library`]
//! in one quick step. Rescans only re-read files whose size or modification time changed.
//!
//! The library is stored as one JSON file, written atomically (temp file, then rename), so a crash
//! or power cut mid-save leaves the previous version intact.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use crossbeam_channel::unbounded;
use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::error::{Error, Result};
use crate::meta::{self, TrackMeta};

const DB_VERSION: u32 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ScanPhase {
    /// Walking folders to see what's there.
    Discovering,
    /// Reading tags from new or changed files.
    Reading,
}

#[derive(Debug, Clone, Serialize)]
pub struct ScanProgress {
    pub phase: ScanPhase,
    pub done: usize,
    pub total: usize,
    pub current: Option<PathBuf>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct ScanReport {
    pub added: usize,
    pub updated: usize,
    pub removed: usize,
    pub unchanged: usize,
    /// Files that looked like audio but couldn't be read, with the reason.
    pub failed: Vec<(PathBuf, String)>,
}

pub struct ScanResult {
    pub tracks: Vec<TrackMeta>,
    pub report: ScanReport,
}

/// Scans `folders` for audio files. `known` is what the library already has; files that haven't
/// changed since are carried over without being read again.
pub fn scan(
    folders: &[PathBuf],
    known: &HashMap<PathBuf, TrackMeta>,
    progress: &mut dyn FnMut(ScanProgress),
) -> ScanResult {
    // 1. Discover. Overlapping folders must not produce duplicates, hence the set.
    let mut seen = HashSet::new();
    let mut files = Vec::new();
    for folder in folders {
        for entry in WalkDir::new(folder).follow_links(false).into_iter().filter_map(|e| e.ok()) {
            if entry.file_type().is_file() && meta::is_audio_file(entry.path()) && seen.insert(entry.path().to_path_buf()) {
                files.push(entry.into_path());
                if files.len() % 200 == 0 {
                    progress(ScanProgress { phase: ScanPhase::Discovering, done: files.len(), total: 0, current: None });
                }
            }
        }
    }
    progress(ScanProgress { phase: ScanPhase::Discovering, done: files.len(), total: files.len(), current: None });

    // 2. Split into unchanged (reuse) and to-read.
    let mut report = ScanReport::default();
    let mut tracks = Vec::with_capacity(files.len());
    let mut to_read = Vec::new();
    for path in files {
        let stat = fs::metadata(&path).ok().map(|m| {
            let mtime = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs());
            (mtime, m.len())
        });
        match (known.get(&path), stat) {
            (Some(old), Some((mtime, size))) if old.modified == mtime && old.size == size => {
                let mut t = old.clone();
                if t.added == 0 {
                    t.added = t.modified;
                }
                tracks.push(t);
                report.unchanged += 1;
            }
            _ => to_read.push(path),
        }
    }
    report.removed = known.keys().filter(|p| !seen.contains(*p)).count();

    // 3. Read tags on a few threads; results come back over a channel so progress stays on the
    //    calling thread.
    let total = to_read.len();
    let next = AtomicUsize::new(0);
    let workers = std::thread::available_parallelism().map_or(2, |n| n.get()).clamp(1, 8).min(total.max(1));
    let (tx, rx) = unbounded::<(PathBuf, Result<TrackMeta>)>();
    std::thread::scope(|scope| {
        for _ in 0..workers {
            let (tx, next, to_read) = (tx.clone(), &next, &to_read);
            scope.spawn(move || loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                let Some(path) = to_read.get(i) else { break };
                if tx.send((path.clone(), meta::read_meta(path))).is_err() {
                    break;
                }
            });
        }
        drop(tx);
        let mut done = 0;
        for (path, result) in rx.iter() {
            done += 1;
            match result {
                Ok(mut m) => {
                    match known.get(&path) {
                        Some(old) => {
                            report.updated += 1;
                            m.added = if old.added > 0 { old.added } else { old.modified };
                        }
                        None => {
                            report.added += 1;
                            // The first scan of a library says nothing about when each file
                            // arrived, so the file's own date stands in; later scans stamp "now".
                            m.added = if known.is_empty() { m.modified } else { crate::stats::now_secs() };
                        }
                    }
                    tracks.push(m);
                }
                Err(e) => report.failed.push((path.clone(), e.to_string())),
            }
            progress(ScanProgress { phase: ScanPhase::Reading, done, total, current: Some(path) });
        }
    });

    ScanResult { tracks, report }
}

#[derive(Serialize, Deserialize)]
struct Db {
    version: u32,
    folders: Vec<PathBuf>,
    tracks: Vec<TrackMeta>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Album {
    pub title: String,
    pub artist: String,
    pub year: Option<u16>,
    pub track_count: usize,
    pub duration_secs: f64,
    /// Tracks in disc/track order.
    pub tracks: Vec<PathBuf>,
}

#[derive(Debug, Clone, Serialize)]
pub struct Artist {
    pub name: String,
    pub album_count: usize,
    pub track_count: usize,
}

pub struct Library {
    db_path: PathBuf,
    folders: Vec<PathBuf>,
    tracks: HashMap<PathBuf, TrackMeta>,
}

impl Library {
    /// Loads the library from `db_path`, or starts empty if there's nothing there. A file that
    /// can't be parsed is set aside as `.bad` rather than overwritten, and the library starts empty.
    pub fn open(db_path: impl Into<PathBuf>) -> Result<Self> {
        let db_path = db_path.into();
        let mut lib = Self { db_path, folders: Vec::new(), tracks: HashMap::new() };
        match fs::read(&lib.db_path) {
            Ok(bytes) => match serde_json::from_slice::<Db>(&bytes) {
                Ok(db) if db.version <= DB_VERSION => {
                    lib.folders = db.folders;
                    lib.tracks = db.tracks.into_iter().map(|t| (t.path.clone(), t)).collect();
                }
                _ => {
                    let mut bad = lib.db_path.clone().into_os_string();
                    bad.push(".bad");
                    let _ = fs::rename(&lib.db_path, PathBuf::from(bad));
                }
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
        Ok(lib)
    }

    pub fn save(&self) -> Result<()> {
        let db = Db {
            version: DB_VERSION,
            folders: self.folders.clone(),
            tracks: self.tracks.values().cloned().collect(),
        };
        write_json_atomic(&self.db_path, &db)
    }

    pub fn folders(&self) -> &[PathBuf] {
        &self.folders
    }

    /// Returns false if the folder was already in the library.
    pub fn add_folder(&mut self, folder: impl Into<PathBuf>) -> bool {
        let folder = folder.into();
        if self.folders.contains(&folder) {
            return false;
        }
        self.folders.push(folder);
        true
    }

    /// Removes a folder and its tracks (unless another folder still covers them).
    pub fn remove_folder(&mut self, folder: &Path) -> bool {
        let before = self.folders.len();
        self.folders.retain(|f| f != folder);
        if self.folders.len() == before {
            return false;
        }
        let remaining = self.folders.clone();
        self.tracks.retain(|p, _| remaining.iter().any(|f| p.starts_with(f)));
        true
    }

    /// What a scan needs to start from. Cheap enough to clone out from under a lock.
    pub fn scan_inputs(&self) -> (Vec<PathBuf>, HashMap<PathBuf, TrackMeta>) {
        (self.folders.clone(), self.tracks.clone())
    }

    /// Replaces the track list with a scan's result.
    pub fn apply_scan(&mut self, result: ScanResult) {
        self.tracks = result.tracks.into_iter().map(|t| (t.path.clone(), t)).collect();
    }

    pub fn len(&self) -> usize {
        self.tracks.len()
    }
    pub fn is_empty(&self) -> bool {
        self.tracks.is_empty()
    }
    pub fn get(&self, path: &Path) -> Option<&TrackMeta> {
        self.tracks.get(path)
    }

    /// All tracks, ordered by artist, album, disc, track number, title.
    pub fn tracks(&self) -> Vec<&TrackMeta> {
        let mut v: Vec<_> = self.tracks.values().collect();
        v.sort_by(|a, b| browse_order(a, b));
        v
    }

    /// Tracks where every word of `query` appears in the title, artist, album, album artist or
    /// genre (case-insensitive).
    pub fn search(&self, query: &str) -> Vec<&TrackMeta> {
        let words: Vec<String> = query.split_whitespace().map(str::to_lowercase).collect();
        if words.is_empty() {
            return self.tracks();
        }
        let mut v: Vec<_> = self
            .tracks
            .values()
            .filter(|t| {
                let hay = format!("{}\n{}\n{}\n{}\n{}", t.title, t.artist, t.album, t.album_artist, t.genre).to_lowercase();
                words.iter().all(|w| hay.contains(w))
            })
            .collect();
        v.sort_by(|a, b| browse_order(a, b));
        v
    }

    pub fn albums(&self) -> Vec<Album> {
        let mut groups: BTreeMap<(String, String), Vec<&TrackMeta>> = BTreeMap::new();
        for t in self.tracks.values() {
            groups.entry((t.album_artist.to_lowercase(), t.album.to_lowercase())).or_default().push(t);
        }
        groups
            .into_values()
            .map(|mut ts| {
                ts.sort_by(|a, b| (a.disc_no, a.track_no, a.title.to_lowercase()).cmp(&(b.disc_no, b.track_no, b.title.to_lowercase())));
                Album {
                    title: ts[0].album.clone(),
                    artist: ts[0].album_artist.clone(),
                    year: ts.iter().filter_map(|t| t.year).min(),
                    track_count: ts.len(),
                    duration_secs: ts.iter().map(|t| t.duration_secs).sum(),
                    tracks: ts.iter().map(|t| t.path.clone()).collect(),
                }
            })
            .collect()
    }

    pub fn artists(&self) -> Vec<Artist> {
        let mut groups: BTreeMap<String, (String, HashSet<String>, usize)> = BTreeMap::new();
        for t in self.tracks.values() {
            let e = groups.entry(t.album_artist.to_lowercase()).or_insert_with(|| (t.album_artist.clone(), HashSet::new(), 0));
            e.1.insert(t.album.to_lowercase());
            e.2 += 1;
        }
        groups.into_values().map(|(name, albums, tracks)| Artist { name, album_count: albums.len(), track_count: tracks }).collect()
    }

    /// The tracks that joined the library most recently first, for a "recently added" view.
    pub fn recently_added(&self, limit: usize) -> Vec<&TrackMeta> {
        let mut v: Vec<_> = self.tracks.values().collect();
        v.sort_by(|a, b| b.added.cmp(&a.added).then_with(|| browse_order(a, b)));
        v.truncate(limit);
        v
    }
}

fn browse_order(a: &TrackMeta, b: &TrackMeta) -> std::cmp::Ordering {
    let key = |t: &TrackMeta| (t.album_artist.to_lowercase(), t.album.to_lowercase(), t.disc_no, t.track_no, t.title.to_lowercase());
    key(a).cmp(&key(b)).then_with(|| a.path.cmp(&b.path))
}

/// Writes JSON to `path` via a temporary file in the same directory, so readers never see a
/// half-written file and a crash leaves the old one in place.
pub fn write_json_atomic<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let dir = path.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    fs::create_dir_all(dir)?;
    let mut tmp_name = path.file_name().ok_or_else(|| Error::Other("path has no file name".into()))?.to_os_string();
    tmp_name.push(".tmp");
    let tmp = dir.join(tmp_name);
    {
        let mut f = fs::File::create(&tmp)?;
        serde_json::to_writer(&mut f, value).map_err(|e| Error::Other(e.to_string()))?;
        f.flush()?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    /// A music folder with some structure: two albums, an untagged file, junk and a nested folder.
    fn music_dir() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Testers/Fixture Album")).unwrap();
        fs::create_dir_all(root.join("Other/Deep/Deeper")).unwrap();
        fs::copy(fixture("tagged.flac"), root.join("Testers/Fixture Album/03 Tagged Song.flac")).unwrap();
        fs::copy(fixture("tagged.opus"), root.join("Other/opus.opus")).unwrap();
        fs::copy(fixture("tagged.mp3"), root.join("Other/Deep/Deeper/song.MP3")).unwrap();
        fs::copy(fixture("Untagged Name.flac"), root.join("Other/Untagged Name.flac")).unwrap();
        fs::write(root.join("Other/notes.txt"), "not audio").unwrap();
        fs::write(root.join("Other/cover.jpg"), "not audio").unwrap();
        fs::write(root.join("Other/broken.flac"), vec![9u8; 3000]).unwrap();
        dir
    }

    fn run_scan(folders: &[PathBuf], known: &HashMap<PathBuf, TrackMeta>) -> ScanResult {
        scan(folders, known, &mut |_| {})
    }

    #[test]
    fn scan_finds_nested_audio_ignores_other_files_and_reports_failures() {
        let dir = music_dir();
        let r = run_scan(&[dir.path().to_path_buf()], &HashMap::new());
        assert_eq!(r.tracks.len(), 4);
        assert_eq!((r.report.added, r.report.updated, r.report.unchanged, r.report.removed), (4, 0, 0, 0));
        assert_eq!(r.report.failed.len(), 1);
        assert!(r.report.failed[0].0.ends_with("broken.flac"));
        assert!(r.tracks.iter().any(|t| t.title == "MP3 Song"), "uppercase .MP3 in a nested folder");
    }

    #[test]
    fn rescan_reuses_unchanged_files_and_rereads_changed_ones() {
        let dir = music_dir();
        let folders = [dir.path().to_path_buf()];
        let first = run_scan(&folders, &HashMap::new());
        let known: HashMap<_, _> = first.tracks.into_iter().map(|t| (t.path.clone(), t)).collect();

        let again = run_scan(&folders, &known);
        assert_eq!((again.report.added, again.report.updated, again.report.unchanged), (0, 0, 4));

        // Change one file's content (different size) and add another.
        let changed = dir.path().join("Other/opus.opus");
        fs::copy(fixture("tone_48k_mono.opus"), &changed).unwrap();
        fs::copy(fixture("tagged.flac"), dir.path().join("new.flac")).unwrap();
        let third = run_scan(&folders, &known);
        assert_eq!((third.report.added, third.report.updated, third.report.unchanged), (1, 1, 3));
        let t = third.tracks.iter().find(|t| t.path == changed).unwrap();
        // The replacement has no title tag, so the title falls back to the file name ("opus.opus"),
        // not the tagged "Opus Song" from before: that proves the tags were read again.
        assert_eq!(t.title, "opus", "tags were re-read");
        assert_ne!(t.title, known[&changed].title);
    }

    #[test]
    fn rescan_drops_deleted_files() {
        let dir = music_dir();
        let folders = [dir.path().to_path_buf()];
        let known: HashMap<_, _> = run_scan(&folders, &HashMap::new()).tracks.into_iter().map(|t| (t.path.clone(), t)).collect();
        fs::remove_file(dir.path().join("Other/opus.opus")).unwrap();
        let r = run_scan(&folders, &known);
        assert_eq!(r.report.removed, 1);
        assert_eq!(r.tracks.len(), 3);
    }

    #[test]
    fn overlapping_folders_do_not_duplicate_tracks() {
        let dir = music_dir();
        let r = run_scan(&[dir.path().to_path_buf(), dir.path().join("Other")], &HashMap::new());
        assert_eq!(r.tracks.len(), 4);
    }

    #[test]
    fn missing_folder_is_just_empty() {
        let r = run_scan(&[PathBuf::from("/definitely/not/here")], &HashMap::new());
        assert!(r.tracks.is_empty() && r.report.failed.is_empty());
    }

    #[test]
    fn progress_is_reported_through_to_completion() {
        let dir = music_dir();
        let mut seen = Vec::new();
        scan(&[dir.path().to_path_buf()], &HashMap::new(), &mut |p| seen.push(p));
        let reads: Vec<_> = seen.iter().filter(|p| p.phase == ScanPhase::Reading).collect();
        assert_eq!(reads.len(), 5, "one update per file read, including the broken one");
        assert_eq!(reads.last().unwrap().done, reads.last().unwrap().total);
        assert!(reads.windows(2).all(|w| w[0].done < w[1].done));
        assert!(seen.iter().any(|p| p.phase == ScanPhase::Discovering));
    }

    fn library_of(dir: &tempfile::TempDir) -> (Library, tempfile::TempDir) {
        let state = tempfile::tempdir().unwrap();
        let mut lib = Library::open(state.path().join("library.json")).unwrap();
        lib.add_folder(dir.path());
        let (folders, known) = lib.scan_inputs();
        lib.apply_scan(run_scan(&folders, &known));
        (lib, state)
    }

    #[test]
    fn browsing_albums_artists_and_search() {
        let dir = music_dir();
        let (lib, _state) = library_of(&dir);
        assert_eq!(lib.len(), 4);

        let albums = lib.albums();
        let names: Vec<_> = albums.iter().map(|a| (a.artist.as_str(), a.title.as_str())).collect();
        assert!(names.contains(&("Various Testers", "Fixture Album")), "{names:?}");
        assert!(names.contains(&("MP3 Artist", "MP3 Album")));
        assert!(names.contains(&("Unknown Artist", "Unknown Album")));
        let fixture_album = albums.iter().find(|a| a.title == "Fixture Album").unwrap();
        assert_eq!((fixture_album.track_count, fixture_album.year), (1, Some(2021)));
        assert!((fixture_album.duration_secs - 1.0).abs() < 0.05);

        assert_eq!(lib.artists().len(), 4);
        assert!(lib.artists().iter().all(|a| a.album_count == 1 && a.track_count == 1));

        // Search: all words must match, anywhere in the main fields, any case.
        assert_eq!(lib.search("tagged song").len(), 1);
        assert_eq!(lib.search("OPUS artist").len(), 1);
        assert_eq!(lib.search("song artist").len(), 2, "title and artist words both match: Opus Song/Artist and MP3 Song/Artist");
        assert_eq!(lib.search("mp3 artist").len(), 1);
        assert_eq!(lib.search("test").len(), 1, "genre is searched too");
        assert!(lib.search("zebra").is_empty());
        assert_eq!(lib.search("   ").len(), 4, "empty query lists everything");
        // Sorted for browsing.
        let order: Vec<_> = lib.tracks().iter().map(|t| t.album_artist.to_lowercase()).collect();
        let mut sorted = order.clone();
        sorted.sort();
        assert_eq!(order, sorted);
    }

    #[test]
    fn album_tracks_are_in_disc_and_track_order() {
        let dir = tempfile::tempdir().unwrap();
        // Same album tag from one fixture copied three times with names that sort differently from
        // their track numbers can't be re-tagged here, so check ordering via the sort key directly.
        let mut a = TrackMeta { title: "b".into(), track_no: Some(2), disc_no: Some(1), ..Default::default() };
        let mut b = TrackMeta { title: "a".into(), track_no: Some(10), disc_no: Some(1), ..Default::default() };
        let c = TrackMeta { title: "z".into(), track_no: Some(1), disc_no: Some(2), ..Default::default() };
        a.path = dir.path().join("a");
        b.path = dir.path().join("b");
        let mut lib = Library::open(dir.path().join("l.json")).unwrap();
        let mut c = c;
        c.path = dir.path().join("c");
        for t in [c.clone(), b.clone(), a.clone()] {
            lib.tracks.insert(t.path.clone(), t);
        }
        let album = &lib.albums()[0];
        assert_eq!(album.tracks, vec![a.path, b.path, c.path], "disc 1 track 2, disc 1 track 10, disc 2 track 1");
    }

    #[test]
    fn save_and_reopen_round_trips() {
        let dir = music_dir();
        let (lib, state) = library_of(&dir);
        lib.save().unwrap();
        let reopened = Library::open(state.path().join("library.json")).unwrap();
        assert_eq!(reopened.len(), 4);
        assert_eq!(reopened.folders(), lib.folders());
        let p = dir.path().join("Testers/Fixture Album/03 Tagged Song.flac");
        assert_eq!(reopened.get(&p), lib.get(&p));
        assert!(!state.path().join("library.json.tmp").exists(), "temp file is gone after saving");
    }

    #[test]
    fn corrupt_database_is_set_aside_not_overwritten() {
        let state = tempfile::tempdir().unwrap();
        let db = state.path().join("library.json");
        fs::write(&db, "{ this is not json").unwrap();
        let lib = Library::open(&db).unwrap();
        assert!(lib.is_empty());
        assert!(!db.exists());
        assert_eq!(fs::read_to_string(state.path().join("library.json.bad")).unwrap(), "{ this is not json");
        // A database from a future version is treated the same way instead of being misread.
        fs::write(&db, r#"{"version":999,"folders":[],"tracks":[]}"#).unwrap();
        assert!(Library::open(&db).unwrap().is_empty());
    }

    #[test]
    fn folders_can_be_added_once_and_removed_with_their_tracks() {
        let dir = music_dir();
        let (mut lib, _state) = library_of(&dir);
        assert!(!lib.add_folder(dir.path()), "already present");
        lib.add_folder(dir.path().join("Other"));
        assert!(lib.remove_folder(dir.path()));
        assert_eq!(lib.len(), 3, "tracks under the remaining folder stay");
        assert!(lib.remove_folder(&dir.path().join("Other")));
        assert!(lib.is_empty());
        assert!(!lib.remove_folder(Path::new("/never/added")));
    }

    #[test]
    fn recently_added_orders_by_the_date_added() {
        let dir = music_dir();
        let (mut lib, _state) = library_of(&dir);
        let paths: Vec<_> = lib.tracks.keys().cloned().collect();
        for (i, p) in paths.iter().enumerate() {
            lib.tracks.get_mut(p).unwrap().added = 1_000 + i as u64;
        }
        let recent = lib.recently_added(2);
        assert_eq!(recent.len(), 2);
        assert!(recent[0].added > recent[1].added);
        assert_eq!(recent[0].added, 1_000 + paths.len() as u64 - 1);
    }

    #[test]
    fn atomic_write_replaces_content_and_creates_directories() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested/dir/file.json");
        write_json_atomic(&path, &vec![1, 2, 3]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "[1,2,3]");
        write_json_atomic(&path, &vec![9]).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "[9]");
    }
}
