//! A listening log: how long each track was heard, and when. It feeds the Stats screen and the
//! mixes. It is kept in its own file (`history.json`) so clearing or damaging it never touches the
//! play counts, favourites or playlists.
//!
//! Time is stored in 15-minute buckets, which is fine enough to group by hour of day in any time
//! zone (every real zone offset is a whole number of quarter hours) and coarse enough that a
//! heavy listener produces a few tens of thousands of rows a year.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::Result;
use crate::library::write_json_atomic;

const DB_VERSION: u32 = 1;
/// Seconds in one bucket.
pub const BUCKET_SECS: u64 = 900;
/// More rows than this and the oldest are dropped (about ten years of heavy listening).
const MAX_ROWS: usize = 400_000;

/// What was heard of one track in one bucket.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Row {
    bucket: u32,
    track: u32,
    secs: f32,
    plays: u16,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Db {
    version: u32,
    paths: Vec<PathBuf>,
    /// `[bucket, track index, seconds, plays]`
    rows: Vec<(u32, u32, f32, u16)>,
}

/// What one track added up to over a stretch of time.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Heard {
    pub secs: f64,
    pub plays: u32,
}

pub struct History {
    path: PathBuf,
    paths: Vec<PathBuf>,
    index: HashMap<PathBuf, u32>,
    rows: Vec<Row>,
    at: HashMap<(u32, u32), usize>,
}

