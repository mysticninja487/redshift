//! Songs that live somewhere else and are read in pieces: on the person's own server or in the
//! Internet Archive. They play as the original files, seekable, gapless like local ones.

use std::sync::Arc;
use std::time::Duration;

use super::{Decoder, OpusFileDecoder, SymphoniaDecoder};
use crate::error::Result;
use crate::net::ranged::{Origin, RangedReader};
use crate::net::Net;

/// Where the song at a queue path comes from, if it is one of these.
pub fn origin_of(path: &std::path::Path) -> Option<Origin> {
    if let Some(id) = crate::util::server_song_id(path) {
        return Some(Origin::Server(id));
    }
    crate::util::archive_song_parts(path).map(|(identifier, file)| Origin::Archive { identifier, file })
}

/// Opens a song on the server. Ogg Opus is saved to the cache folder first (the Opus decoder needs a
/// file); everything else is read straight from the server in pieces.
pub fn open(net: Arc<Net>, id: &str) -> Result<Box<dyn Decoder>> {
    open_origin(net, Origin::Server(id.to_string()))
}

pub fn open_origin(net: Arc<Net>, origin: Origin) -> Result<Box<dyn Decoder>> {
    let mut reader = RangedReader::open_origin(net.clone(), origin.clone())?;
    let head = reader.head(256);
    if head.starts_with(b"OggS") && head.windows(8).any(|w| w == b"OpusHead") {
        let dir = net.cache_dir().join("server-opus");
        std::fs::create_dir_all(&dir)?;
        prune(&dir);
        let path = dir.join(format!("{}.opus", origin.cache_name()));
        reader.save_to(&path)?;
        return Ok(Box::new(OpusFileDecoder::open(&path)?));
    }
    let mime = reader.content_type().to_string();
    Ok(Box::new(SymphoniaDecoder::open_seekable(Box::new(reader), &origin.extension(), &mime)?))
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

    fn archive_net(f: &crate::net::fakearchive::Fake, tag: &str) -> Arc<Net> {
        let n = Arc::new(Net::new(std::env::temp_dir().join(format!("redshift-remote-a-{tag}")), "t", &[]));
        n.set_enabled(true);
        n.set_archive_base_for_tests(&f.base);
        n
    }

    fn song(file: &str) -> Origin {
        Origin::Archive { identifier: "demo-live".into(), file: file.into() }
    }

    #[test]
    fn a_file_from_the_archive_is_found_through_its_redirect_and_decodes() {
        // A file name with a space and an accent, behind a redirect, read in several blocks.
        let f = crate::net::fakearchive::start(fakesub::wav(14, 44100), true);
        let mut d = open_origin(archive_net(&f, "len"), song("Disc 2/d2 t01 Café.flac")).unwrap();
        assert_eq!((d.info().sample_rate, d.info().channels), (44100, 2));
        assert_eq!(drain(d.as_mut()), 14 * 44100);
        assert!(f.requests.lock().iter().any(|l| l.starts_with("GET /download/demo-live/Disc%202/d2%20t01%20Caf%C3%A9.flac")), "{:?}", f.requests.lock());
        assert!(f.ranges.lock().iter().all(|r| r.starts_with("bytes=")), "every audio request asked for a range");
    }

    #[test]
    fn seeking_in_a_file_from_the_archive_skips_what_is_not_needed() {
        let f = crate::net::fakearchive::start(fakesub::wav(60, 44100), true);
        let mut d = open_origin(archive_net(&f, "seek"), song("a.flac")).unwrap();
        d.seek(50.0).unwrap();
        let secs = drain(d.as_mut()) as f64 / 44100.0;
        assert!((secs - 10.0).abs() < 0.1, "{secs}");
        let ranges = f.ranges.lock().clone();
        let mib = |n: u64| format!("bytes={}-", n << 20);
        assert!(!ranges.iter().any(|r| r.starts_with(&mib(4)) || r.starts_with(&mib(5)) || r.starts_with(&mib(6))), "{ranges:?}");
    }

    #[test]
    fn files_the_archive_refuses_or_lacks_give_clear_errors() {
        let f = crate::net::fakearchive::start(fakesub::wav(2, 44100), true);
        let n = archive_net(&f, "err");
        let say = |file: &str| open_origin(n.clone(), song(file)).err().map(|e| e.to_string()).unwrap_or_default();
        assert!(say("missing.flac").contains("does not have that file"), "{}", say("missing.flac"));
        assert!(say("forbidden.flac").contains("restricted"), "{}", say("forbidden.flac"));
        n.set_enabled(false);
        assert!(matches!(open_origin(n, song("a.flac")).err(), Some(crate::error::Error::Offline)));
    }

    #[test]
    fn queue_paths_name_their_origin() {
        use std::path::Path;
        assert_eq!(origin_of(Path::new("subsonic://77")), Some(Origin::Server("77".into())));
        assert_eq!(origin_of(Path::new("archive://demo-live/Disc 2/a.flac")), Some(song("Disc 2/a.flac")));
        assert_eq!(origin_of(Path::new("C:/music/a.flac")), None);
        assert_eq!(song("Disc 2/a.flac").cache_name(), "a_demo_live_Disc_2_a_flac");
        assert_eq!(song("Disc 2/a.flac").extension(), "flac");
    }
}
