//! Turns a playlist from another service into a local one: reads the list (from a link, or from
//! pasted text or a CSV export), then finds each song in the library.
//!
//! Redshift plays files, so a Spotify or YouTube playlist can only become a playlist of the files
//! you already have. Songs are matched by title and artist (and length, to tell a single from a live
//! cut); what isn't found is listed so it can be hunted down.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::Serialize;

use crate::meta::TrackMeta;
use crate::net::remote_playlist::{Entry, RemoteList, MAX_ENTRIES};

/// Two lengths this close are the same recording.
const SAME_LENGTH_SECS: u32 = 10;

fn loose(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

/// The song's name without the decoration that differs between releases: "(Remastered)",
/// "- Radio Edit", "feat. X".
fn base_title(s: &str) -> String {
    let mut plain = String::new();
    let mut depth = 0u32;
    for c in s.chars() {
        match c {
            '(' | '[' => depth += 1,
            ')' | ']' => depth = depth.saturating_sub(1),
            c if depth == 0 => plain.push(c),
            _ => {}
        }
    }
    let low = plain.to_lowercase();
    let mut cut = low.len();
    for m in [" - ", " – ", " feat", " ft.", " ft ", " featuring", " with "] {
        if let Some(i) = low.find(m) {
            cut = cut.min(i);
        }
    }
    let k = loose(&low[..cut]);
    if k.is_empty() { loose(s) } else { k }
}

/// Artists of a song, one name each: "A, B & C feat. D" is four.
fn artists(s: &str) -> Vec<String> {
    let low = s.to_lowercase();
    let mut parts = vec![low];
    for sep in [",", ";", "&", "/", " feat.", " feat ", " ft.", " featuring ", " and ", " x ", " + ", " with "] {
        parts = parts.into_iter().flat_map(|p| p.split(sep).map(str::to_string).collect::<Vec<_>>()).collect();
    }
    parts.iter().map(|p| loose(p)).filter(|p| !p.is_empty()).collect()
}

fn same_artist(a: &[String], b: &[String]) -> bool {
    a.iter().any(|x| b.iter().any(|y| x == y || (x.len() >= 5 && y.len() >= 5 && (x.contains(y.as_str()) || y.contains(x.as_str())))))
}

/// The library, indexed for matching.
pub struct Matcher {
    by_title: HashMap<String, Vec<(Vec<String>, f64, PathBuf)>>,
}

impl Matcher {
    pub fn new<'a>(tracks: impl IntoIterator<Item = &'a TrackMeta>) -> Self {
        let mut by_title: HashMap<String, Vec<(Vec<String>, f64, PathBuf)>> = HashMap::new();
        for t in tracks {
            if t.title.trim().is_empty() {
                continue;
            }
            let mut who = artists(&t.artist);
            who.extend(artists(&t.album_artist));
            by_title.entry(base_title(&t.title)).or_default().push((who, t.duration_secs, t.path.clone()));
        }
        for v in by_title.values_mut() {
            v.sort_by(|a, b| a.2.cmp(&b.2));
        }
        Matcher { by_title }
    }

    /// The library file for this song, or `None` when there isn't one that is certainly it.
    pub fn find(&self, e: &Entry) -> Option<PathBuf> {
        let cands = self.by_title.get(&base_title(&e.title))?;
        let want = artists(&e.artist);
        let ok: Vec<&(Vec<String>, f64, PathBuf)> = if want.is_empty() {
            // no artist given: only when the title alone is unambiguous
            if cands.len() == 1 { cands.iter().collect() } else { Vec::new() }
        } else {
            cands.iter().filter(|c| same_artist(&want, &c.0)).collect()
        };
        let gap = |c: &&(Vec<String>, f64, PathBuf)| -> u32 {
            if e.duration_secs == 0 || c.1 <= 0.0 { SAME_LENGTH_SECS } else { (c.1 - f64::from(e.duration_secs)).abs().round() as u32 }
        };
        // the closest in length first; a different length is still the same song, just another cut
        ok.into_iter().min_by_key(|c| gap(c)).map(|c| c.2.clone())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Row {
    pub title: String,
    pub artist: String,
    pub path: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Preview {
    pub source: String,
    pub name: String,
    pub total: usize,
    pub matched: usize,
    /// The service listed only part of a longer playlist.
    pub truncated: bool,
    pub rows: Vec<Row>,
}

pub fn preview(list: RemoteList, matcher: &Matcher) -> Preview {
    let rows: Vec<Row> = list.entries.into_iter().take(MAX_ENTRIES).map(|e| Row { path: matcher.find(&e), title: e.title, artist: e.artist }).collect();
    Preview { source: list.source, name: list.name, total: rows.len(), matched: rows.iter().filter(|r| r.path.is_some()).count(), truncated: list.truncated, rows }
}

// ---- pasted text ----

fn csv_line(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut cur = String::new();
    let mut quoted = false;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                cur.push('"');
            }
            (_, '"') => quoted = !quoted,
            (false, ',') => out.push(std::mem::take(&mut cur).trim().to_string()),
            (_, c) => cur.push(c),
        }
    }
    out.push(cur.trim().to_string());
    out
}

