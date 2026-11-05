//! Fonts the person imported, kept in `<data dir>/fonts` and served to the UI by file name.

use std::fs;
use std::path::Path;

use serde::Serialize;

use crate::error::{Error, Result};

const MAX_FONT_BYTES: u64 = 25 << 20;
const MAX_FONTS: usize = 100;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FontEntry {
    /// File name inside the fonts folder.
    pub file: String,
    /// Name to show and to use as the CSS family, from the file name.
    pub family: String,
}

fn mime_of(file: &str) -> Option<&'static str> {
    match Path::new(file).extension()?.to_str()?.to_ascii_lowercase().as_str() {
        "ttf" => Some("font/ttf"),
        "otf" => Some("font/otf"),
        "woff" => Some("font/woff"),
        "woff2" => Some("font/woff2"),
        _ => None,
    }
}

/// A bare file name with nothing that could climb out of the folder.
fn safe_name(file: &str) -> bool {
    !file.is_empty() && !file.contains(['/', '\\', ':', '\0']) && file != "." && file != ".." && !file.starts_with('.')
}

fn entry(file: String) -> FontEntry {
    let family = Path::new(&file).file_stem().map(|s| s.to_string_lossy().replace(['_', '-'], " ")).unwrap_or_default();
    FontEntry { file, family: family.trim().to_owned() }
}

pub fn list(dir: &Path) -> Vec<FontEntry> {
    let mut v: Vec<FontEntry> = fs::read_dir(dir)
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter_map(|e| e.file_name().into_string().ok())
        .filter(|n| safe_name(n) && mime_of(n).is_some())
        .map(entry)
        .collect();
    v.sort_by(|a, b| a.family.to_lowercase().cmp(&b.family.to_lowercase()).then_with(|| a.file.cmp(&b.file)));
    v
}

/// Copies a .ttf/.otf/.woff/.woff2 file into the fonts folder. Importing a file with the same
/// name again replaces it.
pub fn import(dir: &Path, src: &Path) -> Result<FontEntry> {
    let name = src.file_name().and_then(|n| n.to_str()).ok_or_else(|| Error::Other("that isn't a font file".into()))?;
    if !safe_name(name) || mime_of(name).is_none() {
        return Err(Error::Unsupported("fonts must be .ttf, .otf, .woff or .woff2 files".into()));
    }
    let meta = fs::metadata(src)?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_FONT_BYTES {
        return Err(Error::Other("that font file is empty or too large (the limit is 25 MB)".into()));
    }
    fs::create_dir_all(dir)?;
    if list(dir).len() >= MAX_FONTS && !dir.join(name).exists() {
        return Err(Error::Other(format!("at most {MAX_FONTS} imported fonts; remove some first")));
    }
    fs::copy(src, dir.join(name))?;
    Ok(entry(name.to_owned()))
}

pub fn remove(dir: &Path, file: &str) -> bool {
    safe_name(file) && mime_of(file).is_some() && fs::remove_file(dir.join(file)).is_ok()
}

/// The bytes and media type of an imported font.
pub fn read(dir: &Path, file: &str) -> Option<(Vec<u8>, &'static str)> {
    if !safe_name(file) {
        return None;
    }
    let mime = mime_of(file)?;
    fs::read(dir.join(file)).ok().map(|b| (b, mime))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn import_list_read_remove() {
        let d = tempfile::tempdir().unwrap();
        let fonts = d.path().join("fonts");
        let src = d.path().join("Space_Grotesk-Bold.ttf");
        fs::write(&src, b"fontbytes").unwrap();
        let e = import(&fonts, &src).unwrap();
        assert_eq!((e.file.as_str(), e.family.as_str()), ("Space_Grotesk-Bold.ttf", "Space Grotesk Bold"));
        assert_eq!(list(&fonts), vec![e.clone()]);
        assert_eq!(read(&fonts, &e.file), Some((b"fontbytes".to_vec(), "font/ttf")));
        assert!(remove(&fonts, &e.file));
        assert!(list(&fonts).is_empty());
        assert!(!remove(&fonts, &e.file));
    }

    #[test]
    fn rejects_other_files_empty_files_and_path_tricks() {
        let d = tempfile::tempdir().unwrap();
        let fonts = d.path().join("fonts");
        let txt = d.path().join("a.txt");
        fs::write(&txt, b"x").unwrap();
        assert!(import(&fonts, &txt).is_err());
        let empty = d.path().join("e.otf");
        fs::write(&empty, b"").unwrap();
        assert!(import(&fonts, &empty).is_err());
        fs::write(d.path().join("secret.ttf"), b"s").unwrap();
        assert!(read(&fonts, "../secret.ttf").is_none());
        assert!(read(&fonts, "..\\secret.ttf").is_none());
        assert!(!remove(&fonts, "../secret.ttf"));
        assert!(d.path().join("secret.ttf").exists());
    }
}
