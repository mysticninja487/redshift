//! Encoder delay and padding for MP4 audio (`.m4a`, AAC and ALAC).
//!
//! AAC encoders start with a few hundred to a couple of thousand frames of priming and pad the end
//! to a whole packet, so a decoded file is 25-50 ms longer than the song, with silence at both
//! ends. MP3 and Vorbis name those frames in their packets and Symphonia trims them; its MP4
//! reader does not. The container does say what to keep, though: the track's edit list holds the
//! first valid frame (`media_time`) and how long the valid stretch is. This reads that, so the
//! decoder can cut the same frames and consecutive AAC songs join without a gap.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// Which part of the decoded stream is the song, in frames at the stream's sample rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Trim {
    /// Frames to drop from the start (the encoder's priming).
    pub delay: u64,
    /// Frames of music after that. Everything past it is padding.
    pub valid: u64,
}

/// The trim for the first audio track of an MP4 file, or `None` if there is nothing to cut or the
/// file is not an MP4 or has no usable edit list.
pub fn read_trim(path: &Path, sample_rate: u32) -> Option<Trim> {
    let mut f = File::open(path).ok()?;
    let len = f.metadata().ok()?.len();
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if &head[4..8] != b"ftyp" {
        return None;
    }
    // Top-level boxes: step over `mdat` by its size and read only `moov`.
    let mut pos = 0u64;
    while pos + 8 <= len {
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut h = [0u8; 8];
        f.read_exact(&mut h).ok()?;
        let mut size = u32::from_be_bytes(h[..4].try_into().ok()?) as u64;
        let mut header = 8;
        if size == 1 {
            let mut big = [0u8; 8];
            f.read_exact(&mut big).ok()?;
            size = u64::from_be_bytes(big);
            header = 16;
        } else if size == 0 {
            size = len - pos;
        }
        if size < header {
            return None;
        }
        if &h[4..8] == b"moov" {
            if size > 64 << 20 {
                return None; // an index this large is not a normal file
            }
            let mut body = vec![0u8; (size - header) as usize];
            f.read_exact(&mut body).ok()?;
            return trim_from_moov(&body, sample_rate);
        }
        pos += size;
    }
    None
}

/// Child boxes of `data` as (type, body).
fn children(data: &[u8]) -> Vec<([u8; 4], &[u8])> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + 8 <= data.len() {
        let mut size = u32::from_be_bytes(data[i..i + 4].try_into().unwrap()) as usize;
        let ty: [u8; 4] = data[i + 4..i + 8].try_into().unwrap();
        let mut header = 8;
        if size == 1 && i + 16 <= data.len() {
            size = u64::from_be_bytes(data[i + 8..i + 16].try_into().unwrap()) as usize;
            header = 16;
        } else if size == 0 {
            size = data.len() - i;
        }
        if size < header || i + size > data.len() {
            break;
        }
        out.push((ty, &data[i + header..i + size]));
        i += size;
    }
    out
}

fn find<'a>(list: &[([u8; 4], &'a [u8])], ty: &[u8; 4]) -> Option<&'a [u8]> {
    list.iter().find(|(t, _)| t == ty).map(|(_, b)| *b)
}

/// (timescale, duration) of an `mvhd` or `mdhd` box body.
fn time_header(b: &[u8]) -> Option<(u64, u64)> {
    match *b.first()? {
        0 if b.len() >= 20 => Some((u32::from_be_bytes(b[12..16].try_into().ok()?) as u64, u32::from_be_bytes(b[16..20].try_into().ok()?) as u64)),
        1 if b.len() >= 32 => Some((u32::from_be_bytes(b[20..24].try_into().ok()?) as u64, u64::from_be_bytes(b[24..32].try_into().ok()?))),
        _ => None,
    }
}

