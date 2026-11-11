//! Editing what's *in* a file's tags: text fields, cover art and embedded lyrics.
//!
//! Writes never touch the original in place. The edit is made on a copy next to it and then moved
//! over the original, so a failed write (disk full, a read-only file, a crash) leaves the song as it
//! was, and a player that has the file open never sees half-written data.
//!
//! The UI sends only what the person changed: a field that is missing is left alone, `null`
//! clears it and a value sets it.

use std::fs;
use std::path::{Path, PathBuf};

use lofty::config::{ParseOptions, WriteOptions};
use lofty::file::{AudioFile, TaggedFile, TaggedFileExt};
use lofty::picture::{MimeType, Picture, PictureType};
use lofty::probe::Probe;
use lofty::tag::items::Timestamp;
use lofty::tag::{Accessor, ItemKey, Tag};
use serde::{Deserialize, Deserializer, Serialize};

use crate::error::{Error, Result};
use crate::lyrics::MAX_LYRICS_BYTES;
use crate::meta;

/// Largest cover image accepted, so a stray photo can't bloat every file.
pub const MAX_COVER_BYTES: u64 = 16 * 1024 * 1024;

/// What the editor shows for a file: the raw tag values (a missing title stays empty rather than
/// falling back to the file name), the embedded lyrics and whether there is an embedded picture.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct TagForm {
    pub path: PathBuf,
    pub format: String,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub album_artist: String,
    pub genre: String,
    pub composer: String,
    pub comment: String,
    pub year: Option<u16>,
    pub track_no: Option<u32>,
    pub track_total: Option<u32>,
    pub disc_no: Option<u32>,
    pub disc_total: Option<u32>,
    /// Lyrics stored in the tags: plain text, or LRC text when synced.
    pub lyrics: String,
    /// An embedded picture exists (a `cover.jpg` next to the file doesn't count).
    pub has_cover: bool,
    /// The file the tags are saved to can be changed (not read-only).
    pub writable: bool,
}

/// Distinguishes "not mentioned" from `null` (clear) and a value (set).
fn patch<'de, D, T>(d: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(d).map(Some)
}

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct TagEdit {
    #[serde(deserialize_with = "patch")]
    pub title: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub artist: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub album: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub album_artist: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub genre: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub composer: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub comment: Option<Option<String>>,
    #[serde(deserialize_with = "patch")]
    pub year: Option<Option<u16>>,
    #[serde(deserialize_with = "patch")]
    pub track_no: Option<Option<u32>>,
    #[serde(deserialize_with = "patch")]
    pub track_total: Option<Option<u32>>,
    #[serde(deserialize_with = "patch")]
    pub disc_no: Option<Option<u32>>,
    #[serde(deserialize_with = "patch")]
    pub disc_total: Option<Option<u32>>,
    /// Lyrics text to embed (LRC when synced); `null` or empty removes them.
    #[serde(deserialize_with = "patch")]
    pub lyrics: Option<Option<String>>,
    pub cover: Option<CoverEdit>,
}

impl TagEdit {
    /// Whether the edit changes nothing.
    pub fn is_empty(&self) -> bool {
        self.title.is_none()
            && self.artist.is_none()
            && self.album.is_none()
            && self.album_artist.is_none()
            && self.genre.is_none()
            && self.composer.is_none()
            && self.comment.is_none()
            && self.year.is_none()
            && self.track_no.is_none()
            && self.track_total.is_none()
            && self.disc_no.is_none()
            && self.disc_total.is_none()
            && self.lyrics.is_none()
            && self.cover.is_none()
    }
}

#[derive(Debug, Clone, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CoverEdit {
    /// Take the embedded picture out.
    Remove,
    /// Use this image file as the front cover.
    File(PathBuf),
}

fn open_with_pictures(path: &Path) -> Result<TaggedFile> {
    Probe::open(path)
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))?
        .options(ParseOptions::new())
        .guess_file_type()
        .map_err(Error::Io)?
        .read()
        .map_err(|e| Error::Decode(format!("{}: {e}", path.display())))
}

fn lyrics_of(tag: &Tag) -> String {
    [ItemKey::Lyrics, ItemKey::UnsyncLyrics]
        .into_iter()
        .filter_map(|k| tag.get_string(k))
        .map(str::trim)
        .find(|s| !s.is_empty())
        .unwrap_or_default()
        .to_string()
}

fn text(tag: &Tag, key: ItemKey) -> String {
    tag.get_string(key).map(|s| s.trim().to_string()).unwrap_or_default()
}

