//! Songs on the person's own server: played as the original files, seekable, gapless like local ones.

use std::sync::Arc;
use std::time::Duration;

use super::{Decoder, OpusFileDecoder, SymphoniaDecoder};
use crate::error::Result;
use crate::net::ranged::RangedReader;
use crate::net::Net;

/// Opens a song on the server. Ogg Opus is saved to the cache folder first (the Opus decoder needs a
/// file); everything else is read straight from the server in pieces.
pub fn open(net: Arc<Net>, id: &str) -> Result<Box<dyn Decoder>> {
    let mut reader = RangedReader::open(net.clone(), id)?;
    let head = reader.head(256);
    if head.starts_with(b"OggS") && head.windows(8).any(|w| w == b"OpusHead") {
        let dir = net.cache_dir().join("server-opus");
        std::fs::create_dir_all(&dir)?;
        prune(&dir);
        let name: String = id.chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '_' }).collect();
        let path = dir.join(format!("{name}.opus"));
        reader.save_to(&path)?;
        return Ok(Box::new(OpusFileDecoder::open(&path)?));
    }
    let mime = reader.content_type().to_string();
    Ok(Box::new(SymphoniaDecoder::open_seekable(Box::new(reader), "", &mime)?))
}

/// Saved Opus songs older than a day are not needed any more. (A file still open on Windows
/// cannot be removed; it goes next time.)
fn prune(dir: &std::path::Path) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.filter_map(|e| e.ok()) {
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age > Duration::from_secs(24 * 3600));
        if old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::fakesub;
    use crate::net::ServerConfig;

    fn net_for(f: &fakesub::Fake, tag: &str) -> Arc<Net> {
        let n = Arc::new(Net::new(std::env::temp_dir().join(format!("redshift-remote-{tag}")), "t", &[]));
        n.set_enabled(true);
        n.set_server(Some(&ServerConfig { url: f.base.clone(), user: fakesub::USER.into(), password: fakesub::PASSWORD.into() }));
        n
    }

    fn drain(d: &mut dyn Decoder) -> usize {
        let mut out = Vec::new();
        while d.read(&mut out).unwrap() {}
        out.len() / d.info().channels
    }

    #[test]
    fn a_song_from_the_server_decodes_to_the_right_length() {
        // 14 s of 44.1 kHz stereo is about 2.4 MB: several blocks.
        let f = fakesub::start(fakesub::wav(14, 44100), true);
        let mut d = open(net_for(&f, "len"), "s1").unwrap();
        assert_eq!((d.info().sample_rate, d.info().channels, d.info().bits_per_sample), (44100, 2, Some(16)));
        let frames = drain(d.as_mut());
        assert_eq!(frames, 14 * 44100);
    }

    #[test]
    fn seeking_a_song_from_the_server_asks_for_the_part_after_the_jump() {
        // 60 s is about 10.6 MB (ten blocks); 50 s is in the ninth.
        let f = fakesub::start(fakesub::wav(60, 44100), true);
        let mut d = open(net_for(&f, "seek"), "s1").unwrap();
        d.seek(50.0).unwrap();
        let rest = drain(d.as_mut());
        let secs = rest as f64 / 44100.0;
        assert!((secs - 10.0).abs() < 0.1, "after seeking to 50 s about 10 s should remain, got {secs}");
        let ranges = f.ranges.lock().clone();
        let mib = |n: u64| format!("bytes={}-", n << 20);
        assert!(!ranges.iter().any(|r| r.starts_with(&mib(4)) || r.starts_with(&mib(5)) || r.starts_with(&mib(6))), "the blocks between the start and 50 s were never needed: {ranges:?}");
    }

    #[test]
    fn a_server_that_cannot_do_ranges_still_plays_and_seeks() {
        let f = fakesub::start(fakesub::wav(6, 44100), false);
        let mut d = open(net_for(&f, "whole"), "s1").unwrap();
        d.seek(4.0).unwrap();
        let secs = drain(d.as_mut()) as f64 / 44100.0;
        assert!((secs - 2.0).abs() < 0.1, "{secs}");
    }
}
