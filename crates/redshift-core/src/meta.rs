//! Reading what's *about* a file: tags, technical properties, cover art and ReplayGain.
//!
//! Tag quirks are smoothed over here so the rest of the app can rely on sensible values: a missing
//! title becomes the file name, "3/12" track numbers are split, ReplayGain strings like
//! `-6.50 dB` become numbers, and cover art falls back from the embedded picture to a
//! `cover.jpg`-style file next to the audio.
//!
//! Scanning skips picture data on purpose (it would mean reading every file's artwork), so cover
//! art is fetched lazily with [`read_cover`] when the UI actually shows it.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use lofty::config::ParseOptions;
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::PictureType;
use lofty::probe::Probe;
use lofty::tag::{Accessor, ItemKey, Tag};
use serde::{Deserialize, Serialize};

use crate::error::{Error, Result};

/// File extensions the player will pick up when scanning folders.
pub const AUDIO_EXTENSIONS: &[&str] =
    &["flac", "opus", "ogg", "oga", "mp3", "wav", "m4a", "mp4", "aac", "alac", "aif", "aiff"];

pub fn is_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| AUDIO_EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct TrackMeta {
    pub path: PathBuf,
    pub title: String,
    pub artist: String,
    pub album: String,
    /// Falls back to `artist` when the file has no album artist, so albums group sensibly.
    pub album_artist: String,
    pub genre: String,
    pub year: Option<u16>,
    pub track_no: Option<u32>,
    pub disc_no: Option<u32>,
    pub duration_secs: f64,
    pub sample_rate: Option<u32>,
    pub bit_depth: Option<u8>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
    /// Lower-case file extension, e.g. "flac".
    pub format: String,
    /// ReplayGain adjustment in dB, from the track and album tags.
    pub replaygain_track_db: Option<f32>,
    pub replaygain_album_db: Option<f32>,
    pub replaygain_track_peak: Option<f32>,
    pub has_embedded_lyrics: bool,
    /// File modification time (seconds since the epoch) and size, used to skip unchanged files
    /// when rescanning.
    pub modified: u64,
    pub size: u64,
    /// When the track first appeared in the library (seconds since the epoch), for "recently
    /// added". Older libraries without it use the file's modification time.
    #[serde(default)]
    pub added: u64,
}

pub const UNKNOWN_ARTIST: &str = "Unknown Artist";
pub const UNKNOWN_ALBUM: &str = "Unknown Album";

pub fn read_meta(path: &Path) -> Result<TrackMeta> {
    let fs_meta = fs::metadata(path)?;
    let tagged = open_tagged(path)?;
    let props = tagged.properties();
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag());

    let text = |get: fn(&Tag) -> Option<std::borrow::Cow<'_, str>>| -> String {
        tag.and_then(get).map(|s| s.trim().to_string()).unwrap_or_default()
    };
    let title = text(|t| t.title());
    let artist = text(|t| t.artist());
    let album_artist = tag
        .and_then(|t| t.get_string(ItemKey::AlbumArtist))
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty());
    let (replaygain_track_db, replaygain_album_db, replaygain_track_peak) = tag.map_or((None, None, None), |t| {
        (
            t.get_string(ItemKey::ReplayGainTrackGain).and_then(parse_gain_db),
            t.get_string(ItemKey::ReplayGainAlbumGain).and_then(parse_gain_db),
            t.get_string(ItemKey::ReplayGainTrackPeak).and_then(|s| s.trim().parse().ok()),
        )
    });

    let artist = if artist.is_empty() { UNKNOWN_ARTIST.to_string() } else { artist };
    Ok(TrackMeta {
        path: path.to_path_buf(),
        title: if title.is_empty() { file_stem(path) } else { title },
        album_artist: album_artist.unwrap_or_else(|| artist.clone()),
        artist,
        album: Some(text(|t| t.album())).filter(|s| !s.is_empty()).unwrap_or_else(|| UNKNOWN_ALBUM.to_string()),
        genre: text(|t| t.genre()),
        year: tag.and_then(|t| t.date()).map(|d| d.year),
        track_no: tag.and_then(|t| t.track()),
        disc_no: tag.and_then(|t| t.disk()),
        duration_secs: props.duration().as_secs_f64(),
        sample_rate: props.sample_rate(),
        bit_depth: props.bit_depth(),
        channels: props.channels(),
        bitrate_kbps: props.audio_bitrate(),
        format: path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase(),
        replaygain_track_db,
        replaygain_album_db,
        replaygain_track_peak,
        has_embedded_lyrics: tag.is_some_and(|t| embedded_lyrics_of(t).is_some()),
        modified: fs_meta.modified().ok().and_then(|m| m.duration_since(UNIX_EPOCH).ok()).map_or(0, |d| d.as_secs()),
        added: 0,
        size: fs_meta.len(),
    })
}

fn open_tagged(path: &Path) -> Result<TaggedFile> {
    Probe::open(path)
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?
        .options(ParseOptions::new().read_cover_art(false))
        .guess_file_type()
        .map_err(Error::Io)?
        .read()
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))
}

