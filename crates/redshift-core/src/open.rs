//! Turns whatever the user handed the player (files, folders, playlists, dropped from Explorer or
//! passed on the command line) into the list of songs to play.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

use crate::meta::is_audio_file;
use crate::playlists::{decode_m3u, parse_m3u};

fn is_playlist(path: &Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("m3u") || e.eq_ignore_ascii_case("m3u8"))
}

/// Expands `inputs` into songs, in the order given:
/// - an audio file is itself,
/// - a folder is every song inside it (sub-folders too), in name order,
/// - an `.m3u`/`.m3u8` playlist is the songs it lists that exist.
/// Anything else (and anything missing) is skipped, and a song is only listed once.
pub fn expand(inputs: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut push = |p: PathBuf, out: &mut Vec<PathBuf>| {
        if seen.insert(p.clone()) {
            out.push(p);
        }
    };
    for input in inputs {
        if input.is_dir() {
            for e in WalkDir::new(input).sort_by_file_name().into_iter().filter_map(|e| e.ok()) {
                if e.file_type().is_file() && is_audio_file(e.path()) {
                    push(e.into_path(), &mut out);
                }
            }
        } else if is_playlist(input) {
            let Ok(bytes) = std::fs::read(input) else { continue };
            let base = input.parent().unwrap_or(Path::new("."));
            for p in parse_m3u(&decode_m3u(&bytes), base) {
                if p.is_file() && is_audio_file(&p) {
                    push(p, &mut out);
                }
            }
        } else if input.is_file() && is_audio_file(input) {
            push(input.clone(), &mut out);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(p: &Path) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, b"x").unwrap();
    }

    #[test]
    fn files_folders_and_playlists_become_one_list() {
        let dir = tempfile::tempdir().unwrap();
        let d = dir.path();
        touch(&d.join("album/02 b.flac"));
        touch(&d.join("album/01 a.mp3"));
        touch(&d.join("album/cover.jpg"));
        touch(&d.join("album/disc2/03 c.opus"));
        touch(&d.join("single.flac"));
        fs::write(d.join("mix.m3u8"), "#EXTM3U\nsingle.flac\nalbum/02 b.flac\nmissing.flac\nnotes.txt\n").unwrap();

        // a folder: songs only, in name order, sub-folders included
        let got = expand(&[d.join("album")]);
        assert_eq!(got, vec![d.join("album/01 a.mp3"), d.join("album/02 b.flac"), d.join("album/disc2/03 c.opus")]);

        // a playlist: only the songs that exist, in the playlist's order
        assert_eq!(expand(&[d.join("mix.m3u8")]), vec![d.join("single.flac"), d.join("album/02 b.flac")]);

        // a mix: given order kept, nothing listed twice, non-audio and missing paths skipped
        let got = expand(&[d.join("single.flac"), d.join("album/cover.jpg"), d.join("nope.flac"), d.join("mix.m3u8"), d.join("single.flac")]);
        assert_eq!(got, vec![d.join("single.flac"), d.join("album/02 b.flac")]);
    }
}
