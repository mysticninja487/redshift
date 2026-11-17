//! Lyrics from BetterLyrics (https://better-lyrics.boidu.dev), a community lyrics service by boidushya
//! with word-by-word timing.
//!
//! What is sent: the song's title, artist, album and length in whole seconds, as the query of an
//! ordinary HTTPS request, with the IP address and the app's name and version, as with any web
//! request. No API key is used. The service only answers for songs it already has, and replies
//! `401` for the rest, which is treated here as "nothing there".
//!
//! The service delivers TTML (a W3C XML subtitle format) inside JSON. [`parse_ttml`] turns it into
//! enhanced LRC (`[mm:ss.xx]<mm:ss.xx>word <mm:ss.xx>word`), which Redshift's own lyrics parser,
//! the `.lrc` files other players read and the lyrics panel all understand. A small reader is used
//! instead of an XML crate because only a handful of elements matter and the input is untrusted:
//! nothing is ever fetched or expanded from it.

use std::time::Duration;

use serde::Deserialize;

use super::{encode_query, Net, Request};
use crate::error::{Error, Result};

pub const HOST: &str = "api.betterlyrics.org";
const BASE: &str = "https://api.betterlyrics.org";
/// Answers are kept for a month; "nothing there" is asked again after a day, because the service
/// learns songs over time.
const HIT_TTL: Duration = Duration::from_secs(30 * 24 * 3600);
const MISS_TTL: Duration = Duration::from_secs(24 * 3600);
/// The service rates its own guess from 0 to 1. Lower than this is likelier a different song.
const MIN_SCORE: f64 = 0.5;
/// Lyrics that start a line later than this past the end of the song belong to another recording.
const SLACK_MS: u64 = 12_000;
/// A reply with more lines than this is not a song.
const MAX_LINES: usize = 2_000;

/// What to look for. The service wants all four, since its cache is keyed on all of them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Query {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub duration_secs: Option<f64>,
}

/// What was found, in the forms the app needs.
#[derive(Debug, Clone, PartialEq)]
pub struct Found {
    /// Enhanced LRC with a stamp for every word (line stamps only if the song has no word timing).
    pub enhanced: String,
    /// The same lines with line stamps only, for places that don't understand word stamps.
    pub lines: String,
    /// The words without any time.
    pub plain: String,
    pub word_synced: bool,
}

#[derive(Deserialize)]
struct Reply {
    ttml: Option<String>,
    score: Option<f64>,
}

fn get_query(q: &Query) -> String {
    let mut s = format!("s={}&a={}", encode_query(&q.title), encode_query(&q.artist));
    s.push_str(&format!("&al={}", encode_query(q.album.as_deref().unwrap_or(""))));
    if let Some(d) = q.duration_secs.filter(|d| (1.0..=3600.0).contains(d)) {
        s.push_str(&format!("&d={}", d.round() as u32));
    }
    s
}

/// Looks the song up. `Ok(None)` means the service has nothing usable for it.
pub fn lookup(net: &Net, q: &Query) -> Result<Option<Found>> {
    lookup_at(net, BASE, q)
}

pub(crate) fn lookup_at(net: &Net, base: &str, q: &Query) -> Result<Option<Found>> {
    if q.title.trim().is_empty() || q.artist.trim().is_empty() {
        return Ok(None);
    }
    let url = format!("{base}/getLyrics?{}", get_query(q));
    let r = net.get(&Request::new(&url, "Lyrics").cached_for(HIT_TTL).misses_for(MISS_TTL))?;
    match r.status {
        200 => {}
        // Not known to the service (and it has no key for us to ask it to look), or no lyrics, or no title.
        401 | 404 | 422 => return Ok(None),
        s => return Err(Error::Network(format!("the lyrics service answered {s}"))),
    }
    let reply: Reply = serde_json::from_str(&r.body).map_err(|e| Error::Network(format!("unreadable lyrics reply: {e}")))?;
    if reply.score.is_some_and(|s| s < MIN_SCORE) {
        return Ok(None);
    }
    let Some(ttml) = reply.ttml else { return Ok(None) };
    let Some(found) = parse_ttml(&ttml) else { return Ok(None) };
    if let (Some(d), Some(last)) = (q.duration_secs, found.last_line_ms) {
        if last > (d.max(0.0) * 1000.0) as u64 + SLACK_MS {
            return Ok(None);
        }
    }
    Ok(Some(found.found))
}