fn file_stem(path: &Path) -> String {
    path.file_stem().map_or_else(|| path.display().to_string(), |s| s.to_string_lossy().into_owned())
}

/// Parses ReplayGain strings: `-6.50 dB`, `+3.2dB`, `-6.5`.
pub fn parse_gain_db(s: &str) -> Option<f32> {
    let s = s.trim();
    let s = s.strip_suffix("dB").or_else(|| s.strip_suffix("db")).or_else(|| s.strip_suffix("DB")).unwrap_or(s);
    s.trim().parse().ok().filter(|v: &f32| v.is_finite())
}

/// Linear gain for a ReplayGain value, limited so the track's own peak can't clip. `peak` is the
/// track's sample peak (1.0 = full scale), if known.
pub fn replaygain_linear(gain_db: f32, peak: Option<f32>) -> f32 {
    let g = 10f32.powf(gain_db / 20.0);
    match peak {
        Some(p) if p > 0.0 && g * p > 1.0 => 1.0 / p,
        _ => g,
    }
}

fn embedded_lyrics_of(tag: &Tag) -> Option<String> {
    [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
        .into_iter()
        .filter_map(|k| tag.get_string(k))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .map(str::to_string)
}

/// Lyrics stored inside the file's tags, if any (synced LRC text or plain).
pub fn read_embedded_lyrics(path: &Path) -> Option<String> {
    let tagged = open_tagged(path).ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    embedded_lyrics_of(tag)
}

#[derive(Debug, Clone, PartialEq)]
pub struct Cover {
    pub mime: String,
    pub data: Vec<u8>,
}

const COVER_FILE_NAMES: &[&str] = &["cover", "folder", "front", "album", "albumart", "artwork"];
const COVER_EXTENSIONS: &[(&str, &str)] =
    &[("jpg", "image/jpeg"), ("jpeg", "image/jpeg"), ("png", "image/png"), ("webp", "image/webp")];

/// Cover art for a track: the embedded front cover (or any embedded picture), else an image file
/// like `cover.jpg` or `folder.png` in the same directory.
pub fn read_cover(path: &Path) -> Option<Cover> {
    if let Some(cover) = embedded_cover(path) {
        return Some(cover);
    }
    cover_file_next_to(path)
}

fn embedded_cover(path: &Path) -> Option<Cover> {
    // Unlike `read_meta`, this wants the pictures, so cover art parsing stays on.
    let tagged = Probe::open(path).ok()?.guess_file_type().ok()?.read().ok()?;
    let tag = tagged.primary_tag().or_else(|| tagged.first_tag())?;
    let pics = tag.pictures();
    let pic = pics.iter().find(|p| p.pic_type() == PictureType::CoverFront).or_else(|| pics.first())?;
    let mime = pic.mime_type().map_or_else(|| sniff_mime(pic.data()), |m| m.as_str().to_string());
    Some(Cover { mime, data: pic.data().to_vec() })
}

fn cover_file_next_to(path: &Path) -> Option<Cover> {
    let dir = path.parent()?;
    let mut found: Vec<(usize, PathBuf, &str)> = fs::read_dir(dir)
        .ok()?
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            let stem = p.file_stem()?.to_str()?.to_ascii_lowercase();
            let ext = p.extension()?.to_str()?.to_ascii_lowercase();
            let rank = COVER_FILE_NAMES.iter().position(|n| *n == stem)?;
            let mime = COVER_EXTENSIONS.iter().find(|(e, _)| *e == ext)?.1;
            Some((rank, p, mime))
        })
        .collect();
    found.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    let (_, p, mime) = found.into_iter().next()?;
    Some(Cover { mime: mime.to_string(), data: fs::read(p).ok()? })
}