fn trim_from_moov(moov: &[u8], sample_rate: u32) -> Option<Trim> {
    let top = children(moov);
    let (movie_scale, _) = time_header(find(&top, b"mvhd")?)?;
    for (ty, trak) in &top {
        if ty != b"trak" {
            continue;
        }
        let parts = children(trak);
        let mdia = children(find(&parts, b"mdia")?);
        // Only the sound track.
        let hdlr = find(&mdia, b"hdlr")?;
        if hdlr.get(8..12) != Some(b"soun".as_slice()) {
            continue;
        }
        let (media_scale, media_dur) = time_header(find(&mdia, b"mdhd")?)?;
        let elst = find(&children(find(&parts, b"edts")?), b"elst")?;
        return trim_from_elst(elst, movie_scale, media_scale, media_dur, sample_rate);
    }
    None
}

fn trim_from_elst(elst: &[u8], movie_scale: u64, media_scale: u64, media_dur: u64, sample_rate: u32) -> Option<Trim> {
    if movie_scale == 0 || media_scale == 0 {
        return None;
    }
    let version = *elst.first()?;
    let count = u32::from_be_bytes(elst.get(4..8)?.try_into().ok()?) as usize;
    let stride = if version == 1 { 20 } else { 12 };
    let mut o = 8;
    for _ in 0..count {
        let e = elst.get(o..o + stride)?;
        o += stride;
        let (seg, media_time) = if version == 1 {
            (u64::from_be_bytes(e[0..8].try_into().ok()?), i64::from_be_bytes(e[8..16].try_into().ok()?))
        } else {
            (u32::from_be_bytes(e[0..4].try_into().ok()?) as u64, i32::from_be_bytes(e[4..8].try_into().ok()?) as i64)
        };
        if media_time < 0 {
            continue; // an empty edit: a pause before the media starts
        }
        let start = media_time as u64;
        // A segment length of 0 means "to the end of the media".
        let units = if seg == 0 { media_dur.saturating_sub(start) } else { (seg as u128 * media_scale as u128 / movie_scale as u128) as u64 };
        let units = units.min(media_dur.saturating_sub(start));
        let to_frames = |u: u64| (u as u128 * sample_rate as u128 / media_scale as u128) as u64;
        let (delay, valid) = (to_frames(start), to_frames(units));
        // Nothing to cut: the whole file is the song.
        if delay == 0 && to_frames(media_dur) <= valid {
            return None;
        }
        return Some(Trim { delay, valid });
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn elst_v0(entries: &[(u32, i32)]) -> Vec<u8> {
        let mut v = vec![0, 0, 0, 0];
        v.extend((entries.len() as u32).to_be_bytes());
        for (seg, mt) in entries {
            v.extend(seg.to_be_bytes());
            v.extend(mt.to_be_bytes());
            v.extend([0, 1, 0, 0]);
        }
        v
    }

    #[test]
    fn reads_delay_and_length_from_an_edit_list() {
        // Movie timescale 1000, media timescale 44100: 3 s of music after 1024 frames of priming.
        let t = trim_from_elst(&elst_v0(&[(3000, 1024)]), 1000, 44_100, 133_324, 44_100).unwrap();
        assert_eq!(t, Trim { delay: 1024, valid: 132_300 });
    }

    #[test]
    fn an_empty_edit_before_the_media_is_skipped() {
        let t = trim_from_elst(&elst_v0(&[(500, -1), (3000, 2112)]), 1000, 44_100, 135_000, 44_100).unwrap();
        assert_eq!((t.delay, t.valid), (2112, 132_300));
    }

    #[test]
    fn a_whole_file_edit_means_nothing_to_cut() {
        assert_eq!(trim_from_elst(&elst_v0(&[(3000, 0)]), 1000, 44_100, 132_300, 44_100), None);
    }

    #[test]
    fn units_convert_to_the_decoded_rate() {
        // Media timescale 22050 (HE-AAC style), decoded at 44100: counts double.
        let t = trim_from_elst(&elst_v0(&[(1000, 1024)]), 1000, 22_050, 24_000, 44_100).unwrap();
        assert_eq!((t.delay, t.valid), (2048, 44_100));
    }

    #[test]
    fn a_non_mp4_file_has_no_trim() {
        assert_eq!(read_trim(Path::new("Cargo.toml"), 44_100), None);
    }
}
