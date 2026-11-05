//! Finds library files for playlist entries whose paths don't exist on this machine.
//!
//! Playlists made on a phone, another drive or another PC point at paths that are gone here
//! (`/storage/emulated/0/Music/a.opus`, `D:\Music\a.opus`). Rather than failing on every entry, each
//! one is matched against the library, from strictest to loosest:
//!
//! 1. the path exists on disk (kept as it is);
//! 2. the file name matches, ignoring case, preferring the candidate whose parent folders also
//!    match the entry's;
//! 3. the name matches once everything but letters and digits (and the extension) is ignored, which
//!    undoes Windows' habit of turning `?`, `:` and so on into `_`;
//! 4. the `#EXTINF` label ("Artist - Title") matches a library track's tags.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Component, Path, PathBuf};

use crate::meta::TrackMeta;

/// Lower-case letters and digits only, so `Don't Stop: Me?.opus` and `don_t stop_ me_.opus` agree.
fn loose(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

fn file_name(p: &Path) -> Option<String> {
    p.file_name().map(|n| n.to_string_lossy().to_lowercase())
}

fn stem_key(p: &Path) -> Option<String> {
    p.file_stem().map(|n| loose(&n.to_string_lossy())).filter(|k| !k.is_empty())
}

/// Folder names from the file upwards, loosened, for comparing how much of two paths agree.
fn parents_rev(p: &Path) -> Vec<String> {
    let mut v: Vec<String> = p
        .components()
        .filter_map(|c| match c {
            Component::Normal(n) => Some(loose(&n.to_string_lossy())),
            _ => None,
        })
        .collect();
    v.pop(); // the file name itself
    v.reverse();
    v
}

pub struct Resolver {
    exact: HashSet<PathBuf>,
    by_name: HashMap<String, Vec<PathBuf>>,
    by_stem: HashMap<String, Vec<PathBuf>>,
    by_label: HashMap<String, Vec<PathBuf>>,
}

impl Resolver {
    pub fn new<'a>(tracks: impl IntoIterator<Item = &'a TrackMeta>) -> Self {
        let mut r = Resolver { exact: HashSet::new(), by_name: HashMap::new(), by_stem: HashMap::new(), by_label: HashMap::new() };
        for t in tracks {
            r.exact.insert(t.path.clone());
            if let Some(n) = file_name(&t.path) {
                r.by_name.entry(n).or_default().push(t.path.clone());
            }
            if let Some(k) = stem_key(&t.path) {
                r.by_stem.entry(k).or_default().push(t.path.clone());
            }
            let label = format!("{}{}", loose(&t.artist), loose(&t.title));
            if !t.title.is_empty() && !label.is_empty() {
                r.by_label.entry(label).or_default().push(t.path.clone());
            }
        }
        for v in r.by_name.values_mut().chain(r.by_stem.values_mut()).chain(r.by_label.values_mut()) {
            v.sort();
        }
        r
    }

    /// The file to use for `entry`, or `None` if nothing on this machine matches. An entry that
    /// already exists is returned unchanged.
    pub fn resolve(&self, entry: &Path, label: Option<&str>) -> Option<PathBuf> {
        if self.exact.contains(entry) || fs::metadata(entry).is_ok() {
            return Some(entry.to_path_buf());
        }
        let want = parents_rev(entry);
        let best = |c: &Vec<PathBuf>| -> PathBuf {
            // Most agreeing parent folders wins; ties go to the first in sorted order.
            let score = |p: &PathBuf| parents_rev(p).iter().zip(&want).take_while(|(a, b)| a == b).count();
            c.iter().max_by_key(|p| (score(p), std::cmp::Reverse((*p).clone()))).cloned().unwrap()
        };
        if let Some(c) = file_name(entry).and_then(|n| self.by_name.get(&n)) {
            return Some(best(c));
        }
        if let Some(c) = stem_key(entry).and_then(|k| self.by_stem.get(&k)) {
            return Some(best(c));
        }
        if let Some(l) = label {
            let k = loose(l);
            if !k.is_empty() {
                if let Some(c) = self.by_label.get(&k) {
                    return Some(best(c));
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(path: &str, artist: &str, title: &str) -> TrackMeta {
        TrackMeta { path: PathBuf::from(path), artist: artist.into(), title: title.into(), ..Default::default() }
    }

    fn lib() -> Resolver {
        Resolver::new(&[
            track("/m/Artist A/Album 1/01 Song.opus", "Artist A", "Song"),
            track("/m/Artist B/Album 2/01 Song.opus", "Artist B", "Song"),
            track("/m/Artist C/Why_ Not.flac", "Artist C", "Why? Not"),
            track("/m/Misc/other name.mp3", "Someone", "Real Title"),
        ])
    }

    #[test]
    fn library_paths_are_kept() {
        assert_eq!(lib().resolve(Path::new("/m/Misc/other name.mp3"), None), Some(PathBuf::from("/m/Misc/other name.mp3")));
    }

    #[test]
    fn matches_by_file_name_preferring_matching_folders() {
        let r = lib();
        let got = r.resolve(Path::new("/storage/emulated/0/Music/Artist B/Album 2/01 song.opus"), None);
        assert_eq!(got, Some(PathBuf::from("/m/Artist B/Album 2/01 Song.opus")));
        let got = r.resolve(Path::new("D:/Music/Artist A/Album 1/01 Song.opus"), None);
        assert_eq!(got, Some(PathBuf::from("/m/Artist A/Album 1/01 Song.opus")));
    }

    #[test]
    fn sanitised_characters_and_other_extensions_still_match() {
        let r = lib();
        assert_eq!(r.resolve(Path::new("/x/Why? Not.opus"), None), Some(PathBuf::from("/m/Artist C/Why_ Not.flac")));
    }

    #[test]
    fn falls_back_to_the_extinf_label() {
        let r = lib();
        assert_eq!(r.resolve(Path::new("/x/track07.opus"), Some("Someone - Real Title")), Some(PathBuf::from("/m/Misc/other name.mp3")));
        assert_eq!(r.resolve(Path::new("/x/track07.opus"), Some("Nobody - Nothing")), None);
        assert_eq!(r.resolve(Path::new("/x/track07.opus"), None), None);
    }
}