/// The editable tags of an audio file.
pub fn read_form(path: &Path) -> Result<TagForm> {
    if !meta::is_audio_file(path) {
        return Err(Error::Unsupported(format!("{} is not an audio file", path.display())));
    }
    let tagged = open_with_pictures(path)?;
    let format = path.extension().and_then(|e| e.to_str()).unwrap_or("").to_ascii_lowercase();
    let writable = fs::metadata(path).map(|m| !m.permissions().readonly()).unwrap_or(false);
    let Some(tag) = tagged.primary_tag().or_else(|| tagged.first_tag()) else {
        return Ok(TagForm { path: path.to_path_buf(), format, writable, ..TagForm::default() });
    };
    Ok(TagForm {
        path: path.to_path_buf(),
        format,
        title: tag.title().map(|s| s.trim().to_string()).unwrap_or_default(),
        artist: tag.artist().map(|s| s.trim().to_string()).unwrap_or_default(),
        album: tag.album().map(|s| s.trim().to_string()).unwrap_or_default(),
        album_artist: text(tag, ItemKey::AlbumArtist),
        genre: tag.genre().map(|s| s.trim().to_string()).unwrap_or_default(),
        composer: text(tag, ItemKey::Composer),
        comment: tag.comment().map(|s| s.trim().to_string()).unwrap_or_default(),
        year: tag.date().map(|d| d.year),
        track_no: tag.track(),
        track_total: tag.track_total(),
        disc_no: tag.disk(),
        disc_total: tag.disk_total(),
        lyrics: lyrics_of(tag),
        has_cover: !tag.pictures().is_empty(),
        writable,
    })
}

fn set_text(tag: &mut Tag, key: ItemKey, value: &Option<String>) {
    match value.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(v) => {
            tag.insert_text(key, v.to_string());
        }
        None => tag.remove_key(key),
    }
}

/// A number to set, or `None` to remove (zero counts as "none").
fn num(value: Option<u32>) -> Option<u32> {
    value.filter(|n| *n > 0)
}

/// Applies `edit` to `tag`. Returns an error for an unusable cover image.
fn apply(tag: &mut Tag, edit: &TagEdit) -> Result<()> {
    if let Some(v) = &edit.title {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => tag.set_title(t.to_string()),
            None => tag.remove_title(),
        }
    }
    if let Some(v) = &edit.artist {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => tag.set_artist(t.to_string()),
            None => tag.remove_artist(),
        }
    }
    if let Some(v) = &edit.album {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => tag.set_album(t.to_string()),
            None => tag.remove_album(),
        }
    }
    if let Some(v) = &edit.genre {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => tag.set_genre(t.to_string()),
            None => tag.remove_genre(),
        }
    }
    if let Some(v) = &edit.comment {
        match v.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => tag.set_comment(t.to_string()),
            None => tag.remove_comment(),
        }
    }
    if let Some(v) = &edit.album_artist {
        set_text(tag, ItemKey::AlbumArtist, v);
    }
    if let Some(v) = &edit.composer {
        set_text(tag, ItemKey::Composer, v);
    }
    if let Some(y) = edit.year {
        match y.filter(|y| *y > 0) {
            // Keep a full release date when only the year is being confirmed.
            Some(y) if tag.date().is_some_and(|d| d.year == y) => {}
            Some(y) => tag.set_date(Timestamp { year: y, ..Timestamp::default() }),
            None => tag.remove_date(),
        }
    }
    if let Some(v) = edit.track_no {
        match num(v) {
            Some(n) => tag.set_track(n),
            None => tag.remove_track(),
        }
    }
    if let Some(v) = edit.track_total {
        match num(v) {
            Some(n) => tag.set_track_total(n),
            None => tag.remove_track_total(),
        }
    }
    if let Some(v) = edit.disc_no {
        match num(v) {
            Some(n) => tag.set_disk(n),
            None => tag.remove_disk(),
        }
    }
    if let Some(v) = edit.disc_total {
        match num(v) {
            Some(n) => tag.set_disk_total(n),
            None => tag.remove_disk_total(),
        }
    }
    if let Some(v) = &edit.lyrics {
        let body = v.as_deref().map(|s| s.replace("\r\n", "\n")).filter(|s| !s.trim().is_empty());
        tag.remove_key(ItemKey::Lyrics);
        tag.remove_key(ItemKey::UnsyncLyrics);
        if let Some(body) = body {
            if body.len() as u64 > MAX_LYRICS_BYTES {
                return Err(Error::Other("those lyrics are too long to embed".into()));
            }
            // Vorbis comments and MP4 keep lyrics under one key, ID3 only knows the unsynchronised
            // frame (LRC text is stored in it as text, which most players understand).
            if !tag.insert_text(ItemKey::Lyrics, body.clone()) {
                tag.insert_text(ItemKey::UnsyncLyrics, body);
            }
        }
    }
    match &edit.cover {
        None => {}
        Some(CoverEdit::Remove) => {
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
        }
        Some(CoverEdit::File(p)) => {
            let pic = picture_from_file(p)?;
            while !tag.pictures().is_empty() {
                tag.remove_picture(0);
            }
            tag.push_picture(pic);
        }
    }
    Ok(())
}

