//! What the listening history says: a report for a stretch of time (top songs, artists, albums,
//! genres, when you listen) and mixes made from what you play. Everything here is computed on
//! this machine from the library, the play counts and the history file.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::history::{Heard, History};
use crate::meta::TrackMeta;
use crate::stats::TrackStat;

const TOP: usize = 10;
const DAY: u64 = 86_400;
const MIX_MAX: usize = 40;
const MIX_MIN: usize = 6;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TopTrack {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub secs: f64,
    pub plays: u32,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TopArtist {
    pub name: String,
    pub secs: f64,
    pub plays: u32,
    /// How many different songs of theirs were heard.
    pub tracks: u32,
    /// A song to take the cover from: the one heard most.
    pub path: PathBuf,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct TopAlbum {
    pub album: String,
    pub artist: String,
    pub secs: f64,
    pub plays: u32,
    pub path: PathBuf,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct GenreShare {
    pub name: String,
    pub secs: f64,
}
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct DecadeShare {
    pub decade: u16,
    pub secs: f64,
}

#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct Report {
    pub secs: f64,
    pub plays: u32,
    /// Different songs and artists heard in the range.
    pub tracks_heard: u32,
    pub artists_heard: u32,
    /// Songs heard for the first time in the range.
    pub new_tracks: u32,
    pub top_tracks: Vec<TopTrack>,
    pub top_artists: Vec<TopArtist>,
    pub top_albums: Vec<TopAlbum>,
    /// Every genre heard, most first.
    pub genres: Vec<GenreShare>,
    /// Seconds heard in each hour of the day (local time).
    pub hours: Vec<f64>,
    pub decades: Vec<DecadeShare>,
}

fn clean(s: &str) -> &str {
    s.trim()
}

/// The genres in a tag, which may hold several ("Rock; Pop"). Never empty.
fn genres_of(t: &TrackMeta) -> Vec<String> {
    let v: Vec<String> = t
        .genre
        .split([';', '\0'])
        .map(|g| g.trim())
        .filter(|g| !g.is_empty())
        .map(str::to_owned)
        .collect();
    if v.is_empty() {
        vec!["Unknown".to_owned()]
    } else {
        v
    }
}

fn by_secs_then_plays<T>(items: &mut [(f64, u32, T)]) {
    items.sort_by(|a, b| b.0.total_cmp(&a.0).then(b.1.cmp(&a.1)));
}

/// A report for listening between `from` and `to` (seconds since the epoch, `to` excluded).
/// `tz_offset_min` is how far the person's clock is ahead of UTC, for the hours of the day.
pub fn report(history: &History, lib: &HashMap<&Path, &TrackMeta>, from: u64, to: u64, tz_offset_min: i32) -> Report {
    let total = history.total(from, to);
    let per = history.per_track(from, to);
    let first = history.first_heard();
    let mut out = Report { secs: total.secs, plays: total.plays, hours: history.by_hour(from, to, tz_offset_min).to_vec(), ..Default::default() };
    let mut artists: HashMap<String, (f64, u32, HashSet<&Path>, (f64, &Path))> = HashMap::new();
    let mut albums: HashMap<(String, String), (f64, u32, (f64, &Path))> = HashMap::new();
    let mut genres: HashMap<String, (String, f64)> = HashMap::new();
    let mut decades: HashMap<u16, f64> = HashMap::new();
    let mut tracks: Vec<(f64, u32, &TrackMeta)> = Vec::new();

    for (path, heard) in &per {
        out.tracks_heard += 1;
        if first.get(path).is_some_and(|&t| t >= from && t < to) {
            out.new_tracks += 1;
        }
        // Songs that have left the library still count in the totals, but can't be named.
        let Some(t) = lib.get(path).copied() else { continue };
        tracks.push((heard.secs, heard.plays, t));
        let artist = clean(&t.artist);
        if !artist.is_empty() {
            let e = artists.entry(artist.to_lowercase()).or_insert_with(|| (0.0, 0, HashSet::new(), (-1.0, *path)));
            e.0 += heard.secs;
            e.1 += heard.plays;
            e.2.insert(*path);
            if heard.secs > e.3 .0 {
                e.3 = (heard.secs, *path);
            }
        }
        let album = clean(&t.album);
        if !album.is_empty() {
            let who = if clean(&t.album_artist).is_empty() { clean(&t.artist) } else { clean(&t.album_artist) };
            let e = albums.entry((album.to_lowercase(), who.to_lowercase())).or_insert((0.0, 0, (-1.0, *path)));
            e.0 += heard.secs;
            e.1 += heard.plays;
            if heard.secs > e.2 .0 {
                e.2 = (heard.secs, *path);
            }
        }
        let gs = genres_of(t);
        for g in &gs {
            genres.entry(g.to_lowercase()).or_insert_with(|| (g.clone(), 0.0)).1 += heard.secs / gs.len() as f64;
        }
        if let Some(y) = t.year.filter(|y| (1900..2100).contains(y)) {
            *decades.entry(y / 10 * 10).or_default() += heard.secs;
        }
    }

    out.artists_heard = artists.len() as u32;
    tracks.sort_by(|a, b| b.1.cmp(&a.1).then(b.0.total_cmp(&a.0)).then(a.2.path.cmp(&b.2.path)));
    out.top_tracks = tracks
        .iter()
        .take(TOP)
        .map(|(secs, plays, t)| TopTrack { path: t.path.clone(), title: t.title.clone(), artist: t.artist.clone(), album: t.album.clone(), secs: *secs, plays: *plays })
        .collect();

    // The display name is the one a track used; look it up from a track by that artist.
    let name_of = |key: &str, path: &Path| lib.get(path).map(|t| clean(&t.artist).to_owned()).filter(|n| n.to_lowercase() == key).unwrap_or_else(|| key.to_owned());
    let mut list: Vec<(f64, u32, (String, usize, PathBuf))> =
        artists.iter().map(|(k, (s, p, set, best))| (*s, *p, (name_of(k, best.1), set.len(), best.1.to_path_buf()))).collect();
    by_secs_then_plays(&mut list);
    out.top_artists = list.into_iter().take(TOP).map(|(secs, plays, (name, n, path))| TopArtist { name, secs, plays, tracks: n as u32, path }).collect();

    let mut list: Vec<(f64, u32, (String, String, PathBuf))> = albums
        .iter()
        .map(|((_, _), (s, p, best))| {
            let t = lib[best.1];
            let who = if clean(&t.album_artist).is_empty() { clean(&t.artist) } else { clean(&t.album_artist) };
            (*s, *p, (clean(&t.album).to_owned(), who.to_owned(), best.1.to_path_buf()))
        })
        .collect();
    by_secs_then_plays(&mut list);
    out.top_albums = list.into_iter().take(TOP).map(|(secs, plays, (album, artist, path))| TopAlbum { album, artist, secs, plays, path }).collect();

    let mut g: Vec<GenreShare> = genres.into_values().map(|(name, secs)| GenreShare { name, secs }).collect();
    g.sort_by(|a, b| b.secs.total_cmp(&a.secs).then(a.name.cmp(&b.name)));
    out.genres = g;
    let mut d: Vec<DecadeShare> = decades.into_iter().map(|(decade, secs)| DecadeShare { decade, secs }).collect();
    d.sort_by_key(|x| x.decade);
    out.decades = d;
    out
}

// ---------------------------------------------------------------------------------------------
// Mixes
// ---------------------------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Mix {
    pub id: String,
    /// "repeat", "rediscover", "genre", "artist", "fresh" or "favourites": the UI picks a colour from it.
    pub kind: &'static str,
    pub name: String,
    pub blurb: String,
    pub tracks: Vec<PathBuf>,
}

/// A small deterministic generator, so a mix keeps its order through a day and changes the next.
struct Rng(u64);
impl Rng {
    fn new(seed: u64, salt: &str) -> Self {
        let mut h = seed ^ 0x9E37_79B9_7F4A_7C15;
        for b in salt.bytes() {
            h = (h ^ b as u64).wrapping_mul(0x0100_0000_01B3);
        }
        Rng(h)
    }
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    fn shuffle<T>(&mut self, v: &mut [T]) {
        for i in (1..v.len()).rev() {
            v.swap(i, (self.next() % (i as u64 + 1)) as usize);
        }
    }
}

fn slug(s: &str) -> String {
    let v: String = s.to_lowercase().chars().map(|c| if c.is_alphanumeric() { c } else { '-' }).collect();
    v.split('-').filter(|p| !p.is_empty()).collect::<Vec<_>>().join("-")
}

/// Keeps at most `per_artist` songs by one artist, in the order given.
fn spread<'a>(tracks: impl IntoIterator<Item = &'a TrackMeta>, per_artist: usize, limit: usize) -> Vec<&'a TrackMeta> {
    let mut seen: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::new();
    for t in tracks {
        let n = seen.entry(clean(&t.artist).to_lowercase()).or_default();
        if *n < per_artist {
            *n += 1;
            out.push(t);
            if out.len() >= limit {
                break;
            }
        }
    }
    out
}