// ---------------------------------------------------------------------------------------------
// TTML -> LRC

struct Parsed {
    found: Found,
    last_line_ms: Option<u64>,
}

/// A line being collected: its start and its timed words, plus loose text if it has no timed words.
#[derive(Default)]
struct Line {
    begin: Option<u64>,
    words: Vec<(u64, String)>,
    bg: Vec<(u64, String)>,
    loose: String,
}

#[derive(Debug)]
enum Tok {
    Open { name: String, attrs: Vec<(String, String)>, selfclose: bool },
    Close(String),
    Text(String),
}

/// The part of a name after any `prefix:`, in lower case.
fn local(name: &str) -> String {
    name.rsplit(':').next().unwrap_or(name).to_ascii_lowercase()
}

fn decode(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|e| *e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let ent = &rest[1..end];
        let ch = match ent {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => ent
                .strip_prefix("#x")
                .or_else(|| ent.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| ent.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

fn tokenize(xml: &str) -> Vec<Tok> {
    let mut toks = Vec::new();
    let mut rest = xml;
    while !rest.is_empty() {
        let Some(lt) = rest.find('<') else {
            toks.push(Tok::Text(decode(rest)));
            break;
        };
        if lt > 0 {
            toks.push(Tok::Text(decode(&rest[..lt])));
        }
        rest = &rest[lt..];
        if let Some(r) = rest.strip_prefix("<!--") {
            rest = r.find("-->").map_or("", |e| &r[e + 3..]);
        } else if let Some(r) = rest.strip_prefix("<![CDATA[") {
            let e = r.find("]]>").unwrap_or(r.len());
            toks.push(Tok::Text(r[..e].to_string()));
            rest = r.get(e + 3..).unwrap_or("");
        } else if rest.starts_with("<?") || rest.starts_with("<!") {
            rest = rest.find('>').map_or("", |e| &rest[e + 1..]);
        } else {
            // Find the closing '>' that is not inside a quoted attribute value.
            let (mut quote, mut end) = (None, None);
            for (i, c) in rest.char_indices().skip(1) {
                match (quote, c) {
                    (None, '"' | '\'') => quote = Some(c),
                    (Some(q), c) if c == q => quote = None,
                    (None, '>') => {
                        end = Some(i);
                        break;
                    }
                    _ => {}
                }
            }
            let Some(end) = end else { break };
            let inner = &rest[1..end];
            rest = &rest[end + 1..];
            if let Some(name) = inner.strip_prefix('/') {
                toks.push(Tok::Close(local(name.trim())));
                continue;
            }
            let selfclose = inner.ends_with('/');
            let inner = inner.trim_end_matches('/');
            let name_end = inner.find(|c: char| c.is_whitespace()).unwrap_or(inner.len());
            toks.push(Tok::Open { name: local(&inner[..name_end]), attrs: attrs(&inner[name_end..]), selfclose });
        }
    }
    toks
}

fn attrs(s: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let mut rest = s.trim_start();
    while let Some(eq) = rest.find('=') {
        let key = local(rest[..eq].trim());
        let after = rest[eq + 1..].trim_start();
        let Some(q) = after.chars().next().filter(|c| *c == '"' || *c == '\'') else { break };
        let body = &after[1..];
        let Some(close) = body.find(q) else { break };
        out.push((key, decode(&body[..close])));
        rest = body[close + 1..].trim_start();
    }
    out
}

fn attr<'a>(attrs: &'a [(String, String)], key: &str) -> Option<&'a str> {
    attrs.iter().find(|(k, _)| k == key).map(|(_, v)| v.as_str())
}

/// `12.345`, `1:23.456`, `01:23:45.678`, `12.3s` and `1200ms` to milliseconds.
fn parse_time(s: &str) -> Option<u64> {
    let s = s.trim();
    if let Some(ms) = s.strip_suffix("ms") {
        return ms.trim().parse::<f64>().ok().filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as u64);
    }
    let s = s.strip_suffix('s').unwrap_or(s);
    let mut secs = 0.0f64;
    for part in s.split(':') {
        let v: f64 = part.trim().parse().ok().filter(|v: &f64| v.is_finite() && *v >= 0.0)?;
        secs = secs * 60.0 + v;
    }
    (secs < 86_400.0).then(|| (secs * 1000.0).round() as u64)
}