fn picture_from_file(path: &Path) -> Result<Picture> {
    let len = fs::metadata(path)?.len();
    if len > MAX_COVER_BYTES {
        return Err(Error::Other("that picture is too large to embed (16 MB at most)".into()));
    }
    let data = fs::read(path)?;
    let mime = match data.as_slice() {
        [0xFF, 0xD8, 0xFF, ..] => MimeType::Jpeg,
        [0x89, b'P', b'N', b'G', ..] => MimeType::Png,
        _ => return Err(Error::Unsupported("covers must be JPEG or PNG pictures".into())),
    };
    Ok(Picture::unchecked(data).pic_type(PictureType::CoverFront).mime_type(mime).build())
}

/// A picture file shrunk for showing in the editor before it is saved. Anything but a JPEG or
/// PNG is refused, the same as when saving.
pub fn preview_image(path: &Path, max_px: u32) -> Result<meta::Cover> {
    let pic = picture_from_file(path)?;
    let full = meta::Cover { mime: pic.mime_type().map_or_else(|| "image/png".into(), |m| m.as_str().to_string()), data: pic.into_data() };
    Ok(crate::thumb::shrink(&full, max_px).unwrap_or(full))
}

/// Writes `edit` into the file's tags. The file is replaced only once the new copy is complete.
pub fn write_tags(path: &Path, edit: &TagEdit) -> Result<()> {
    if !meta::is_audio_file(path) {
        return Err(Error::Unsupported(format!("{} is not an audio file", path.display())));
    }
    if edit.is_empty() {
        return Ok(());
    }
    let original = fs::metadata(path)?;
    if original.permissions().readonly() {
        return Err(Error::Other("this file is read-only".into()));
    }
    let mut tagged = open_with_pictures(path)?;
    if tagged.primary_tag().is_none() && tagged.first_tag().is_none() {
        let tt = tagged.primary_tag_type();
        tagged.insert_tag(Tag::new(tt));
    }
    {
        let tag = match tagged.primary_tag_mut() {
            Some(t) => t,
            None => tagged.first_tag_mut().ok_or_else(|| Error::Other("this file can't hold tags".into()))?,
        };
        apply(tag, edit)?;
    }

    let dir = path.parent().ok_or_else(|| Error::Other("no folder".into()))?;
    let stem = path.file_stem().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let ext = path.extension().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    // Keeps the real extension so the copy is recognised as audio when it is checked below.
    let tmp = dir.join(format!(".{stem}.redshift-tmp.{ext}"));
    let result = (|| -> Result<()> {
        fs::copy(path, &tmp)?;
        tagged.save_to_path(&tmp, WriteOptions::default()).map_err(|e| Error::Other(format!("couldn't write the tags: {e}")))?;
        // A copy that can't be read back means something went wrong: keep the original.
        read_form(&tmp).map_err(|e| Error::Other(format!("the edited copy didn't check out: {e}")))?;
        fs::rename(&tmp, path).map_err(|e| Error::Other(format!("couldn't replace the file (is something else using it?): {e}")))?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

/// Saves lyrics as a `.lrc`/`.txt` file next to the song, replacing the one that's there. Returns
/// the file written.
pub fn write_sidecar_lyrics(audio: &Path, text: &str, synced: bool) -> Result<PathBuf> {
    if !meta::is_audio_file(audio) {
        return Err(Error::Unsupported("not an audio file".into()));
    }
    if text.len() as u64 > MAX_LYRICS_BYTES {
        return Err(Error::Other("those lyrics are too long".into()));
    }
    let existing = crate::lyrics::sidecar_path(audio);
    let dest = match existing {
        Some(p) if p.extension().is_some_and(|e| e.eq_ignore_ascii_case(if synced { "lrc" } else { "txt" })) => p,
        _ => audio.with_extension(if synced { "lrc" } else { "txt" }),
    };
    let tmp = dest.with_extension("redshift-tmp");
    fs::write(&tmp, text.replace("\r\n", "\n"))?;
    fs::rename(&tmp, &dest).inspect_err(|_| {
        let _ = fs::remove_file(&tmp);
    })?;
    Ok(dest)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)
    }
    fn copy_of(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join(name);
        fs::copy(fixture(name), &p).unwrap();
        (dir, p)
    }
    fn edit(json: &str) -> TagEdit {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn missing_null_and_value_mean_keep_clear_and_set() {
        let e = edit(r#"{"title":"New","album":null}"#);
        assert_eq!(e.title, Some(Some("New".into())));
        assert_eq!(e.album, Some(None));
        assert_eq!(e.artist, None);
        assert!(edit("{}").is_empty());
        assert!(!e.is_empty());
    }

    #[test]
    fn reads_the_form_of_a_tagged_file() {
        let f = read_form(&fixture("tagged.flac")).unwrap();
        assert_eq!((f.title.as_str(), f.artist.as_str(), f.album.as_str()), ("Tagged Song", "The Testers", "Fixture Album"));
        assert_eq!((f.year, f.track_no, f.disc_no), (Some(2021), Some(3), Some(2)));
        assert!(f.has_cover && f.lyrics.contains("[00:01.00]First line"));
        let u = read_form(&fixture("Untagged Name.flac")).unwrap();
        assert_eq!(u.title, "", "no fallback to the file name here");
        assert!(read_form(&fixture("nope.flac")).is_err());
    }

    #[test]
    fn edits_round_trip_in_flac_opus_and_mp3() {
        for name in ["tagged.flac", "tagged.opus", "tagged.mp3"] {
            let (_d, p) = copy_of(name);
            write_tags(
                &p,
                &edit(r#"{"title":"Renamed","artist":"Someone Else","album":"Brand New","album_artist":"Various","genre":"Jazz","composer":"C. Omposer","comment":"hello","year":1987,"track_no":9,"track_total":12,"disc_no":1,"disc_total":2}"#),
            )
            .unwrap();
            let f = read_form(&p).unwrap();
            assert_eq!((f.title.as_str(), f.artist.as_str(), f.album.as_str()), ("Renamed", "Someone Else", "Brand New"), "{name}");
            assert_eq!((f.album_artist.as_str(), f.genre.as_str(), f.composer.as_str()), ("Various", "Jazz", "C. Omposer"), "{name}");
            assert_eq!((f.year, f.track_no, f.track_total, f.disc_no, f.disc_total), (Some(1987), Some(9), Some(12), Some(1), Some(2)), "{name}");
            // The library's reader agrees.
            let m = meta::read_meta(&p).unwrap();
            assert_eq!((m.title.as_str(), m.album.as_str(), m.year, m.track_no), ("Renamed", "Brand New", Some(1987), Some(9)), "{name}");
            assert!((m.duration_secs - meta::read_meta(&fixture(name)).unwrap().duration_secs).abs() < 0.05, "audio untouched: {name}");
        }
    }

    #[test]
    fn untouched_fields_stay_and_null_clears() {
        let (_d, p) = copy_of("tagged.flac");
        write_tags(&p, &edit(r#"{"title":"Only this","genre":null,"year":null}"#)).unwrap();
        let f = read_form(&p).unwrap();
        assert_eq!(f.title, "Only this");
        assert_eq!((f.artist.as_str(), f.album.as_str()), ("The Testers", "Fixture Album"));
        assert_eq!((f.genre.as_str(), f.year), ("", None));
        assert!(f.has_cover && f.lyrics.contains("First line"), "cover and lyrics weren't mentioned");
    }

    #[test]
    fn confirming_the_same_year_keeps_the_full_date() {
        let (_d, p) = copy_of("tagged.flac");
        write_tags(&p, &edit(r#"{"year":2021}"#)).unwrap();
        assert_eq!(read_form(&p).unwrap().year, Some(2021));
    }

    #[test]
    fn lyrics_are_embedded_replaced_and_removed() {
        for name in ["tagged.flac", "tagged.opus", "tagged.mp3"] {
            let (_d, p) = copy_of(name);
            let lrc = "[00:01.00]one\n[00:04.50]two\n";
            write_tags(&p, &TagEdit { lyrics: Some(Some(lrc.into())), ..TagEdit::default() }).unwrap();
            assert_eq!(read_form(&p).unwrap().lyrics, lrc.trim(), "{name}");
            let l = crate::lyrics::find_lyrics(&p).expect(name);
            assert!(l.synced && l.lines.len() == 2, "{name}: shows up as synced lyrics");
            assert!(meta::read_meta(&p).unwrap().has_embedded_lyrics);
            write_tags(&p, &TagEdit { lyrics: Some(Some("plain words\nmore".into())), ..TagEdit::default() }).unwrap();
            assert!(!crate::lyrics::find_lyrics(&p).unwrap().synced, "{name}");
            write_tags(&p, &TagEdit { lyrics: Some(None), ..TagEdit::default() }).unwrap();
            assert_eq!(read_form(&p).unwrap().lyrics, "", "{name}");
            assert!(!meta::read_meta(&p).unwrap().has_embedded_lyrics);
        }
    }

    #[test]
    fn cover_can_be_replaced_and_removed() {
        let (d, p) = copy_of("tagged.flac");
        let png = {
            let src = meta::read_cover(&fixture("tagged.flac")).unwrap();
            let f = d.path().join("new.png");
            fs::write(&f, [&src.data[..], b""].concat()).unwrap();
            f
        };
        let before = meta::read_cover(&p).unwrap();
        write_tags(&p, &TagEdit { cover: Some(CoverEdit::File(png.clone())), ..TagEdit::default() }).unwrap();
        assert_eq!(meta::read_cover(&p).unwrap().data, before.data);

        let jpg = d.path().join("c.jpg");
        fs::write(&jpg, b"\xFF\xD8\xFFnot really a jpeg").unwrap();
        write_tags(&p, &TagEdit { cover: Some(CoverEdit::File(jpg)), ..TagEdit::default() }).unwrap();
        let c = meta::read_cover(&p).unwrap();
        assert_eq!(c.mime, "image/jpeg");
        assert!(c.data.starts_with(b"\xFF\xD8\xFFnot"));

        write_tags(&p, &TagEdit { cover: Some(CoverEdit::Remove), ..TagEdit::default() }).unwrap();
        assert!(!read_form(&p).unwrap().has_cover);
        assert!(meta::read_cover(&p).is_none());
    }

    #[test]
    fn bad_covers_and_files_are_refused_without_touching_the_song() {
        let (d, p) = copy_of("tagged.flac");
        let before = fs::read(&p).unwrap();
        let txt = d.path().join("c.png");
        fs::write(&txt, b"definitely not an image").unwrap();
        assert!(write_tags(&p, &TagEdit { cover: Some(CoverEdit::File(txt)), ..TagEdit::default() }).is_err());
        assert!(write_tags(&p, &TagEdit { cover: Some(CoverEdit::File(d.path().join("missing.png"))), ..TagEdit::default() }).is_err());
        assert_eq!(fs::read(&p).unwrap(), before);
        assert!(!d.path().join(".tagged.redshift-tmp.flac").exists(), "no leftovers");

        let notes = d.path().join("notes.txt");
        fs::write(&notes, "hi").unwrap();
        assert!(write_tags(&notes, &edit(r#"{"title":"x"}"#)).is_err());
        assert!(write_tags(&d.path().join("gone.flac"), &edit(r#"{"title":"x"}"#)).is_err());
    }

    #[test]
    fn untagged_files_get_a_tag() {
        let (_d, p) = copy_of("Untagged Name.flac");
        write_tags(&p, &edit(r#"{"title":"Fresh","artist":"New Artist"}"#)).unwrap();
        let f = read_form(&p).unwrap();
        assert_eq!((f.title.as_str(), f.artist.as_str()), ("Fresh", "New Artist"));
    }

    #[test]
    fn sidecar_lyrics_replace_the_existing_file() {
        let dir = tempfile::tempdir().unwrap();
        let song = dir.path().join("Song.flac");
        fs::copy(fixture("Untagged Name.flac"), &song).unwrap();
        let p = write_sidecar_lyrics(&song, "[00:01.00]hi\r\n", true).unwrap();
        assert_eq!(p.file_name().unwrap(), "Song.lrc");
        assert_eq!(fs::read_to_string(&p).unwrap(), "[00:01.00]hi\n");
        write_sidecar_lyrics(&song, "[00:02.00]again", true).unwrap();
        assert_eq!(fs::read_to_string(&p).unwrap(), "[00:02.00]again");
        assert!(crate::lyrics::find_lyrics(&song).unwrap().synced);
        assert!(write_sidecar_lyrics(&dir.path().join("a.txt"), "x", false).is_err());
    }
}
