//! Lyrics: parsing LRC (plain, multi-stamp and enhanced word-timed) and finding them for a track.
//!
//! Supported syntax:
//! * `[mm:ss.xx]` line stamps, also `[mm:ss]`, `[mm:ss.xxx]`, `[mm:ss:xx]`, minutes above 59
//! * several stamps on one line, `[00:10.00][01:20.00]Chorus`, which repeats the text
//! * `[offset:+500]` (milliseconds; positive makes lyrics appear sooner) and the usual id tags
//!   `[ti:]`, `[ar:]`, `[al:]`, `[by:]`
//! * enhanced LRC word stamps, `[00:12.00]<00:12.00>Hel<00:12.30>lo <00:12.80>world`
//! * text with no stamps at all, which becomes unsynced lyrics
//!
//! The front end gets plain data (times in milliseconds) and finds the current line itself with
//! [`Lyrics::line_index_at`], so scrolling stays smooth without a round trip per frame.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::meta;

/// Sidecar and embedded lyrics larger than this are ignored; real lyrics are a few KB.
pub const MAX_LYRICS_BYTES: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricWord {
    pub time_ms: u64,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub time_ms: u64,
    pub text: String,
    /// Per-word timing for karaoke-style highlighting. Empty unless the file had word stamps.
    pub words: Vec<LyricWord>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LyricsSource {
    /// A `.lrc` / `.txt` file next to the audio file.
    Sidecar,
    /// Stored in the audio file's tags.
    Embedded,
    /// Parsed from a string handed in directly.
    Text,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Lyrics {
    /// True when lines carry timestamps; false for plain text (all `time_ms` are 0).
    pub synced: bool,
    pub lines: Vec<LyricLine>,
    pub title: Option<String>,
    pub artist: Option<String>,
    pub album: Option<String>,
    pub offset_ms: i64,
    pub source: LyricsSource,
}

impl Lyrics {
    /// Index of the line being sung at `time_ms`: the last line that starts at or before it.
    /// `None` before the first line starts, and always `None` for unsynced lyrics.
    pub fn line_index_at(&self, time_ms: u64) -> Option<usize> {
        if !self.synced {
            return None;
        }
        self.lines.partition_point(|l| l.time_ms <= time_ms).checked_sub(1)
    }

    /// Index of the word being sung within `line`, for enhanced lyrics.
    pub fn word_index_at(&self, line: usize, time_ms: u64) -> Option<usize> {
        let words = &self.lines.get(line)?.words;
        words.partition_point(|w| w.time_ms <= time_ms).checked_sub(1)
    }
}

/// Parses lyrics text. Never fails: text without timestamps becomes unsynced lyrics.
pub fn parse(text: &str, source: LyricsSource) -> Lyrics {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let mut out = Lyrics {
        synced: false,
        lines: Vec::new(),
        title: None,
        artist: None,
        album: None,
        offset_ms: 0,
        source,
    };
    let mut plain: Vec<String> = Vec::new();
    let mut timed: Vec<LyricLine> = Vec::new();

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() {
            plain.push(String::new());
            continue;
        }

        // Leading `[..]` groups: any number of time stamps, or a single id tag.
        let mut rest = line;
        let mut stamps = Vec::new();
        while let Some(inner_end) = rest.strip_prefix('[').and_then(|r| r.find(']')) {
            let inner = &rest[1..1 + inner_end];
            match parse_stamp(inner) {
                Some(t) => {
                    stamps.push(t);
                    rest = &rest[inner_end + 2..];
                }
                None => break,
            }
        }

        if stamps.is_empty() {
            if let Some((key, value)) = parse_id_tag(rest) {
                match key.as_str() {
                    "ti" => out.title = Some(value),
                    "ar" => out.artist = Some(value),
                    "al" => out.album = Some(value),
                    "offset" => out.offset_ms = value.trim().trim_start_matches('+').parse().unwrap_or(0),
                    _ => {}
                }
                continue;
            }
            // A line that starts with a word stamp is enhanced text without a line stamp.
            if rest.starts_with('<') {
                let words = parse_words(rest, None);
                if let Some(first) = words.first() {
                    timed.push(LyricLine { time_ms: first.time_ms, text: join_words(&words), words });
                    continue;
                }
            }
            plain.push(rest.to_string());
            continue;
        }

        let words = parse_words(rest, stamps.first().copied());
        let text = if words.is_empty() { strip_word_stamps(rest).trim().to_string() } else { join_words(&words) };
        // Word timing is dropped for lines that only have one (pointless) word stamp at the start.
        for &t in &stamps {
            let words = words.iter().map(|w| LyricWord { time_ms: w.time_ms, text: w.text.clone() }).collect();
            timed.push(LyricLine { time_ms: t, text: text.clone(), words });
        }
    }

    if timed.is_empty() {
        while plain.first().is_some_and(String::is_empty) {
            plain.remove(0);
        }
        while plain.last().is_some_and(String::is_empty) {
            plain.pop();
        }
        out.lines = plain.into_iter().map(|text| LyricLine { time_ms: 0, text, words: Vec::new() }).collect();
        return out;
    }

    // Offset: positive means the lyrics should show up sooner than their stamps say.
    let shift = |t: u64| (t as i64 - out.offset_ms).max(0) as u64;
    for line in &mut timed {
        line.time_ms = shift(line.time_ms);
        for w in &mut line.words {
            w.time_ms = shift(w.time_ms);
        }
    }
    timed.sort_by_key(|l| l.time_ms); // stable: translations on the same stamp keep their order
    out.synced = true;
    out.lines = timed;
    out
}

/// `01:23.45`, `1:23`, `01:23.456`, `01:23:45`, `100:05.0` -> milliseconds.
fn parse_stamp(s: &str) -> Option<u64> {
    let (min, rest) = s.split_once(':')?;
    if min.is_empty() || min.len() > 4 || !min.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let (sec, frac) = match rest.split_once(['.', ':']) {
        Some((sec, frac)) => (sec, Some(frac)),
        None => (rest, None),
    };
    if sec.is_empty() || sec.len() > 2 || !sec.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let ms = match frac {
        None => 0,
        Some(f) if !f.is_empty() && f.len() <= 3 && f.bytes().all(|b| b.is_ascii_digit()) => {
            // The digits are a decimal fraction of a second: ".5" is 500 ms, ".05" is 50 ms.
            let n: u64 = f.parse().ok()?;
            n * 10u64.pow(3 - f.len() as u32)
        }
        Some(_) => return None,
    };
    let (min, sec): (u64, u64) = (min.parse().ok()?, sec.parse().ok()?);
    if sec >= 60 {
        return None;
    }
    Some((min * 60 + sec) * 1000 + ms)
}

/// `ti:Song name` -> ("ti", "Song name"). Only alphabetic keys, so time stamps never match.
fn parse_id_tag(s: &str) -> Option<(String, String)> {
    let inner = s.strip_prefix('[')?.strip_suffix(']')?;
    let (key, value) = inner.split_once(':')?;
    if key.is_empty() || !key.bytes().all(|b| b.is_ascii_alphabetic()) {
        return None;
    }
    Some((key.to_ascii_lowercase(), value.trim().to_string()))
}

/// Splits `<t>word <t>word` text into timed words. Text before the first stamp is given
/// `leading_time` (the line stamp), or dropped into the first word's time if there isn't one.
fn parse_words(s: &str, leading_time: Option<u64>) -> Vec<LyricWord> {
    let mut words: Vec<LyricWord> = Vec::new();
    let mut cursor = 0;
    let mut current: Option<u64> = leading_time;
    let bytes = s.as_bytes();
    let mut i = 0;
    let mut saw_stamp = false;
    while i < bytes.len() {
        if bytes[i] == b'<' {
            if let Some(close) = s[i..].find('>') {
                if let Some(t) = parse_stamp(&s[i + 1..i + close]) {
                    saw_stamp = true;
                    push_word(&mut words, current, &s[cursor..i]);
                    current = Some(t);
                    cursor = i + close + 1;
                    i = cursor;
                    continue;
                }
            }
        }
        i += 1;
    }
    if !saw_stamp {
        return Vec::new();
    }
    push_word(&mut words, current, &s[cursor..]);
    words
}

fn push_word(words: &mut Vec<LyricWord>, time: Option<u64>, text: &str) {
    if text.is_empty() {
        return;
    }
    if let Some(time_ms) = time {
        words.push(LyricWord { time_ms, text: text.to_string() });
    }
}

fn join_words(words: &[LyricWord]) -> String {
    words.iter().map(|w| w.text.as_str()).collect::<String>().trim().to_string()
}

fn strip_word_stamps(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some(start) = rest.find('<') {
        match rest[start..].find('>') {
            Some(end) if parse_stamp(&rest[start + 1..start + end]).is_some() => {
                out.push_str(&rest[..start]);
                rest = &rest[start + end + 1..];
            }
            _ => {
                out.push_str(&rest[..=start]);
                rest = &rest[start + 1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Turns the bytes of a lyrics file into text: UTF-8 (with or without BOM) or UTF-16 with BOM.
/// Anything else is decoded as UTF-8 with replacement characters rather than rejected.
pub fn decode_text(bytes: &[u8]) -> String {
    match bytes {
        [0xFF, 0xFE, rest @ ..] => decode_utf16(rest, u16::from_le_bytes),
        [0xFE, 0xFF, rest @ ..] => decode_utf16(rest, u16::from_be_bytes),
        [0xEF, 0xBB, 0xBF, rest @ ..] => String::from_utf8_lossy(rest).into_owned(),
        _ => String::from_utf8_lossy(bytes).into_owned(),
    }
}

fn decode_utf16(bytes: &[u8], f: fn([u8; 2]) -> u16) -> String {
    let units: Vec<u16> = bytes.chunks_exact(2).map(|c| f([c[0], c[1]])).collect();
    String::from_utf16_lossy(&units)
}

/// The `.lrc` (preferred) or `.txt` file with the same name as `audio`, if there is one. The
/// extension match ignores case, since `Song.LRC` is common on files copied from Windows.
pub fn sidecar_path(audio: &Path) -> Option<PathBuf> {
    let dir = audio.parent()?;
    let stem = audio.file_stem()?.to_str()?.to_lowercase();
    let mut best: Option<(u8, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.filter_map(|e| e.ok()) {
        let p = entry.path();
        let (Some(s), Some(e)) = (p.file_stem().and_then(|s| s.to_str()), p.extension().and_then(|e| e.to_str())) else {
            continue;
        };
        if s.to_lowercase() != stem {
            continue;
        }
        let rank = match e.to_ascii_lowercase().as_str() {
            "lrc" => 0,
            "txt" => 1,
            _ => continue,
        };
        if best.as_ref().is_none_or(|(r, _)| rank < *r) {
            best = Some((rank, p));
        }
    }
    best.map(|(_, p)| p)
}

/// Finds lyrics for a track: a sidecar file first, then lyrics embedded in its tags. A synced
/// version beats an unsynced one when both exist.
pub fn find_lyrics(audio: &Path) -> Option<Lyrics> {
    let sidecar = sidecar_path(audio).and_then(|p| {
        if fs::metadata(&p).ok()?.len() > MAX_LYRICS_BYTES {
            return None;
        }
        let text = decode_text(&fs::read(&p).ok()?);
        Some(parse(&text, LyricsSource::Sidecar)).filter(|l| !l.lines.is_empty())
    });
    if sidecar.as_ref().is_some_and(|l| l.synced) {
        return sidecar;
    }
    let embedded = meta::read_embedded_lyrics(audio)
        .filter(|t| t.len() as u64 <= MAX_LYRICS_BYTES)
        .map(|t| parse(&t, LyricsSource::Embedded))
        .filter(|l| !l.lines.is_empty());
    match (sidecar, embedded) {
        (_, Some(e)) if e.synced => Some(e),
        (Some(s), _) => Some(s),
        (None, e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(text: &str) -> Lyrics {
        parse(text, LyricsSource::Text)
    }
    fn texts(l: &Lyrics) -> Vec<&str> {
        l.lines.iter().map(|l| l.text.as_str()).collect()
    }
    fn times(l: &Lyrics) -> Vec<u64> {
        l.lines.iter().map(|l| l.time_ms).collect()
    }

    #[test]
    fn basic_synced_lyrics_with_metadata() {
        let l = p("[ti:My Song]\n[ar:Some Band]\n[al:An Album]\n[00:12.00]Line one\n[00:15.50]Line two\n[01:02.345]Line three\n");
        assert!(l.synced);
        assert_eq!((l.title.as_deref(), l.artist.as_deref(), l.album.as_deref()), (Some("My Song"), Some("Some Band"), Some("An Album")));
        assert_eq!(texts(&l), ["Line one", "Line two", "Line three"]);
        assert_eq!(times(&l), [12_000, 15_500, 62_345]);
    }

    #[test]
    fn stamp_formats() {
        assert_eq!(parse_stamp("00:00"), Some(0));
        assert_eq!(parse_stamp("01:23"), Some(83_000));
        assert_eq!(parse_stamp("1:23.4"), Some(83_400));
        assert_eq!(parse_stamp("01:23.45"), Some(83_450));
        assert_eq!(parse_stamp("01:23.05"), Some(83_050));
        assert_eq!(parse_stamp("01:23.456"), Some(83_456));
        assert_eq!(parse_stamp("01:23:45"), Some(83_450));
        assert_eq!(parse_stamp("120:00.00"), Some(7_200_000));
        for bad in ["", ":", "1", "ab:cd", "01:60.00", "01:2x", "01:23.4567", "01:23.", "-1:00", "ti:Song", "00:00:00:00"] {
            assert_eq!(parse_stamp(bad), None, "{bad:?}");
        }
    }

    #[test]
    fn several_stamps_on_one_line_repeat_the_text_in_time_order() {
        let l = p("[00:30.00]Verse\n[00:10.00][01:10.00]Chorus\n");
        assert_eq!(texts(&l), ["Chorus", "Verse", "Chorus"]);
        assert_eq!(times(&l), [10_000, 30_000, 70_000]);
    }

    #[test]
    fn unsorted_input_is_sorted_and_ties_keep_their_order() {
        let l = p("[00:20.00]later\n[00:10.00]original\n[00:10.00]translation\n");
        assert_eq!(texts(&l), ["original", "translation", "later"]);
    }

    #[test]
    fn offset_shifts_lines_and_never_goes_negative() {
        let l = p("[offset:+500]\n[00:00.20]early\n[00:10.00]ten\n");
        assert_eq!(l.offset_ms, 500);
        assert_eq!(times(&l), [0, 9_500]);
        let l = p("[offset:-250]\n[00:10.00]ten\n");
        assert_eq!(times(&l), [10_250]);
        assert_eq!(p("[offset:junk]\n[00:01.00]x").offset_ms, 0);
    }

    #[test]
    fn empty_lines_are_kept_as_breaks() {
        let l = p("[00:05.00]sing\n[00:09.00]\n[00:12.00]sing again\n");
        assert_eq!(texts(&l), ["sing", "", "sing again"]);
    }

    #[test]
    fn lines_without_stamps_in_a_synced_file_are_ignored() {
        let l = p("[00:05.00]one\nstray comment line\n[00:09.00]two\n");
        assert_eq!(texts(&l), ["one", "two"]);
    }

    #[test]
    fn plain_text_becomes_unsynced_lyrics() {
        let l = p("\n\nFirst line\nSecond line\n\nNew verse\n\n");
        assert!(!l.synced);
        assert_eq!(texts(&l), ["First line", "Second line", "", "New verse"]);
        assert!(l.lines.iter().all(|l| l.time_ms == 0));
        assert_eq!(l.line_index_at(5_000), None, "unsynced lyrics have no current line");
        assert!(p("").lines.is_empty() && !p("").synced);
        assert!(p("   \n\n  ").lines.is_empty());
    }

    #[test]
    fn crlf_bom_and_odd_whitespace() {
        let l = p("\u{feff}[00:01.00] spaced \r\n[00:02.00]\tTabbed\r\n");
        assert_eq!(texts(&l), ["spaced", "Tabbed"]);
        let l = p("  [00:01.00]indented stamp");
        assert_eq!(texts(&l), ["indented stamp"]);
    }

    #[test]
    fn square_brackets_inside_text_are_kept() {
        let l = p("[00:01.00]Hello [world] and [00:99.99]\n[00:02.00][Chorus]\n");
        assert_eq!(texts(&l), ["Hello [world] and [00:99.99]", "[Chorus]"]);
    }

    #[test]
    fn enhanced_word_timing() {
        let l = p("[00:12.00]<00:12.00>Hel<00:12.30>lo <00:12.80>world<00:13.40>\n");
        let line = &l.lines[0];
        assert_eq!(line.text, "Hello world");
        assert_eq!(
            line.words,
            vec![
                LyricWord { time_ms: 12_000, text: "Hel".into() },
                LyricWord { time_ms: 12_300, text: "lo ".into() },
                LyricWord { time_ms: 12_800, text: "world".into() },
            ]
        );
        assert_eq!(l.word_index_at(0, 11_000), None);
        assert_eq!(l.word_index_at(0, 12_100), Some(0));
        assert_eq!(l.word_index_at(0, 12_800), Some(2));
        assert_eq!(l.word_index_at(0, 99_000), Some(2));
        assert_eq!(l.word_index_at(5, 0), None);
    }

    #[test]
    fn enhanced_text_without_a_line_stamp_and_with_leading_text() {
        let l = p("<00:05.00>Only <00:05.50>words\n");
        assert!(l.synced);
        assert_eq!((l.lines[0].time_ms, l.lines[0].text.as_str()), (5_000, "Only words"));
        // Text before the first word stamp takes the line's own stamp.
        let l = p("[00:01.00]Intro <00:02.00>then");
        assert_eq!(l.lines[0].words[0], LyricWord { time_ms: 1_000, text: "Intro ".into() });
        assert_eq!(l.lines[0].words[1], LyricWord { time_ms: 2_000, text: "then".into() });
    }

    #[test]
    fn word_stamps_get_the_offset_too() {
        let l = p("[offset:+1000]\n[00:05.00]<00:05.00>a <00:06.00>b");
        assert_eq!(l.lines[0].time_ms, 4_000);
        assert_eq!(l.lines[0].words.iter().map(|w| w.time_ms).collect::<Vec<_>>(), [4_000, 5_000]);
    }

    #[test]
    fn angle_brackets_that_are_not_stamps_are_just_text() {
        let l = p("[00:01.00]a <3 b > c <not a stamp>");
        assert_eq!(texts(&l), ["a <3 b > c <not a stamp>"]);
        assert!(l.lines[0].words.is_empty());
    }

    #[test]
    fn finding_the_current_line() {
        let l = p("[00:05.00]a\n[00:10.00]b\n[00:20.00]c\n");
        assert_eq!(l.line_index_at(0), None);
        assert_eq!(l.line_index_at(4_999), None);
        assert_eq!(l.line_index_at(5_000), Some(0));
        assert_eq!(l.line_index_at(9_999), Some(0));
        assert_eq!(l.line_index_at(10_000), Some(1));
        assert_eq!(l.line_index_at(1_000_000), Some(2));
    }

    #[test]
    fn text_decoding() {
        assert_eq!(decode_text(b"[00:01.00]plain"), "[00:01.00]plain");
        assert_eq!(decode_text(b"\xEF\xBB\xBF[00:01.00]bom"), "[00:01.00]bom");
        let utf16le: Vec<u8> = [0xFF, 0xFE].into_iter().chain("[00:01.00]héllo".encode_utf16().flat_map(u16::to_le_bytes)).collect();
        assert_eq!(decode_text(&utf16le), "[00:01.00]héllo");
        let utf16be: Vec<u8> = [0xFE, 0xFF].into_iter().chain("日本語".encode_utf16().flat_map(u16::to_be_bytes)).collect();
        assert_eq!(decode_text(&utf16be), "日本語");
        assert!(decode_text(b"bad \xFF\xFE bytes").contains("bad"), "invalid UTF-8 degrades, doesn't fail");
    }

    #[test]
    fn garbage_never_panics() {
        for s in ["[", "]", "[]", "[[[[", "<", "<>", "[00:01.00", "[00:01.00]<", "[00:01.00]<00:02", "<00:01.00><00:02.00>", "[:]", "[a:b:c]", "\u{0}\u{1}"] {
            let _ = p(s);
        }
    }

    // ---- finding lyrics on disk ----

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    #[test]
    fn embedded_lyrics_are_found_and_parsed_as_synced() {
        let l = find_lyrics(&fixture("tagged.flac")).unwrap();
        assert!(l.synced);
        assert_eq!(l.source, LyricsSource::Embedded);
        assert_eq!((texts(&l), times(&l)), (vec!["First line", "Second line"], vec![1_000, 2_500]));
        assert!(find_lyrics(&fixture("tagged.opus")).is_none());
    }

    #[test]
    fn sidecar_files_win_and_match_case_insensitively() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("My Song.flac");
        fs::copy(fixture("tagged.opus"), &song).unwrap();
        assert!(find_lyrics(&song).is_none());

        fs::write(dir.path().join("my song.TXT"), "plain words\nsecond").unwrap();
        let l = find_lyrics(&song).unwrap();
        assert!(!l.synced && l.source == LyricsSource::Sidecar);

        fs::write(dir.path().join("MY SONG.Lrc"), "[00:01.00]synced words").unwrap();
        let l = find_lyrics(&song).unwrap();
        assert!(l.synced, ".lrc beats .txt");
        assert_eq!(texts(&l), ["synced words"]);
        assert_eq!(sidecar_path(&song).unwrap().extension().unwrap().to_ascii_lowercase(), "lrc");
        fs::write(dir.path().join("Other Song.lrc"), "[00:01.00]not mine").unwrap();
        assert_eq!(texts(&find_lyrics(&song).unwrap()), ["synced words"]);
    }

    #[test]
    fn synced_embedded_beats_unsynced_sidecar_but_not_the_reverse() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("tagged.flac");
        fs::copy(fixture("tagged.flac"), &song).unwrap();
        fs::write(dir.path().join("tagged.txt"), "just plain text").unwrap();
        let l = find_lyrics(&song).unwrap();
        assert!(l.synced && l.source == LyricsSource::Embedded);
        fs::write(dir.path().join("tagged.lrc"), "[00:00.50]from the sidecar").unwrap();
        let l = find_lyrics(&song).unwrap();
        assert_eq!(l.source, LyricsSource::Sidecar, "a synced sidecar is preferred over embedded");
    }

    #[test]
    fn empty_or_huge_sidecars_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("a.flac");
        fs::copy(fixture("tagged.opus"), &song).unwrap();
        fs::write(dir.path().join("a.lrc"), "  \n \n").unwrap();
        assert!(find_lyrics(&song).is_none());
        fs::write(dir.path().join("a.lrc"), vec![b'x'; (MAX_LYRICS_BYTES + 1) as usize]).unwrap();
        assert!(find_lyrics(&song).is_none());
    }
}