fn sniff_mime(data: &[u8]) -> String {
    match data {
        [0xFF, 0xD8, 0xFF, ..] => "image/jpeg",
        [0x89, b'P', b'N', b'G', ..] => "image/png",
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => "image/webp",
        _ => "application/octet-stream",
    }
    .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }

    #[test]
    fn reads_a_fully_tagged_flac() {
        let m = read_meta(&fixture("tagged.flac")).unwrap();
        assert_eq!(m.title, "Tagged Song");
        assert_eq!(m.artist, "The Testers");
        assert_eq!(m.album, "Fixture Album");
        assert_eq!(m.album_artist, "Various Testers");
        assert_eq!(m.genre, "Test");
        assert_eq!(m.year, Some(2021));
        assert_eq!((m.track_no, m.disc_no), (Some(3), Some(2)));
        assert_eq!((m.sample_rate, m.channels, m.format.as_str()), (Some(44_100), Some(1), "flac"));
        assert!((m.duration_secs - 1.0).abs() < 0.05, "{}", m.duration_secs);
        assert_eq!(m.replaygain_track_db, Some(-6.5));
        assert_eq!(m.replaygain_album_db, Some(-5.25));
        assert_eq!(m.replaygain_track_peak, Some(0.9));
        assert!(m.has_embedded_lyrics);
        assert!(m.size > 1000 && m.modified > 0);
    }

    #[test]
    fn reads_opus_and_mp3_tags() {
        let o = read_meta(&fixture("tagged.opus")).unwrap();
        assert_eq!((o.title.as_str(), o.artist.as_str(), o.album.as_str()), ("Opus Song", "Opus Artist", "Opus Album"));
        assert_eq!((o.year, o.track_no), (Some(1999), Some(7)));
        assert_eq!(o.album_artist, "Opus Artist", "album artist falls back to artist");
        assert_eq!(o.sample_rate, Some(48_000));

        let m = read_meta(&fixture("tagged.mp3")).unwrap();
        assert_eq!((m.title.as_str(), m.artist.as_str()), ("MP3 Song", "MP3 Artist"));
        assert_eq!((m.year, m.track_no), (Some(2010), Some(1)));
    }

    #[test]
    fn untagged_files_fall_back_to_sensible_values() {
        let m = read_meta(&fixture("Untagged Name.flac")).unwrap();
        assert_eq!(m.title, "Untagged Name", "title from the file name");
        assert_eq!((m.artist.as_str(), m.album.as_str()), (UNKNOWN_ARTIST, UNKNOWN_ALBUM));
        assert_eq!(m.album_artist, UNKNOWN_ARTIST);
        assert_eq!((m.year, m.track_no, m.replaygain_track_db), (None, None, None));
        assert!(!m.has_embedded_lyrics);
    }

    #[test]
    fn broken_and_missing_files_are_errors() {
        assert!(read_meta(&fixture("nope.flac")).is_err());
        let dir = tempfile::tempdir().unwrap();
        let junk = dir.path().join("junk.mp3");
        fs::write(&junk, vec![1u8; 2048]).unwrap();
        assert!(read_meta(&junk).is_err());
    }

    #[test]
    fn replaygain_strings_and_gain_math() {
        assert_eq!(parse_gain_db("-6.50 dB"), Some(-6.5));
        assert_eq!(parse_gain_db(" +3.2dB "), Some(3.2));
        assert_eq!(parse_gain_db("-6.5"), Some(-6.5));
        assert_eq!(parse_gain_db("0.00 DB"), Some(0.0));
        assert_eq!(parse_gain_db("loud"), None);
        assert_eq!(parse_gain_db("NaN dB"), None);

        assert!((replaygain_linear(-6.0206, None) - 0.5).abs() < 1e-3);
        assert!((replaygain_linear(6.0206, Some(0.4)) - 2.0).abs() < 1e-3, "room to spare: full boost");
        assert!((replaygain_linear(6.0206, Some(0.9)) - 1.0 / 0.9).abs() < 1e-3, "boost limited by the peak");
        assert_eq!(replaygain_linear(0.0, Some(1.0)), 1.0);
    }

    #[test]
    fn embedded_cover_is_found_and_typed() {
        let c = read_cover(&fixture("tagged.flac")).expect("cover");
        assert_eq!(c.mime, "image/png");
        assert_eq!(&c.data[1..4], b"PNG");
        assert!(read_cover(&fixture("tagged.mp3")).is_some());
        assert!(read_cover(&fixture("tagged.opus")).is_none(), "no picture and no cover file in that folder");
    }

    #[test]
    fn falls_back_to_a_cover_file_in_the_folder_and_prefers_better_names() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("song.flac");
        fs::copy(fixture("Untagged Name.flac"), &song).unwrap();
        assert!(read_cover(&song).is_none());
        fs::write(dir.path().join("random.jpg"), b"\xFF\xD8\xFFnot a cover name").unwrap();
        assert!(read_cover(&song).is_none(), "arbitrary images aren't covers");
        fs::write(dir.path().join("Folder.PNG"), b"\x89PNG....").unwrap();
        let c = read_cover(&song).unwrap();
        assert_eq!((c.mime.as_str(), &c.data[..4]), ("image/png", &b"\x89PNG"[..]));
        fs::write(dir.path().join("cover.jpg"), b"\xFF\xD8\xFF-cover").unwrap();
        assert_eq!(read_cover(&song).unwrap().mime, "image/jpeg", "cover.* ranks above folder.*");
    }

    #[test]
    fn embedded_lyrics_are_returned() {
        let text = read_embedded_lyrics(&fixture("tagged.flac")).unwrap();
        assert!(text.contains("[00:01.00]First line"));
        assert!(read_embedded_lyrics(&fixture("tagged.opus")).is_none());
    }

    #[test]
    fn audio_extension_detection() {
        for ok in ["a.flac", "b.OPUS", "c.Mp3", "d.m4a", "e.wav"] {
            assert!(is_audio_file(Path::new(ok)), "{ok}");
        }
        for no in ["a.txt", "b.jpg", "noext", "c.lrc", ".flac.bak"] {
            assert!(!is_audio_file(Path::new(no)), "{no}");
        }
    }
}
