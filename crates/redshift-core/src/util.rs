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

/// An address a stream is played from, as opposed to a file on disk. Streams ride through the queue
/// and the player as a "path" that is the address itself.
pub fn is_url(path: &std::path::Path) -> bool {
    path.to_str().is_some_and(|s| {
        let s = s.trim_start();
        s.len() > 8 && (s[..7].eq_ignore_ascii_case("http://") || s[..8].eq_ignore_ascii_case("https://"))
    })
}
