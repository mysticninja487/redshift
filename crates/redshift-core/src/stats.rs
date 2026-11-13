//! Play counts, last-played times and favourites, kept in their own file (`stats.json`) so
//! rescanning or rebuilding the library never loses them.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::engine::player::CurrentTrack;
use crate::error::Result;
use crate::library::write_json_atomic;
use crate::queue::EntryId;

const DB_VERSION: u32 = 1;

pub fn now_secs() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrackStat {
    /// Times the track was listened to (see [`PlayCounter`] for what counts).
    pub plays: u32,
    /// Seconds since the epoch when it last started playing; 0 if never.
    pub last_played: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Favourite {
    path: PathBuf,
    added: u64,
}

#[derive(Serialize, Deserialize, Default)]
#[serde(default)]
struct Db {
    version: u32,
    tracks: Vec<(PathBuf, TrackStat)>,
    favourites: Vec<Favourite>,
}

pub struct Stats {
    path: PathBuf,
    tracks: HashMap<PathBuf, TrackStat>,
    favourites: Vec<Favourite>,
}

impl Stats {
    /// Opens the file, or starts empty if there isn't one. A damaged file is set aside as `.bad`.
    pub fn open(path: impl Into<PathBuf>) -> Result<Self> {
        let path = path.into();
        let mut stats = Stats { path: path.clone(), tracks: HashMap::new(), favourites: Vec::new() };
        match std::fs::read(&path) {
            Ok(bytes) => match serde_json::from_slice::<Db>(&bytes) {
                Ok(db) => {
                    stats.tracks = db.tracks.into_iter().collect();
                    stats.favourites = db.favourites;
                }
                Err(_) => {
                    let mut bad = path.as_os_str().to_owned();
                    bad.push(".bad");
                    let _ = std::fs::rename(&path, PathBuf::from(bad));
                }
            },
            Err(_) => {}
        }
        Ok(stats)
    }

    pub fn save(&self) -> Result<()> {
        let mut tracks: Vec<_> = self.tracks.iter().map(|(p, s)| (p.clone(), *s)).collect();
        tracks.sort_by(|a, b| a.0.cmp(&b.0));
        write_json_atomic(&self.path, &Db { version: DB_VERSION, tracks, favourites: self.favourites.clone() })
    }

    pub fn get(&self, path: &Path) -> TrackStat {
        self.tracks.get(path).copied().unwrap_or_default()
    }
    pub fn all(&self) -> &HashMap<PathBuf, TrackStat> {
        &self.tracks
    }

    /// The track began playing now.
    pub fn record_start(&mut self, path: &Path, at: u64) {
        self.tracks.entry(path.to_path_buf()).or_default().last_played = at;
    }
    /// The track was listened to.
    pub fn record_play(&mut self, path: &Path) {
        let s = self.tracks.entry(path.to_path_buf()).or_default();
        s.plays = s.plays.saturating_add(1);
    }
    /// Forgets the counts of one track (or of all of them if `path` is `None`).
    pub fn reset(&mut self, path: Option<&Path>) {
        match path {
            Some(p) => {
                self.tracks.remove(p);
            }
            None => self.tracks.clear(),
        }
    }

    pub fn is_favourite(&self, path: &Path) -> bool {
        self.favourites.iter().any(|f| f.path == path)
    }
    /// Returns true if anything changed.
    pub fn set_favourite(&mut self, path: &Path, on: bool, at: u64) -> bool {
        let had = self.is_favourite(path);
        match (had, on) {
            (false, true) => {
                self.favourites.push(Favourite { path: path.to_path_buf(), added: at });
                true
            }
            (true, false) => {
                self.favourites.retain(|f| f.path != path);
                true
            }
            _ => false,
        }
    }
    /// Favourites, most recently added first.
    pub fn favourites(&self) -> Vec<PathBuf> {
        let mut v: Vec<&Favourite> = self.favourites.iter().collect();
        v.sort_by(|a, b| b.added.cmp(&a.added));
        v.into_iter().map(|f| f.path.clone()).collect()
    }
}

/// How much of a track has to be heard to count as a play, as a fraction of its length. The person
/// sets it (a percentage); half is the default.
pub const DEFAULT_COUNT_FRACTION: f64 = 0.5;
/// A track that doesn't say how long it is counts after this many seconds.
const UNKNOWN_LENGTH_COUNT_SECS: f64 = 30.0;

/// Seconds of listening a track needs before it counts. Never less than a second, so brushing past
/// a track with the setting at its lowest isn't a play.
pub fn secs_needed(duration: Option<f64>, fraction: f64) -> f64 {
    match duration.filter(|d| d.is_finite() && *d > 0.0) {
        Some(d) => (d * fraction.clamp(0.01, 1.0)).max(1.0).min(d),
        None => UNKNOWN_LENGTH_COUNT_SECS,
    }
}

/// Decides when a play counts: the track has to be heard for a set share of its length, so skipping
/// past it doesn't count. Fed once per tick with what the player is doing.
#[derive(Debug, Default)]
pub struct PlayCounter {
    entry: Option<EntryId>,
    heard: f64,
    last_pos: f64,
    counted: bool,
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct PlayTick {
    /// The track just started.
    pub started: bool,
    /// The track has been heard for long enough (reported once per listen).
    pub counted: bool,
}

impl PlayCounter {
    /// `dt` is the time since the previous tick; `pos` the playback position in seconds; `fraction` the
    /// share of the track that has to be heard (0.01 to 1).
    pub fn tick(&mut self, current: Option<&CurrentTrack>, playing: bool, pos: f64, dt: f64, fraction: f64) -> PlayTick {
        let Some(cur) = current else {
            *self = PlayCounter::default();
            return PlayTick::default();
        };
        let mut started = false;
        if self.entry != Some(cur.entry_id) {
            *self = PlayCounter { entry: Some(cur.entry_id), ..Default::default() };
            started = true;
        } else if self.counted && pos + 2.0 < self.last_pos {
            // Repeat-one (or a seek back to the start): a new listen begins.
            self.counted = false;
            self.heard = 0.0;
            started = true;
        }
        self.last_pos = pos;
        if playing {
            self.heard += dt.clamp(0.0, 5.0);
        }
        let need = secs_needed(cur.duration_secs, fraction);
        let counted = !self.counted && self.heard >= need;
        self.counted |= counted;
        PlayTick { started, counted }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cur(id: u64, dur: Option<f64>) -> CurrentTrack {
        CurrentTrack { entry_id: id, path: PathBuf::from("/a.flac"), codec: "flac", source_rate: 44100, source_channels: 2, bits_per_sample: None, duration_secs: dur }
    }

    #[test]
    fn counts_play_favourites_round_trip_and_survive_reopen() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("stats.json");
        let mut s = Stats::open(&f).unwrap();
        let (a, b) = (Path::new("/a.flac"), Path::new("/b.flac"));
        s.record_start(a, 100);
        s.record_play(a);
        s.record_play(a);
        assert!(s.set_favourite(a, true, 10));
        assert!(!s.set_favourite(a, true, 11), "already a favourite");
        assert!(s.set_favourite(b, true, 20));
        assert_eq!(s.favourites(), vec![b.to_path_buf(), a.to_path_buf()], "newest first");
        s.save().unwrap();
        let mut s = Stats::open(&f).unwrap();
        assert_eq!(s.get(a), TrackStat { plays: 2, last_played: 100 });
        assert_eq!(s.get(b), TrackStat::default());
        assert!(s.set_favourite(b, false, 0));
        assert_eq!(s.favourites(), vec![a.to_path_buf()]);
        s.reset(Some(a));
        assert_eq!(s.get(a).plays, 0);
    }

    #[test]
    fn damaged_file_is_set_aside() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("stats.json");
        std::fs::write(&f, "{nope").unwrap();
        let s = Stats::open(&f).unwrap();
        assert!(s.all().is_empty());
        assert!(d.path().join("stats.json.bad").exists());
    }

    fn started() -> PlayTick {
        PlayTick { started: true, counted: false }
    }

    #[test]
    fn a_play_counts_after_the_chosen_share_of_the_track() {
        // Half of a 20 second track is 10 seconds (the first tick is the "started" one, so 10 ticks in all).
        let mut c = PlayCounter::default();
        let t = cur(1, Some(20.0));
        let mut at = None;
        for i in 0..12 {
            if c.tick(Some(&t), true, i as f64, 1.0, 0.5).counted {
                at = Some(i);
            }
        }
        assert_eq!(at, Some(9));
        // The same track at 90% needs 18 seconds; at 1% it needs only the one-second floor.
        for (fraction, tick) in [(0.9, 17), (0.01, 0)] {
            let mut c = PlayCounter::default();
            let mut at = None;
            for i in 0..22 {
                if c.tick(Some(&t), true, i as f64, 1.0, fraction).counted {
                    at.get_or_insert(i);
                }
            }
            assert_eq!(at, Some(tick), "{fraction}");
        }
        // Counted once per listen, however long it plays on.
        let mut c = PlayCounter::default();
        let long = cur(2, Some(200.0));
        let n = (0..150).filter(|i| c.tick(Some(&long), true, *i as f64, 1.0, 0.5).counted).count();
        assert_eq!(n, 1);
    }

    #[test]
    fn how_long_a_track_needs_follows_the_setting() {
        assert_eq!(secs_needed(Some(200.0), 0.5), 100.0);
        assert_eq!(secs_needed(Some(200.0), 1.0), 200.0);
        assert_eq!(secs_needed(Some(200.0), 0.1), 20.0);
        assert_eq!(secs_needed(Some(10.0), 0.01), 1.0, "never less than a second");
        assert_eq!(secs_needed(Some(0.5), 0.5), 0.5, "and never more than the track");
        assert_eq!(secs_needed(Some(200.0), 5.0), 200.0, "out-of-range settings are held in range");
        assert_eq!(secs_needed(None, 0.5), 30.0);
        assert_eq!(secs_needed(Some(f64::NAN), 0.5), 30.0);
    }

    #[test]
    fn paused_time_and_skipped_tracks_do_not_count() {
        let mut c = PlayCounter::default();
        let t = cur(1, Some(200.0));
        for _ in 0..100 {
            assert!(!c.tick(Some(&t), false, 0.0, 1.0, 0.5).counted);
        }
        for i in 0..10 {
            assert!(!c.tick(Some(&t), true, i as f64, 1.0, 0.5).counted);
        }
        // Skipping to the next entry starts over.
        let next = cur(2, Some(200.0));
        assert_eq!(c.tick(Some(&next), true, 0.0, 1.0, 0.5), started());
        assert_eq!(c.tick(None, false, 0.0, 1.0, 0.5), PlayTick::default());
    }

    #[test]
    fn a_very_short_track_reports_start_and_count_together() {
        let mut c = PlayCounter::default();
        let t = cur(1, Some(1.0));
        assert_eq!(c.tick(Some(&t), true, 0.0, 2.0, 0.5), PlayTick { started: true, counted: true });
    }

    #[test]
    fn repeat_one_counts_again_after_the_track_loops() {
        let mut c = PlayCounter::default();
        let t = cur(1, Some(10.0));
        let mut n = 0;
        for lap in 0..2 {
            for i in 0..10 {
                n += c.tick(Some(&t), true, i as f64, 1.0, 0.5).counted as usize;
            }
            assert_eq!(n, lap + 1);
        }
    }
}
