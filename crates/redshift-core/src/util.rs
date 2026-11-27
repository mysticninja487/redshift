//! Small helpers shared by several modules.

/// Decodes `%XX` escapes (as in URLs). Malformed escapes are kept literally, and bytes that don't
/// form valid UTF-8 become replacement characters.
pub fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            // Work on bytes, not str slices, so a multi-byte character after '%' can't panic.
            if let (Some(hi), Some(lo)) = ((b[i + 1] as char).to_digit(16), (b[i + 2] as char).to_digit(16)) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The queue "path" of a song that lives on the person's own Navidrome or Subsonic server:
/// `subsonic://` and the server's id for the song. The address, user and password stay in the
/// server settings, so a saved queue never holds them.
pub const SERVER_SONG_PREFIX: &str = "subsonic://";

/// The server's id for a song path made by [`server_song_path`].
pub fn server_song_id(path: &std::path::Path) -> Option<String> {
    let s = path.to_str()?.trim();
    (s.len() > SERVER_SONG_PREFIX.len() && s[..SERVER_SONG_PREFIX.len()].eq_ignore_ascii_case(SERVER_SONG_PREFIX))
        .then(|| s[SERVER_SONG_PREFIX.len()..].to_string())
}

pub fn server_song_path(id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{SERVER_SONG_PREFIX}{id}"))
}

/// A picture on the server (an album's cover) addressed the way the front end asks for covers: by a path.
pub const SERVER_COVER_PREFIX: &str = "subsonic-cover://";

pub fn server_cover_path(cover_id: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{SERVER_COVER_PREFIX}{cover_id}"))
}

/// The server's cover id in a path made by [`server_cover_path`].
pub fn server_cover_id(path: &std::path::Path) -> Option<String> {
    let s = path.to_str()?.trim();
    (s.len() > SERVER_COVER_PREFIX.len() && s[..SERVER_COVER_PREFIX.len()].eq_ignore_ascii_case(SERVER_COVER_PREFIX)).then(|| s[SERVER_COVER_PREFIX.len()..].to_string())
}

/// The queue "path" of a file in an item on the Internet Archive: `archive://`, the item's
/// identifier, `/` and the file's name inside the item (which may itself hold `/`).
pub const ARCHIVE_SONG_PREFIX: &str = "archive://";

pub fn archive_song_path(identifier: &str, file: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{ARCHIVE_SONG_PREFIX}{identifier}/{file}"))
}

/// The item identifier and file name in a path made by [`archive_song_path`].
pub fn archive_song_parts(path: &std::path::Path) -> Option<(String, String)> {
    let s = path.to_str()?.trim();
    if s.len() <= ARCHIVE_SONG_PREFIX.len() || !s[..ARCHIVE_SONG_PREFIX.len()].eq_ignore_ascii_case(ARCHIVE_SONG_PREFIX) {
        return None;
    }
    let (id, file) = s[ARCHIVE_SONG_PREFIX.len()..].split_once('/')?;
    (!id.is_empty() && !file.is_empty()).then(|| (id.to_string(), file.to_string()))
}

/// An item's picture, addressed the way the front end asks for covers: by a path.
pub const ARCHIVE_COVER_PREFIX: &str = "archive-cover://";

pub fn archive_cover_path(identifier: &str) -> std::path::PathBuf {
    std::path::PathBuf::from(format!("{ARCHIVE_COVER_PREFIX}{identifier}"))
}

pub fn archive_cover_id(path: &std::path::Path) -> Option<String> {
    let s = path.to_str()?.trim();
    (s.len() > ARCHIVE_COVER_PREFIX.len() && s[..ARCHIVE_COVER_PREFIX.len()].eq_ignore_ascii_case(ARCHIVE_COVER_PREFIX)).then(|| s[ARCHIVE_COVER_PREFIX.len()..].to_string())
}

/// A song that is fetched from somewhere else in pieces and plays like a file: one on the person's
/// own server or one from the Internet Archive.
pub fn is_remote_song(path: &std::path::Path) -> bool {
    server_song_id(path).is_some() || archive_song_parts(path).is_some()
}

/// An address a stream is played from, as opposed to a file on disk. Streams ride through the queue
/// and the player as a "path" that is the address itself.
pub fn is_url(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|s| {
        let s = s.trim_start();
        s.len() > 8 && (s[..7].eq_ignore_ascii_case("http://") || s[..8].eq_ignore_ascii_case("https://"))
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn archive_paths_round_trip() {
        let p = archive_song_path("gd77-05-08", "Disc 1/01 Minglewood Blues.flac");
        assert_eq!(p.to_str().unwrap(), "archive://gd77-05-08/Disc 1/01 Minglewood Blues.flac");
        assert_eq!(archive_song_parts(&p), Some(("gd77-05-08".into(), "Disc 1/01 Minglewood Blues.flac".into())));
        assert_eq!(archive_song_parts(Path::new("ARCHIVE://x/y.mp3")), Some(("x".into(), "y.mp3".into())));
        assert_eq!(archive_song_parts(Path::new("archive://onlyid")), None);
        assert_eq!(archive_song_parts(Path::new("archive:///file.mp3")), None);
        assert_eq!(archive_song_parts(Path::new("subsonic://5")), None);
        assert!(is_remote_song(Path::new("subsonic://5")) && is_remote_song(&p) && !is_remote_song(Path::new("C:/a.flac")));
        assert_eq!(archive_cover_id(&archive_cover_path("abc")), Some("abc".into()));
    }
}