pub struct MixInput<'a> {
    pub history: &'a History,
    /// The whole library.
    pub lib: &'a [&'a TrackMeta],
    /// Play counts and last-played times kept before (and alongside) the history file.
    pub stats: &'a HashMap<PathBuf, TrackStat>,
    pub favourites: &'a HashSet<PathBuf>,
    pub now: u64,
}

/// Mixes made from what you play. Only mixes with enough songs are returned; a new library with no
/// history gets at most a favourites mix.
pub fn mixes(input: &MixInput) -> Vec<Mix> {
    let MixInput { history, lib, stats, favourites, now } = *input;
    let seed = now / DAY;
    let known: HashMap<&Path, &TrackMeta> = lib.iter().map(|t| (t.path.as_path(), *t)).collect();
    let mut out = Vec::new();
    let plays_of = |p: &Path| stats.get(p).map_or(0, |s| s.plays);
    let heard_before = history.last_heard();
    let last_of = |p: &Path| stats.get(p).map_or(0, |s| s.last_played).max(heard_before.get(p).copied().unwrap_or(0));

    // --- On repeat: what you've played most in the last month.
    let recent = history.per_track(now.saturating_sub(30 * DAY), now + 1);
    let mut rep: Vec<(&TrackMeta, Heard)> = recent.iter().filter_map(|(p, h)| known.get(p).map(|t| (*t, *h))).filter(|(_, h)| h.plays > 0 || h.secs >= 60.0).collect();
    rep.sort_by(|a, b| b.1.plays.cmp(&a.1.plays).then(b.1.secs.total_cmp(&a.1.secs)).then(a.0.path.cmp(&b.0.path)));
    rep.truncate(MIX_MAX);
    if rep.len() >= MIX_MIN {
        let mut v: Vec<&TrackMeta> = rep.iter().map(|x| x.0).collect();
        Rng::new(seed, "repeat").shuffle(&mut v);
        out.push(Mix { id: "repeat".into(), kind: "repeat", name: "On repeat".into(), blurb: "What you've played most in the last month.".into(), tracks: v.iter().map(|t| t.path.clone()).collect() });
    }

    // --- What you listen to, over the last three months (all the history if that's empty).
    let mut window = history.per_track(now.saturating_sub(90 * DAY), now + 1);
    if window.is_empty() {
        window = history.per_track(0, now + 1);
    }
    let mut genre_secs: HashMap<String, (String, f64)> = HashMap::new();
    let mut artist_secs: HashMap<String, (String, f64)> = HashMap::new();
    for (p, h) in &window {
        let Some(t) = known.get(p) else { continue };
        let w = h.secs.max(1.0) + h.plays as f64 * 30.0;
        let gs = genres_of(t);
        for g in &gs {
            genre_secs.entry(g.to_lowercase()).or_insert_with(|| (g.clone(), 0.0)).1 += w / gs.len() as f64;
        }
        let a = clean(&t.artist);
        if !a.is_empty() {
            artist_secs.entry(a.to_lowercase()).or_insert_with(|| (a.to_owned(), 0.0)).1 += w;
        }
    }
    let rank = |m: HashMap<String, (String, f64)>| {
        let mut v: Vec<(String, f64, String)> = m.into_iter().map(|(k, (n, s))| (n, s, k)).collect();
        v.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
        v
    };
    let top_genres = rank(genre_secs);
    let top_artists = rank(artist_secs);
    let usable_genres: Vec<&(String, f64, String)> = top_genres.iter().filter(|g| g.2 != "unknown").collect();

    // How much each song has been heard, to put the familiar ones first.
    let all_time = history.per_track(0, u64::MAX);
    let familiarity = |p: &Path| plays_of(p) + all_time.get(p).map_or(0, |h| h.plays);

    // --- Rediscover: songs you loved and haven't heard for a while.
    let cutoff = now.saturating_sub(45 * DAY);
    let mut old: Vec<&TrackMeta> = lib
        .iter()
        .copied()
        .filter(|t| (familiarity(&t.path) >= 2 || favourites.contains(&t.path)) && (1..cutoff).contains(&last_of(&t.path)))
        .collect();
    old.sort_by(|a, b| familiarity(&b.path).cmp(&familiarity(&a.path)).then(a.path.cmp(&b.path)));
    old.truncate(MIX_MAX + 20);
    if old.len() >= MIX_MIN {
        Rng::new(seed, "rediscover").shuffle(&mut old);
        let v = spread(old.iter().copied(), 3, MIX_MAX);
        out.push(Mix { id: "rediscover".into(), kind: "rediscover", name: "Take me back".into(), blurb: "Songs you've loved, that you haven't played for a while.".into(), tracks: v.iter().map(|t| t.path.clone()).collect() });
    }

    // --- A mix for each of your top two genres: what you know, with songs you haven't heard.
    for (name, _, key) in usable_genres.iter().take(2) {
        let mut of: Vec<&TrackMeta> = lib.iter().copied().filter(|t| genres_of(t).iter().any(|g| g.to_lowercase() == *key)).collect();
        of.sort_by(|a, b| familiarity(&b.path).cmp(&familiarity(&a.path)).then(a.path.cmp(&b.path)));
        if of.len() < MIX_MIN {
            continue;
        }
        let (fam, fresh): (Vec<&TrackMeta>, Vec<&TrackMeta>) = of.into_iter().partition(|t| familiarity(&t.path) > 0);
        let mut pool: Vec<&TrackMeta> = fam.into_iter().take(MIX_MAX / 2 + 5).chain(fresh.into_iter().take(MIX_MAX / 2)).collect();
        Rng::new(seed, key).shuffle(&mut pool);
        let v = spread(pool.iter().copied(), 4, MIX_MAX);
        if v.len() >= MIX_MIN {
            out.push(Mix { id: format!("genre-{}", slug(key)), kind: "genre", name: format!("{name} mix"), blurb: format!("The {name} you play, with a few you haven't heard yet."), tracks: v.iter().map(|t| t.path.clone()).collect() });
        }
    }

    // --- A mix for each of your top two artists: their songs, with others from their world.
    for (name, _, key) in top_artists.iter().take(2) {
        let own: Vec<&TrackMeta> = {
            let mut o: Vec<&TrackMeta> = lib.iter().copied().filter(|t| clean(&t.artist).to_lowercase() == *key).collect();
            o.sort_by(|a, b| familiarity(&b.path).cmp(&familiarity(&a.path)).then(a.path.cmp(&b.path)));
            o
        };
        // Their genre: the one most of their songs share.
        let mut gc: HashMap<String, usize> = HashMap::new();
        for t in &own {
            for g in genres_of(t) {
                *gc.entry(g.to_lowercase()).or_default() += 1;
            }
        }
        let their = gc.into_iter().filter(|(g, _)| g != "unknown").max_by(|a, b| a.1.cmp(&b.1).then(b.0.cmp(&a.0))).map(|(g, _)| g);
        let mut others: Vec<&TrackMeta> = match &their {
            Some(g) => lib.iter().copied().filter(|t| clean(&t.artist).to_lowercase() != *key && genres_of(t).iter().any(|x| x.to_lowercase() == *g)).collect(),
            None => Vec::new(),
        };
        others.sort_by(|a, b| familiarity(&b.path).cmp(&familiarity(&a.path)).then(a.path.cmp(&b.path)));
        others.truncate(MIX_MAX);
        let mut rng = Rng::new(seed, key);
        let mut own = own;
        own.truncate(MIX_MAX);
        rng.shuffle(&mut own);
        rng.shuffle(&mut others);
        // Two of theirs, then one from someone else.
        let mut mixed: Vec<&TrackMeta> = Vec::new();
        let (mut a, mut b) = (own.iter(), others.iter());
        loop {
            let before = mixed.len();
            mixed.extend(a.by_ref().take(2).copied());
            mixed.extend(b.by_ref().take(1).copied());
            if mixed.len() == before || mixed.len() >= MIX_MAX {
                break;
            }
        }
        mixed.truncate(MIX_MAX);
        if mixed.len() >= MIX_MIN {
            out.push(Mix { id: format!("artist-{}", slug(key)), kind: "artist", name: format!("{name} & more"), blurb: format!("{name}, with songs from the same world."), tracks: mixed.iter().map(|t| t.path.clone()).collect() });
        }
    }

    // --- Fresh picks: nothing you've played yet, in the genres and by the artists you like.
    if !window.is_empty() {
        let liked_genres: HashSet<&str> = usable_genres.iter().take(3).map(|g| g.2.as_str()).collect();
        let liked_artists: HashSet<&str> = top_artists.iter().take(5).map(|a| a.2.as_str()).collect();
        let mut fresh: Vec<&TrackMeta> = lib
            .iter()
            .copied()
            .filter(|t| familiarity(&t.path) == 0 && last_of(&t.path) == 0)
            .filter(|t| liked_artists.contains(clean(&t.artist).to_lowercase().as_str()) || genres_of(t).iter().any(|g| liked_genres.contains(g.to_lowercase().as_str())))
            .collect();
        fresh.sort_by(|a, b| a.path.cmp(&b.path));
        Rng::new(seed, "fresh").shuffle(&mut fresh);
        let v = spread(fresh.iter().copied(), 3, MIX_MAX);
        if v.len() >= MIX_MIN {
            out.push(Mix { id: "fresh".into(), kind: "fresh", name: "Fresh picks".into(), blurb: "Songs in your library you haven't played yet, picked from what you like.".into(), tracks: v.iter().map(|t| t.path.clone()).collect() });
        }
    }

    // --- Favourites, shuffled.
    let mut fav: Vec<&TrackMeta> = lib.iter().copied().filter(|t| favourites.contains(&t.path)).collect();
    fav.sort_by(|a, b| a.path.cmp(&b.path));
    if fav.len() >= MIX_MIN {
        Rng::new(seed, "favourites").shuffle(&mut fav);
        let v = spread(fav.iter().copied(), 4, MIX_MAX);
        out.push(Mix { id: "favourites".into(), kind: "favourites", name: "Your favourites".into(), blurb: "Songs you've hearted, in a fresh order.".into(), tracks: v.iter().map(|t| t.path.clone()).collect() });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: u64 = 20_000 * DAY;

    fn t(path: &str, title: &str, artist: &str, album: &str, genre: &str, year: u16) -> TrackMeta {
        TrackMeta { path: PathBuf::from(path), title: title.into(), artist: artist.into(), album: album.into(), album_artist: artist.into(), genre: genre.into(), year: Some(year), ..Default::default() }
    }
    fn library() -> Vec<TrackMeta> {
        let mut v = Vec::new();
        for i in 0..14 {
            v.push(t(&format!("/rock/{i}.flac"), &format!("Rock {i}"), if i < 8 { "The Amps" } else { "Loud Folk" }, if i < 8 { "Volume" } else { "Stack" }, "Rock", 1995));
        }
        for i in 0..10 {
            v.push(t(&format!("/jazz/{i}.flac"), &format!("Jazz {i}"), "Blue Room", "Late", "Jazz; Blues", 1962));
        }
        v.push(t("/misc/x.flac", "No tags", "", "", "", 0));
        v
    }
    fn refs(v: &[TrackMeta]) -> Vec<&TrackMeta> {
        v.iter().collect()
    }

    fn history_with(f: impl FnOnce(&mut History)) -> History {
        let d = tempfile::tempdir().unwrap();
        let mut h = History::open(d.path().join("h.json")).unwrap();
        f(&mut h);
        h
    }

    #[test]
    fn a_report_names_the_tops_and_shares() {
        let lib = library();
        let by: HashMap<&Path, &TrackMeta> = lib.iter().map(|t| (t.path.as_path(), t)).collect();
        let h = history_with(|h| {
            // Rock 0 played three times, Rock 1 once, a jazz track twice, today.
            for _ in 0..3 {
                h.add(Path::new("/rock/0.flac"), NOW - 3600, 200.0, 1);
            }
            h.add(Path::new("/rock/1.flac"), NOW - 3000, 200.0, 1);
            h.add(Path::new("/jazz/0.flac"), NOW - 2000, 300.0, 1);
            h.add(Path::new("/jazz/0.flac"), NOW - 1000, 300.0, 1);
            h.add(Path::new("/gone/removed.flac"), NOW - 500, 100.0, 1);
            // Last month: another song, not in this week's range.
            h.add(Path::new("/rock/9.flac"), NOW - 20 * DAY, 100.0, 1);
        });
        let r = report(&h, &by, NOW - DAY, NOW, 0);
        assert_eq!(r.plays, 7);
        assert!((r.secs - 1500.0).abs() < 0.5);
        assert_eq!(r.tracks_heard, 4, "the removed song still counts as heard");
        assert_eq!(r.new_tracks, 4);
        assert_eq!(r.top_tracks[0].title, "Rock 0");
        assert_eq!(r.top_tracks[0].plays, 3);
        assert_eq!(r.top_artists[0].name, "The Amps", "800 s beats 600 s");
        assert_eq!(r.top_artists[0].tracks, 2);
    }

    #[test]
    fn artists_and_albums_are_ranked_by_time_and_genres_are_split() {
        let lib = library();
        let by: HashMap<&Path, &TrackMeta> = lib.iter().map(|t| (t.path.as_path(), t)).collect();
        let h = history_with(|h| {
            h.add(Path::new("/rock/0.flac"), NOW - 100, 100.0, 1);
            h.add(Path::new("/rock/1.flac"), NOW - 100, 100.0, 1);
            h.add(Path::new("/jazz/0.flac"), NOW - 100, 400.0, 1);
            h.add(Path::new("/misc/x.flac"), NOW - 100, 50.0, 0);
        });
        let r = report(&h, &by, 0, NOW, 0);
        assert_eq!(r.top_artists[0].name, "Blue Room");
        assert_eq!(r.top_artists[1].name, "The Amps");
        assert_eq!(r.top_artists[1].tracks, 2);
        assert_eq!(r.top_artists.len(), 2, "a song with no artist isn't an artist");
        assert_eq!(r.top_albums[0].album, "Late");
        assert_eq!(r.top_albums[0].artist, "Blue Room");
        // "Jazz; Blues" shares the 400 s between two genres.
        let g = |n: &str| r.genres.iter().find(|g| g.name == n).map(|g| g.secs);
        assert_eq!(g("Jazz"), Some(200.0));
        assert_eq!(g("Blues"), Some(200.0));
        assert_eq!(g("Rock"), Some(200.0));
        assert_eq!(g("Unknown"), Some(50.0));
        assert_eq!(r.decades.iter().map(|d| d.decade).collect::<Vec<_>>(), vec![1960, 1990]);
        assert_eq!(r.hours.len(), 24);
    }

    #[test]
    fn an_empty_range_is_an_empty_report() {
        let lib = library();
        let by: HashMap<&Path, &TrackMeta> = lib.iter().map(|t| (t.path.as_path(), t)).collect();
        let h = history_with(|_| {});
        assert_eq!(report(&h, &by, 0, NOW, 0).top_tracks.len(), 0);
        assert_eq!(report(&h, &by, 0, NOW, 0).secs, 0.0);
    }

    fn input<'a>(h: &'a History, lib: &'a [&'a TrackMeta], stats: &'a HashMap<PathBuf, TrackStat>, favs: &'a HashSet<PathBuf>) -> MixInput<'a> {
        MixInput { history: h, lib, stats, favourites: favs, now: NOW }
    }

    #[test]
    fn no_history_means_no_mixes_except_favourites() {
        let lib = library();
        let all = refs(&lib);
        let h = history_with(|_| {});
        let (stats, mut favs) = (HashMap::new(), HashSet::new());
        assert!(mixes(&input(&h, &all, &stats, &favs)).is_empty());
        for t in lib.iter().take(8) {
            favs.insert(t.path.clone());
        }
        let m = mixes(&input(&h, &all, &stats, &favs));
        assert_eq!(m.len(), 1);
        assert_eq!(m[0].id, "favourites");
        assert!(m[0].tracks.iter().all(|p| favs.contains(p)));
    }

    #[test]
    fn mixes_follow_what_was_played() {
        let lib = library();
        let all = refs(&lib);
        let h = history_with(|h| {
            // Last week: lots of rock by The Amps.
            for i in 0..8 {
                for _ in 0..3 {
                    h.add(Path::new(&format!("/rock/{i}.flac")), NOW - 2 * DAY, 200.0, 1);
                }
            }
        });
        let mut stats = HashMap::new();
        // Old jazz favourites, not heard for ages.
        for i in 0..8 {
            stats.insert(PathBuf::from(format!("/jazz/{i}.flac")), TrackStat { plays: 5, last_played: NOW - 200 * DAY });
        }
        let favs = HashSet::new();
        let m = mixes(&input(&h, &all, &stats, &favs));
        let ids: Vec<&str> = m.iter().map(|m| m.id.as_str()).collect();
        assert!(ids.contains(&"repeat"), "{ids:?}");
        assert!(ids.contains(&"rediscover"), "{ids:?}");
        assert!(ids.contains(&"genre-rock"), "{ids:?}");
        assert!(ids.contains(&"artist-the-amps"), "{ids:?}");
        let get = |id: &str| m.iter().find(|m| m.id == id).unwrap();
        assert!(get("repeat").tracks.iter().all(|p| p.starts_with("/rock/") && !p.ends_with("8.flac")));
        assert!(get("rediscover").tracks.iter().all(|p| p.starts_with("/jazz/")), "only what hasn't been heard lately");
        assert!(get("genre-rock").tracks.len() >= MIX_MIN);
        // The artist mix is mostly their songs, with others from the same genre mixed in.
        let amps = get("artist-the-amps");
        assert!(amps.tracks.iter().filter(|p| p.to_string_lossy().ends_with(|c: char| c.is_ascii_digit() || c == 'c')).count() > 0);
        assert!(amps.tracks.iter().any(|p| ["8", "9", "10", "11", "12", "13"].iter().any(|n| p.ends_with(&format!("{n}.flac")))), "someone else from the same genre");
        // Nobody in a mix twice, and nothing outside the library.
        for mix in &m {
            let set: HashSet<&PathBuf> = mix.tracks.iter().collect();
            assert_eq!(set.len(), mix.tracks.len(), "{}", mix.id);
            assert!(mix.tracks.len() <= MIX_MAX);
            assert!(mix.tracks.iter().all(|p| lib.iter().any(|t| &t.path == p)));
        }
        // Fresh picks hold only songs never played.
        if let Some(fresh) = m.iter().find(|m| m.id == "fresh") {
            assert!(fresh.tracks.iter().all(|p| !p.starts_with("/rock/") || ["8", "9", "10", "11", "12", "13"].iter().any(|n| p.ends_with(&format!("{n}.flac")))));
        }
    }

    #[test]
    fn a_mix_keeps_its_order_through_the_day_and_changes_the_next_day() {
        let lib = library();
        let all = refs(&lib);
        let h = history_with(|h| {
            for i in 0..14 {
                h.add(Path::new(&format!("/rock/{i}.flac")), NOW - DAY, 120.0, 1);
            }
        });
        let (stats, favs) = (HashMap::new(), HashSet::new());
        let a = mixes(&input(&h, &all, &stats, &favs));
        let b = mixes(&MixInput { now: NOW + 3600, ..input(&h, &all, &stats, &favs) });
        let c = mixes(&MixInput { now: NOW + 2 * DAY, ..input(&h, &all, &stats, &favs) });
        let order = |v: &Vec<Mix>| v.iter().find(|m| m.id == "repeat").unwrap().tracks.clone();
        assert_eq!(order(&a), order(&b));
        assert_ne!(order(&a), order(&c));
    }

    #[test]
    fn songs_with_the_same_artist_are_capped_in_a_mix() {
        let lib = library();
        let all = refs(&lib);
        let h = history_with(|h| {
            for t in lib.iter().filter(|t| t.genre == "Rock") {
                h.add(&t.path, NOW - DAY, 100.0, 1);
            }
        });
        let (stats, favs) = (HashMap::new(), HashSet::new());
        let m = mixes(&input(&h, &all, &stats, &favs));
        let g = m.iter().find(|m| m.id == "genre-rock").unwrap();
        let amps = g.tracks.iter().filter(|p| lib.iter().find(|t| &t.path == *p).unwrap().artist == "The Amps").count();
        assert!(amps <= 4);
    }
}