fn strip_number(line: &str) -> &str {
    let t = line.trim_start();
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    if digits > 0 && digits <= 4 {
        let rest = &t[digits..];
        if let Some(r) = rest.strip_prefix(". ").or_else(|| rest.strip_prefix(") ")).or_else(|| rest.strip_prefix(" - ")) {
            return r.trim_start();
        }
    }
    t
}

/// Songs from pasted text: one "Artist - Title" per line (a leading "1." is ignored), or a CSV with
/// a header that names the title and artist columns, such as the files Exportify makes for Spotify.
pub fn parse_text(text: &str) -> RemoteList {
    let mut out = RemoteList { source: "Pasted list".into(), name: "Imported playlist".into(), ..Default::default() };
    let lines: Vec<&str> = text.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
    if let Some(head) = lines.first().filter(|l| l.contains(',')).map(|l| csv_line(l)) {
        let col = |names: &[&str]| head.iter().position(|h| names.contains(&h.trim_start_matches('\u{feff}').to_lowercase().as_str()));
        let title = col(&["track name", "title", "song", "song name", "track", "name"]);
        let artist = col(&["artist name(s)", "artist name", "artist", "artists", "artist(s)"]);
        let dur = col(&["duration (ms)", "duration_ms", "duration ms"]);
        if let (Some(ti), Some(ai)) = (title, artist) {
            for line in &lines[1..] {
                let f = csv_line(line);
                let Some(t) = f.get(ti).filter(|t| !t.is_empty()) else { continue };
                let secs = dur.and_then(|d| f.get(d)).and_then(|d| d.parse::<u64>().ok()).map_or(0, |ms| (ms / 1000) as u32);
                out.entries.push(Entry { title: t.clone(), artist: f.get(ai).cloned().unwrap_or_default().replace(';', ", "), duration_secs: secs });
            }
            out.entries.truncate(MAX_ENTRIES);
            return out;
        }
    }
    for line in lines {
        let line = strip_number(line);
        if line.starts_with('#') {
            continue;
        }
        let split = [" - ", " – ", " — "].iter().find_map(|s| line.split_once(s));
        out.entries.push(match split {
            Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => Entry { title: t.trim().into(), artist: a.trim().into(), duration_secs: 0 },
            _ => Entry { title: line.into(), ..Default::default() },
        });
        if out.entries.len() >= MAX_ENTRIES {
            out.truncated = true;
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, artist: &str, title: &str, secs: f64) -> TrackMeta {
        TrackMeta { path: PathBuf::from(path), artist: artist.into(), album_artist: artist.into(), title: title.into(), duration_secs: secs, ..Default::default() }
    }

    fn lib() -> Matcher {
        Matcher::new(&[
            track("/m/a/carrier.flac", "Paper Satellites", "Carrier Wave", 215.0),
            track("/m/a/carrier-live.flac", "Paper Satellites", "Carrier Wave", 330.0),
            track("/m/b/low.mp3", "Paper Satellites", "Low Orbit (Remastered 2011)", 180.0),
            track("/m/c/duet.opus", "Ana & Ben", "Two Of Us", 200.0),
            track("/m/d/unknown.mp3", "", "Mystery", 100.0),
        ])
    }

    fn e(artist: &str, title: &str, secs: u32) -> Entry {
        Entry { artist: artist.into(), title: title.into(), duration_secs: secs }
    }

    #[test]
    fn title_and_artist_match_and_length_picks_the_right_cut() {
        let m = lib();
        assert_eq!(m.find(&e("Paper Satellites", "Carrier Wave", 214)), Some("/m/a/carrier.flac".into()));
        assert_eq!(m.find(&e("Paper Satellites", "Carrier Wave", 331)), Some("/m/a/carrier-live.flac".into()));
        // length unknown: the first in path order
        assert!(m.find(&e("Paper Satellites", "Carrier Wave", 0)).is_some());
    }

    #[test]
    fn release_decoration_and_featured_artists_are_ignored() {
        let m = lib();
        assert_eq!(m.find(&e("Paper Satellites", "Low Orbit - 2011 Remaster", 181)), Some("/m/b/low.mp3".into()));
        assert_eq!(m.find(&e("Paper Satellites feat. Someone", "Low Orbit (feat. Someone)", 0)), Some("/m/b/low.mp3".into()));
        assert_eq!(m.find(&e("Ben", "Two of Us", 200)), Some("/m/c/duet.opus".into()));
        assert_eq!(m.find(&e("Ana, Ben", "Two of Us", 200)), Some("/m/c/duet.opus".into()));
    }

    #[test]
    fn the_wrong_artist_is_not_a_match() {
        let m = lib();
        assert_eq!(m.find(&e("Cover Band", "Carrier Wave", 215)), None);
        assert_eq!(m.find(&e("Paper Satellites", "Not In The Library", 0)), None);
    }

    #[test]
    fn without_an_artist_only_an_unambiguous_title_matches() {
        let m = lib();
        assert_eq!(m.find(&e("", "Mystery", 0)), Some("/m/d/unknown.mp3".into()));
        assert_eq!(m.find(&e("", "Carrier Wave", 0)), None);
    }

    #[test]
    fn preview_counts() {
        let p = preview(RemoteList { source: "Deezer".into(), name: "X".into(), entries: vec![e("Paper Satellites", "Carrier Wave", 215), e("Nobody", "Nothing", 0)], truncated: false }, &lib());
        assert_eq!((p.total, p.matched), (2, 1));
        assert_eq!(p.rows[1].path, None);
    }

    #[test]
    fn pasted_lines_are_split_into_artist_and_title() {
        let l = parse_text("1. Paper Satellites - Carrier Wave\n2) Ana & Ben – Two Of Us\n\n# comment\nJust A Title\n");
        assert_eq!(l.entries, vec![e("Paper Satellites", "Carrier Wave", 0), e("Ana & Ben", "Two Of Us", 0), e("", "Just A Title", 0)]);
    }

    #[test]
    fn an_exportify_csv_is_read() {
        let csv = "\u{feff}\"Track URI\",\"Track Name\",\"Album Name\",\"Artist Name(s)\",\"Release Date\",\"Duration (ms)\"\n\
                   \"spotify:track:1\",\"Carrier Wave\",\"Signal, Noise\",\"Paper Satellites;Guest\",\"2020\",\"215000\"\n\
                   \"spotify:track:2\",\"He said \"\"hi\"\"\",\"X\",\"Ana\",\"2021\",\"90500\"\n";
        let l = parse_text(csv);
        assert_eq!(l.entries.len(), 2);
        assert_eq!(l.entries[0], e("Paper Satellites, Guest", "Carrier Wave", 215));
        assert_eq!(l.entries[1], e("Ana", "He said \"hi\"", 90));
    }

    #[test]
    fn a_line_with_commas_but_no_header_is_still_a_song() {
        let l = parse_text("Ana, Ben - Hello, World");
        assert_eq!(l.entries, vec![e("Ana, Ben", "Hello, World", 0)]);
    }
}