impl History {
    /// Opens the file, or starts empty if there isn't one. A damaged file is set aside as `.bad`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut h = History { path: path.clone(), paths: Vec::new(), index: HashMap::new(), rows: Vec::new(), at: HashMap::new() };
        if let Ok(bytes) = std::fs::read(&path) {
            match serde_json::from_slice::<Db>(&bytes) {
                Ok(db) => h.load(db),
                Err(_) => {
                    let mut bad = path.as_os_str().to_owned();
                    bad.push(".bad");
                    let _ = std::fs::rename(&path, PathBuf::from(bad));
                }
            }
        }
        Ok(h)
    }

    fn load(&mut self, db: Db) {
        self.index = db.paths.iter().enumerate().map(|(i, p)| (p.clone(), i as u32)).collect();
        self.paths = db.paths;
        for (bucket, track, secs, plays) in db.rows {
            // A row that points at a path that isn't there is skipped rather than trusted.
            if (track as usize) < self.paths.len() && secs.is_finite() && secs >= 0.0 {
                self.push(Row { bucket, track, secs, plays });
            }
        }
    }

    fn push(&mut self, row: Row) {
        if let Some(&i) = self.at.get(&(row.bucket, row.track)) {
            self.rows[i].secs += row.secs;
            self.rows[i].plays = self.rows[i].plays.saturating_add(row.plays);
        } else {
            self.at.insert((row.bucket, row.track), self.rows.len());
            self.rows.push(row);
        }
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }
    pub fn len(&self) -> usize {
        self.rows.len()
    }

    /// Adds `secs` of listening to `path` at time `at` (seconds since the epoch), with `plays` more counted listens.
    pub fn add(&mut self, path: &Path, at: u64, secs: f64, plays: u16) {
        if !(secs.is_finite() && secs > 0.0) && plays == 0 {
            return;
        }
        let track = match self.index.get(path) {
            Some(&i) => i,
            None => {
                let i = self.paths.len() as u32;
                self.paths.push(path.to_path_buf());
                self.index.insert(path.to_path_buf(), i);
                i
            }
        };
        let bucket = (at / BUCKET_SECS).min(u32::MAX as u64) as u32;
        self.push(Row { bucket, track, secs: secs.max(0.0) as f32, plays });
    }

    pub fn clear(&mut self) {
        self.paths.clear();
        self.index.clear();
        self.rows.clear();
        self.at.clear();
    }

    pub fn save(&mut self) -> Result<()> {
        if self.rows.len() > MAX_ROWS {
            self.rows.sort_by_key(|r| r.bucket);
            self.rows.drain(..self.rows.len() - MAX_ROWS);
            self.reindex();
        }
        let rows = self.rows.iter().map(|r| (r.bucket, r.track, (r.secs * 10.0).round() / 10.0, r.plays)).collect();
        write_json_atomic(&self.path, &Db { version: DB_VERSION, paths: self.paths.clone(), rows })
    }

    /// Rebuilds the lookup tables after rows were dropped.
    fn reindex(&mut self) {
        let old = std::mem::take(&mut self.rows);
        let paths = std::mem::take(&mut self.paths);
        self.index.clear();
        self.at.clear();
        for r in old {
            let p = &paths[r.track as usize];
            let track = *self.index.entry(p.clone()).or_insert_with(|| {
                self.paths.push(p.clone());
                (self.paths.len() - 1) as u32
            });
            self.push(Row { track, ..r });
        }
    }

    fn in_range(row: &Row, from: u64, to: u64) -> bool {
        let t = row.bucket as u64 * BUCKET_SECS;
        t >= from && t < to
    }

    /// Total listening time and counted plays between `from` (included) and `to` (excluded).
    pub fn total(&self, from: u64, to: u64) -> Heard {
        let mut out = Heard::default();
        for r in self.rows.iter().filter(|r| Self::in_range(r, from, to)) {
            out.secs += r.secs as f64;
            out.plays += r.plays as u32;
        }
        out
    }

    /// What each track added up to in the range.
    pub fn per_track(&self, from: u64, to: u64) -> HashMap<&Path, Heard> {
        let mut out: HashMap<&Path, Heard> = HashMap::new();
        for r in self.rows.iter().filter(|r| Self::in_range(r, from, to)) {
            let h = out.entry(self.paths[r.track as usize].as_path()).or_default();
            h.secs += r.secs as f64;
            h.plays += r.plays as u32;
        }
        out
    }

    /// Seconds heard in each hour of the day (0..24) in a zone `tz_offset_min` minutes ahead of UTC.
    pub fn by_hour(&self, from: u64, to: u64, tz_offset_min: i32) -> [f64; 24] {
        let mut out = [0.0; 24];
        for r in self.rows.iter().filter(|r| Self::in_range(r, from, to)) {
            let local = r.bucket as i64 * BUCKET_SECS as i64 + tz_offset_min as i64 * 60;
            out[(local.div_euclid(3600).rem_euclid(24)) as usize] += r.secs as f64;
        }
        out
    }

    /// Tracks heard before `before` (seconds since the epoch), with the time of the first listen.
    pub fn first_heard(&self) -> HashMap<&Path, u64> {
        let mut out: HashMap<&Path, u64> = HashMap::new();
        for r in &self.rows {
            let t = r.bucket as u64 * BUCKET_SECS;
            let e = out.entry(self.paths[r.track as usize].as_path()).or_insert(t);
            *e = (*e).min(t);
        }
        out
    }

    /// When each track was last heard.
    pub fn last_heard(&self) -> HashMap<&Path, u64> {
        let mut out: HashMap<&Path, u64> = HashMap::new();
        for r in &self.rows {
            let t = r.bucket as u64 * BUCKET_SECS;
            let e = out.entry(self.paths[r.track as usize].as_path()).or_insert(t);
            *e = (*e).max(t);
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: u64 = 86_400;

    fn open(d: &Path) -> History {
        History::open(d.join("history.json")).unwrap()
    }

    #[test]
    fn adds_up_per_range_and_survives_a_reopen() {
        let d = tempfile::tempdir().unwrap();
        let mut h = open(d.path());
        let (a, b) = (Path::new("/a.flac"), Path::new("/b.flac"));
        h.add(a, 10 * DAY + 100, 60.0, 0);
        h.add(a, 10 * DAY + 200, 30.0, 1); // same bucket: one row
        h.add(b, 11 * DAY + 100, 45.0, 1);
        assert_eq!(h.len(), 2);
        assert_eq!(h.total(10 * DAY, 11 * DAY), Heard { secs: 90.0, plays: 1 });
        assert_eq!(h.total(10 * DAY, 12 * DAY), Heard { secs: 135.0, plays: 2 });
        assert_eq!(h.total(0, 10 * DAY), Heard::default());
        h.save().unwrap();
        let h = open(d.path());
        assert_eq!(h.total(0, 100 * DAY).secs, 135.0);
        let per = h.per_track(0, 100 * DAY);
        assert_eq!(per[a], Heard { secs: 90.0, plays: 1 });
        assert_eq!(per[b].plays, 1);
    }

    #[test]
    fn hours_follow_the_time_zone() {
        let d = tempfile::tempdir().unwrap();
        let mut h = open(d.path());
        // 22:30 UTC.
        h.add(Path::new("/a.flac"), 5 * DAY + 22 * 3600 + 1800, 120.0, 0);
        let utc = h.by_hour(0, 10 * DAY, 0);
        assert_eq!(utc[22], 120.0);
        // Six hours ahead (Dhaka): 04:30 the next morning.
        assert_eq!(h.by_hour(0, 10 * DAY, 360)[4], 120.0);
        // Behind UTC, across midnight the other way.
        assert_eq!(h.by_hour(0, 10 * DAY, -23 * 60)[23], 120.0);
    }

    #[test]
    fn first_and_last_heard() {
        let d = tempfile::tempdir().unwrap();
        let mut h = open(d.path());
        let a = Path::new("/a.flac");
        h.add(a, 3 * DAY, 10.0, 0);
        h.add(a, 9 * DAY, 10.0, 0);
        assert_eq!(h.first_heard()[a], 3 * DAY);
        assert_eq!(h.last_heard()[a], 9 * DAY);
    }

    #[test]
    fn damaged_or_inconsistent_files_are_handled() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("history.json");
        std::fs::write(&f, "{nope").unwrap();
        assert!(History::open(&f).unwrap().is_empty());
        assert!(d.path().join("history.json.bad").exists());
        // A row pointing past the path table, and a negative time, are skipped.
        std::fs::write(&f, r#"{"version":1,"paths":["/a.flac"],"rows":[[1,0,5.0,1],[2,7,5.0,1],[3,0,-4.0,0]]}"#).unwrap();
        let h = History::open(&f).unwrap();
        assert_eq!(h.len(), 1);
    }

    #[test]
    fn clearing_forgets_everything_and_nothing_is_recorded_for_silence() {
        let d = tempfile::tempdir().unwrap();
        let mut h = open(d.path());
        h.add(Path::new("/a.flac"), DAY, 0.0, 0);
        assert!(h.is_empty());
        h.add(Path::new("/a.flac"), DAY, 12.0, 0);
        h.clear();
        assert!(h.is_empty());
        h.save().unwrap();
        assert!(open(d.path()).is_empty());
    }

    #[test]
    fn the_oldest_rows_go_first_when_it_gets_too_big() {
        let d = tempfile::tempdir().unwrap();
        let mut h = open(d.path());
        for i in 0..(MAX_ROWS as u64 + 10) {
            h.add(Path::new(if i % 2 == 0 { "/a.flac" } else { "/b.flac" }), i * BUCKET_SECS, 1.0, 0);
        }
        h.save().unwrap();
        assert_eq!(h.len(), MAX_ROWS);
        assert_eq!(h.total(0, 10 * BUCKET_SECS).secs, 0.0, "the first rows were dropped");
        assert_eq!(h.total(0, u64::MAX).secs, MAX_ROWS as f64);
    }
}
