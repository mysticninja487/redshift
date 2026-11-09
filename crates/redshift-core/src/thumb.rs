//! Small copies of cover art for lists, grids and the queue. Embedded covers are often several
//! megabytes; a 40-pixel tile doesn't need that, and the window would decode every one of them.

use std::io::Cursor;

use image::codecs::jpeg::JpegEncoder;
use image::codecs::png::PngEncoder;
use image::imageops::FilterType;
use image::{ExtendedColorType, ImageEncoder, ImageReader};

use crate::meta::Cover;

/// A copy of `cover` that fits in `max_px` by `max_px`, or `None` when it already does (use the
/// original) or can't be read as an image (use the original; the window will show or reject it).
pub fn shrink(cover: &Cover, max_px: u32) -> Option<Cover> {
    let max_px = max_px.max(16);
    let reader = ImageReader::new(Cursor::new(&cover.data)).with_guessed_format().ok()?;
    let (w, h) = reader.into_dimensions().ok()?;
    if w.max(h) <= max_px {
        return None;
    }
    // Refuse absurd sizes rather than allocate gigabytes for one tile.
    if u64::from(w) * u64::from(h) > 100_000_000 {
        return None;
    }
    let img = ImageReader::new(Cursor::new(&cover.data)).with_guessed_format().ok()?.decode().ok()?;
    let small = img.resize(max_px, max_px, FilterType::Triangle);
    let mut out = Vec::new();
    let mime = if small.color().has_alpha() {
        let rgba = small.to_rgba8();
        PngEncoder::new(&mut out).write_image(rgba.as_raw(), rgba.width(), rgba.height(), ExtendedColorType::Rgba8).ok()?;
        "image/png"
    } else {
        let rgb = small.to_rgb8();
        JpegEncoder::new_with_quality(&mut out, 84).write_image(rgb.as_raw(), rgb.width(), rgb.height(), ExtendedColorType::Rgb8).ok()?;
        "image/jpeg"
    };
    // A flat, highly compressed picture can come out bigger than it went in; then the original is the better copy.
    if out.len() >= cover.data.len() {
        return None;
    }
    Some(Cover { mime: mime.into(), data: out })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, alpha: bool) -> Cover {
        let mut data = Vec::new();
        if alpha {
            let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 13) as u8, 90, 200]).collect();
            PngEncoder::new(&mut data).write_image(&px, w, h, ExtendedColorType::Rgba8).unwrap();
        } else {
            let px: Vec<u8> = (0..w * h).flat_map(|i| [(i % 251) as u8, (i % 13) as u8, 90]).collect();
            PngEncoder::new(&mut data).write_image(&px, w, h, ExtendedColorType::Rgb8).unwrap();
        }
        Cover { mime: "image/png".into(), data }
    }

    fn dims(c: &Cover) -> (u32, u32) {
        ImageReader::new(Cursor::new(&c.data)).with_guessed_format().unwrap().into_dimensions().unwrap()
    }

    #[test]
    fn a_big_cover_shrinks_to_fit_and_keeps_its_proportions() {
        let big = png(1200, 800, false);
        let small = shrink(&big, 300).expect("shrunk");
        assert_eq!(dims(&small), (300, 200));
        assert_eq!(small.mime, "image/jpeg");
        assert!(small.data.len() < big.data.len());
    }

    #[test]
    fn a_copy_is_never_bigger_than_the_original() {
        // A solid-colour PNG is tiny already; a JPEG of it at 300 px would not be.
        let mut data = Vec::new();
        PngEncoder::new(&mut data).write_image(&vec![40u8; 600 * 600 * 3], 600, 600, ExtendedColorType::Rgb8).unwrap();
        let flat = Cover { mime: "image/png".into(), data };
        if let Some(small) = shrink(&flat, 300) { assert!(small.data.len() < flat.data.len()); }
    }

    #[test]
    fn a_cover_with_transparency_stays_a_png() {
        let small = shrink(&png(400, 400, true), 100).expect("shrunk");
        assert_eq!((small.mime.as_str(), dims(&small)), ("image/png", (100, 100)));
    }

    #[test]
    fn a_cover_that_already_fits_or_isnt_an_image_is_left_alone() {
        assert!(shrink(&png(64, 64, false), 128).is_none());
        assert!(shrink(&Cover { mime: "image/jpeg".into(), data: b"not an image".to_vec() }, 128).is_none());
    }
}