fn squash(text: &str) -> String {
    text.chars().map(|c| if c.is_whitespace() { ' ' } else { c }).collect()
}

fn stamp(ms: u64) -> String {
    let cs = ms / 10;
    format!("{:02}:{:02}.{:02}", cs / 6000, cs / 100 % 60, cs % 100)
}

/// Joins words as `<t>word` and drops the space that ends the line.
fn enhanced_line(first: u64, words: &[(u64, String)]) -> String {
    let mut s = format!("[{}]", stamp(first));
    for (t, w) in words {
        s.push_str(&format!("<{}>{}", stamp(*t), w));
    }
    s.trim_end().to_string()
}

/// Reads the lyrics out of a TTML document. `None` if it holds no lines.
fn parse_ttml(xml: &str) -> Option<Parsed> {
    // (time, enhanced text, line-only text, plain text); background singers become their own lines.
    let mut out: Vec<(u64, String, String, String)> = Vec::new();
    let mut any_words = false;

    let mut head = 0u32;
    let mut skip = 0u32; // depth inside a translation or transliteration, which is not the song's text
    let mut bg = 0u32; // depth inside a background-vocal wrapper
    let mut line: Option<Line> = None;
    // The open timed word: where it starts and whether it belongs to the background singers.
    let mut word: Option<(u64, bool, String)> = None;
    let mut span_kinds: Vec<(bool, bool, bool)> = Vec::new(); // per open span: (skip, bg, timed)

    for tok in tokenize(xml) {
        match tok {
            Tok::Open { name, attrs, selfclose } => match name.as_str() {
                "head" if !selfclose => head += 1,
                "p" if head == 0 && !selfclose => {
                    line = Some(Line { begin: attr(&attrs, "begin").and_then(parse_time), ..Line::default() });
                }
                "span" if head == 0 && line.is_some() => {
                    let role = attr(&attrs, "role").unwrap_or("");
                    let is_skip = role.starts_with("x-translation") || role.starts_with("x-roman");
                    let is_bg = role == "x-bg";
                    let begin = attr(&attrs, "begin").and_then(parse_time);
                    let timed = begin.is_some() && skip == 0 && !is_skip;
                    if selfclose {
                        continue;
                    }
                    if is_skip {
                        skip += 1;
                    }
                    if is_bg {
                        bg += 1;
                    }
                    if timed {
                        word = Some((begin.unwrap_or(0), bg > 0, String::new()));
                    }
                    span_kinds.push((is_skip, is_bg, timed));
                }
                "br" if head == 0 && skip == 0 => {
                    if let Some((_, _, t)) = word.as_mut() {
                        t.push(' ');
                    }
                }
                _ => {}
            },
            Tok::Close(name) => match name.as_str() {
                "head" => head = head.saturating_sub(1),
                "span" if head == 0 => {
                    let Some((is_skip, is_bg, timed)) = span_kinds.pop() else { continue };
                    if timed {
                        if let (Some((t, in_bg, text)), Some(l)) = (word.take(), line.as_mut()) {
                            if !text.trim().is_empty() {
                                if in_bg { &mut l.bg } else { &mut l.words }.push((t, text));
                            } else if let Some(last) = if in_bg { &mut l.bg } else { &mut l.words }.last_mut() {
                                last.1.push(' ');
                            }
                        }
                    }
                    if is_skip {
                        skip = skip.saturating_sub(1);
                    }
                    if is_bg {
                        bg = bg.saturating_sub(1);
                    }
                }
                "p" if head == 0 => {
                    let Some(l) = line.take() else { continue };
                    word = None;
                    span_kinds.clear();
                    skip = 0;
                    bg = 0;
                    let untimed = l.words.is_empty() && l.bg.is_empty();
                    for (words, wrap) in [(l.words, false), (l.bg, true)] {
                        if words.is_empty() {
                            continue;
                        }
                        any_words = true;
                        let mut words = words;
                        words.sort_by_key(|w| w.0);
                        if wrap {
                            // Some documents already put the brackets in the text.
                            if let Some(first) = words.first_mut().filter(|w| !w.1.trim_start().starts_with('(')) {
                                first.1.insert(0, '(');
                            }
                            if let Some(last) = words.last_mut().filter(|w| !w.1.trim_end().ends_with(')')) {
                                let trimmed = last.1.trim_end().len();
                                last.1.insert(trimmed, ')');
                            }
                        }
                        let first = if wrap { words[0].0 } else { l.begin.map_or(words[0].0, |b| b.min(words[0].0)) };
                        let text: String = words.iter().map(|w| w.1.as_str()).collect::<String>();
                        let text = squash(&text).split_whitespace().collect::<Vec<_>>().join(" ");
                        out.push((first, enhanced_line(first, &words), format!("[{}]{}", stamp(first), text), text));
                    }
                    let loose = squash(&l.loose).split_whitespace().collect::<Vec<_>>().join(" ");
                    if untimed && !loose.is_empty() {
                        if let Some(b) = l.begin {
                            let line_lrc = format!("[{}]{}", stamp(b), loose);
                            out.push((b, line_lrc.clone(), line_lrc, loose));
                        }
                    }
                }
                _ => {}
            },
            Tok::Text(text) => {
                if head > 0 || skip > 0 {
                    continue;
                }
                let Some(l) = line.as_mut() else { continue };
                if let Some((_, _, w)) = word.as_mut() {
                    w.push_str(&squash(&text));
                } else if !l.words.is_empty() || !l.bg.is_empty() {
                    // spacing between words
                    if text.chars().all(char::is_whitespace) && bg == 0 {
                        if let Some(last) = l.words.last_mut() {
                            if !last.1.ends_with(' ') {
                                last.1.push(' ');
                            }
                        }
                    }
                } else {
                    l.loose.push_str(&text);
                }
            }
        }
    }

    if out.is_empty() || out.len() > MAX_LINES {
        return None;
    }
    out.sort_by_key(|l| l.0); // stable
    let last_line_ms = out.last().map(|l| l.0);
    let join = |f: fn(&(u64, String, String, String)) -> &str| out.iter().map(f).collect::<Vec<_>>().join("\n");
    Some(Parsed {
        found: Found { enhanced: join(|l| &l.1), lines: join(|l| &l.2), plain: join(|l| &l.3), word_synced: any_words },
        last_line_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::super::testserver::*;
    use super::*;

    const DOC: &str = r#"<?xml version="1.0" encoding="UTF-8"?>
<tt xmlns="http://www.w3.org/ns/ttml" xmlns:ttm="http://www.w3.org/ns/ttml#metadata" xmlns:itunes="http://music.apple.com/lyric-ttml-internal">
<head><metadata><ttm:agent type="person" xml:id="v1"/>
<iTunesMetadata><transliterations><transliteration xml:lang="ja-Latn"><text for="L1"><span begin="0:01.000" end="0:01.500">nope </span></text></transliteration></transliterations></iTunesMetadata></metadata></head>
<body dur="3:00.000"><div begin="0:01.000" end="1:00.000">
<p begin="0:01.000" end="0:03.000" ttm:agent="v1" itunes:key="L1"><span begin="0:01.000" end="0:01.500">Hel</span><span begin="0:01.500" end="0:02.000">lo </span><span begin="0:02.000" end="0:03.000">world &amp; you</span><span ttm:role="x-bg"><span begin="0:02.500" end="0:03.000">(ooh)</span></span></p>
<p begin="1:05.250" end="1:08.000" ttm:agent="v1"><span begin="1:05.250" end="1:06.000">Second</span> <span begin="1:06.000" end="1:08.000">line</span></p>
</div></body></tt>"#;

    fn q() -> Query {
        Query { title: "Limbo".into(), artist: "Haruno".into(), album: Some("Limbo".into()), duration_secs: Some(201.4) }
    }
    fn json(ttml: &str) -> String {
        serde_json::json!({ "ttml": ttml }).to_string()
    }

    #[test]
    fn words_become_enhanced_lrc_and_the_head_is_ignored() {
        let p = parse_ttml(DOC).unwrap();
        let lines: Vec<&str> = p.found.enhanced.lines().collect();
        assert_eq!(lines[0], "[00:01.00]<00:01.00>Hel<00:01.50>lo <00:02.00>world & you");
        assert_eq!(lines[1], "[00:02.50]<00:02.50>(ooh)", "background singers are their own line, in time order");
        assert_eq!(lines[2], "[01:05.25]<01:05.25>Second <01:06.00>line", "spacing between spans is kept");
        assert_eq!(lines.len(), 3);
        assert!(p.found.word_synced && !p.found.enhanced.contains("nope"));
        assert_eq!(p.found.lines, "[00:01.00]Hello world & you\n[00:02.50](ooh)\n[01:05.25]Second line");
        assert_eq!(p.found.plain, "Hello world & you\n(ooh)\nSecond line");
    }

    #[test]
    fn our_own_parser_reads_the_result_with_word_timing() {
        let p = parse_ttml(DOC).unwrap();
        let l = crate::lyrics::parse(&p.found.enhanced, crate::lyrics::LyricsSource::Online);
        assert!(l.synced && l.lines.len() == 3);
        assert_eq!(l.lines[0].text, "Hello world & you");
        assert_eq!(l.lines[0].words.iter().map(|w| w.time_ms).collect::<Vec<_>>(), vec![1000, 1500, 2000]);
        assert_eq!(l.lines[2].time_ms, 65_250);
    }

    #[test]
    fn line_synced_ttml_without_word_spans_works() {
        let doc = r#"<tt xmlns="http://www.w3.org/ns/ttml"><body><div>
        <p begin="00:00:05.500" end="00:00:08.000">First &lt;line&gt;</p>
        <p begin="00:00:09" end="00:00:12">Second
        line</p></div></body></tt>"#;
        let p = parse_ttml(doc).unwrap();
        assert!(!p.found.word_synced);
        assert_eq!(p.found.enhanced, "[00:05.50]First <line>\n[00:09.00]Second line");
        assert_eq!(p.last_line_ms, Some(9000));
    }

    #[test]
    fn translations_are_not_part_of_the_lyrics() {
        let doc = r#"<tt xmlns:ttm="x"><body><div><p begin="1.0"><span begin="1.0" end="2.0">Bonjour</span><span ttm:role="x-translation" xml:lang="en">Hello</span></p></div></body></tt>"#;
        assert_eq!(parse_ttml(doc).unwrap().found.plain, "Bonjour");
    }

    #[test]
    fn garbage_and_empty_documents_give_nothing() {
        assert!(parse_ttml("").is_none());
        assert!(parse_ttml("not xml at all").is_none());
        assert!(parse_ttml("<tt><body><div></div></body></tt>").is_none());
        assert!(parse_ttml("<tt><body><div><p begin=\"1\"></p></div></body></tt>").is_none());
        assert!(parse_ttml("<tt><body><p begin=\"1\"><span begin=").is_none());
    }

    #[test]
    fn time_formats() {
        assert_eq!(parse_time("12.345"), Some(12_345));
        assert_eq!(parse_time("1:23.456"), Some(83_456));
        assert_eq!(parse_time("01:02:03.5"), Some(3_723_500));
        assert_eq!(parse_time("12.3s"), Some(12_300));
        assert_eq!(parse_time("1200ms"), Some(1200));
        assert_eq!(parse_time("x"), None);
        assert_eq!(parse_time("-3"), None);
        assert_eq!(parse_time("NaN"), None);
    }

    #[test]
    fn entities() {
        assert_eq!(decode("a &amp; b &#39;c&#x41; &bogus; &"), "a & b 'cA &bogus; &");
    }

    #[test]
    fn the_request_carries_all_four_values_and_the_reply_is_read() {
        let d = tempfile::tempdir().unwrap();
        let s = start(vec![reply(200, &json(DOC))]);
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        let f = lookup_at(&n, &s.base, &q()).unwrap().unwrap();
        assert!(f.word_synced && f.enhanced.starts_with("[00:01.00]<00:01.00>Hel"));
        let sent = s.requests.lock()[0].clone();
        assert!(sent.starts_with("GET /getLyrics?s=Limbo&a=Haruno&al=Limbo&d=201 "), "{sent}");
    }

    #[test]
    fn unknown_songs_and_wrong_recordings_are_not_errors() {
        let d = tempfile::tempdir().unwrap();
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        for st in [401, 404, 422] {
            let s = start(vec![reply(st, r#"{"error":"x"}"#)]);
            assert_eq!(lookup_at(&n, &s.base, &q()).unwrap(), None, "{st}");
        }
        // lines that run long past the end of this recording
        let s = start(vec![reply(200, &json(DOC))]);
        let short = Query { duration_secs: Some(30.0), ..q() };
        assert_eq!(lookup_at(&n, &s.base, &short).unwrap(), None);
        // a low match score
        let low = serde_json::json!({ "ttml": DOC, "score": 0.2 }).to_string();
        let s = start(vec![reply(200, &low)]);
        assert_eq!(lookup_at(&n, &s.base, &Query { title: "Other".into(), ..q() }).unwrap(), None);
    }

    #[test]
    fn not_there_is_remembered_and_failures_are_errors() {
        let d = tempfile::tempdir().unwrap();
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        let s = start(vec![reply(401, "{}"), reply(200, &json(DOC))]);
        assert_eq!(lookup_at(&n, &s.base, &q()).unwrap(), None);
        assert_eq!(lookup_at(&n, &s.base, &q()).unwrap(), None);
        assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 1, "the second lookup came from the cache");
        let s = start(vec![reply(500, "oops")]);
        assert!(matches!(lookup_at(&n, &s.base, &Query { title: "Else".into(), ..q() }), Err(Error::Network(_))));
        let s = start(vec![reply(200, "<html>")]);
        assert!(matches!(lookup_at(&n, &s.base, &Query { title: "Third".into(), ..q() }), Err(Error::Network(_))));
    }

    #[test]
    fn missing_title_or_artist_asks_nobody() {
        let d = tempfile::tempdir().unwrap();
        let n = Net::new(d.path().join("c"), "Redshift-test", &["127.0.0.1"]);
        n.set_enabled(true);
        let s = start(vec![reply(200, &json(DOC))]);
        assert_eq!(lookup_at(&n, &s.base, &Query { title: "Limbo".into(), ..Default::default() }).unwrap(), None);
        assert_eq!(s.hits.load(std::sync::atomic::Ordering::SeqCst), 0);
    }
}
