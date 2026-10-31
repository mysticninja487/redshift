mod common;

use common::*;
use sonara_core::decode;

const WINDOW: usize = 4096;
/// The chirp sweeps ~90 Hz across one window, so allow for that plus FFT bin width.
const CHIRP_TOLERANCE_HZ: f32 = 60.0;

fn assert_close(actual: f32, expected: f32, tol: f32, what: &str) {
    assert!((actual - expected).abs() <= tol, "{what}: got {actual}, expected {expected} ± {tol}");
}

// ---- FLAC ------------------------------------------------------------------------------------

#[test]
fn flac_decodes_whole_file_with_correct_format_and_length() {
    let (info, pcm) = decode_file("chirp_44k_stereo.flac");
    assert_eq!((info.sample_rate, info.channels, info.codec), (44_100, 2, "flac"));
    assert_eq!(info.bits_per_sample, Some(16));
    assert_eq!(info.total_frames, Some(132_300));
    assert_eq!(pcm.len() / 2, 132_300, "every frame should come through, none added or lost");
}

#[test]
fn flac_keeps_channel_order_and_level() {
    let (_, pcm) = decode_file("chirp_44k_stereo.flac");
    let (left, right) = (channel(&pcm, 2, 0), channel(&pcm, 2, 1));
    // Fixture: left amplitude 0.5, right 0.25.
    assert_close(peak(&left), 0.5, 0.01, "left peak");
    assert_close(peak(&right), 0.25, 0.01, "right peak");
    assert_close(rms(&left) / rms(&right), 2.0, 0.05, "left/right rms ratio");
}

#[test]
fn flac_timeline_matches_chirp() {
    let (info, pcm) = decode_file("chirp_44k_stereo.flac");
    let left = channel(&pcm, 2, 0);
    for start in [0, 30_000, 60_000, 120_000] {
        let hz = dominant_hz(&left[start..], info.sample_rate, WINDOW);
        let expected = chirp_hz_at_window(start, WINDOW, info.sample_rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("frequency at frame {start}"));
    }
}

#[test]
fn flac_seek_lands_on_the_requested_frame() {
    let path = fixture("chirp_44k_stereo.flac");
    for secs in [0.0, 0.5, 1.37, 2.5] {
        let mut dec = decode::open(&path).unwrap();
        let (rate, total) = (dec.info().sample_rate, dec.info().total_frames.unwrap());
        dec.seek(secs).unwrap();
        let pcm = read_all(dec.as_mut());

        let target = (secs * rate as f64).round() as u64;
        let frames = (pcm.len() / 2) as u64;
        assert!(
            frames.abs_diff(total - target) <= 1,
            "seek to {secs}s: {frames} frames remain, expected {}",
            total - target
        );
        let left = channel(&pcm, 2, 0);
        let hz = dominant_hz(&left, rate, WINDOW);
        let expected = chirp_hz_at_window(target as usize, WINDOW, rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("frequency after seek to {secs}s"));
    }
}

#[test]
fn seeking_repeatedly_on_one_decoder_works() {
    let mut dec = decode::open(&fixture("chirp_44k_stereo.flac")).unwrap();
    let rate = dec.info().sample_rate;
    for secs in [2.0, 0.25, 1.5, 0.0] {
        dec.seek(secs).unwrap();
        let pcm = read_frames(dec.as_mut(), WINDOW, 2);
        let hz = dominant_hz(&channel(&pcm, 2, 0), rate, WINDOW);
        let expected = chirp_hz_at_window((secs * rate as f64) as usize, WINDOW, rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("after seeking to {secs}s"));
    }
}

#[test]
fn seeking_past_the_end_yields_nothing() {
    let mut dec = decode::open(&fixture("chirp_44k_stereo.flac")).unwrap();
    dec.seek(999.0).unwrap();
    assert!(read_all(dec.as_mut()).is_empty());
}

#[test]
fn flac_24_bit_96k_mono() {
    let (info, pcm) = decode_file("tone_96k_24bit_mono.flac");
    assert_eq!((info.sample_rate, info.channels), (96_000, 1));
    assert_eq!(info.bits_per_sample, Some(24));
    assert_eq!(pcm.len(), 192_000);
    assert_close(peak(&pcm), 0.5, 0.001, "peak");
    assert_close(dominant_hz(&pcm, 96_000, 8192), 1000.0, 15.0, "tone frequency");
}

// ---- Opus ------------------------------------------------------------------------------------

#[test]
fn opus_decodes_at_48k_with_exact_length() {
    let (info, pcm) = decode_file("chirp_48k_stereo.opus");
    assert_eq!((info.sample_rate, info.channels, info.codec), (48_000, 2, "opus"));
    let total = info.total_frames.expect("length comes from the last page's granule position");
    assert_eq!(pcm.len() / 2, total as usize, "pre-skip and end padding must be trimmed exactly");
    assert!(total.abs_diff(144_000) <= 960, "3 s fixture decoded to {total} frames");
}

#[test]
fn opus_timeline_matches_chirp_including_the_very_start() {
    let (info, pcm) = decode_file("chirp_48k_stereo.opus");
    let left = channel(&pcm, 2, 0);
    // Start of file checks that pre-skip was dropped: if it weren't, the first window would
    // be shifted by ~6.5 ms and read a few Hz high; more importantly the later windows would drift.
    for start in [0, 40_000, 90_000, 130_000] {
        let hz = dominant_hz(&left[start..], info.sample_rate, WINDOW);
        let expected = chirp_hz_at_window(start, WINDOW, info.sample_rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("frequency at frame {start}"));
    }
    assert_close(rms(&left) / rms(&channel(&pcm, 2, 1)), 2.0, 0.15, "left/right balance after lossy coding");
}

#[test]
fn opus_seek_lands_on_the_requested_frame() {
    let path = fixture("chirp_48k_stereo.opus");
    for secs in [0.0, 0.05, 0.5, 1.37, 2.5, 2.95] {
        let mut dec = decode::open(&path).unwrap();
        let (rate, total) = (dec.info().sample_rate, dec.info().total_frames.unwrap());
        dec.seek(secs).unwrap();
        let pcm = read_all(dec.as_mut());

        let target = (secs * rate as f64) as u64;
        let frames = (pcm.len() / 2) as u64;
        assert!(
            frames.abs_diff(total - target) <= 1,
            "seek to {secs}s: {frames} frames remain, expected {}",
            total - target
        );
        if frames as usize >= WINDOW {
            let hz = dominant_hz(&channel(&pcm, 2, 0), rate, WINDOW);
            let expected = chirp_hz_at_window(target as usize, WINDOW, rate);
            assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("frequency after seek to {secs}s"));
        }
    }
}

#[test]
fn opus_seek_forwards_and_backwards_on_one_decoder() {
    let mut dec = decode::open(&fixture("chirp_48k_stereo.opus")).unwrap();
    let rate = dec.info().sample_rate;
    for secs in [2.0, 0.25, 1.5, 0.0, 2.2] {
        dec.seek(secs).unwrap();
        let pcm = read_frames(dec.as_mut(), WINDOW, 2);
        let hz = dominant_hz(&channel(&pcm, 2, 0), rate, WINDOW);
        let expected = chirp_hz_at_window((secs * rate as f64) as usize, WINDOW, rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("after seeking to {secs}s"));
    }
}

#[test]
fn opus_seek_past_end_yields_nothing() {
    let mut dec = decode::open(&fixture("chirp_48k_stereo.opus")).unwrap();
    dec.seek(999.0).unwrap();
    assert!(read_all(dec.as_mut()).is_empty());
}

#[test]
fn ogg_extension_is_sniffed_as_opus() {
    let (info, pcm) = decode_file("chirp_48k_stereo_opus.ogg");
    assert_eq!(info.codec, "opus");
    assert_eq!(pcm.len() / 2, info.total_frames.unwrap() as usize);
}

#[test]
fn mono_opus() {
    let (info, pcm) = decode_file("tone_48k_mono.opus");
    assert_eq!((info.sample_rate, info.channels), (48_000, 1));
    assert_eq!(pcm.len(), info.total_frames.unwrap() as usize);
    assert_close(dominant_hz(&pcm[4800..], 48_000, 8192), 440.0, 8.0, "tone frequency");
}

// ---- MP3 / errors ----------------------------------------------------------------------------

#[test]
fn mp3_is_trimmed_exactly_once_for_encoder_delay_and_padding() {
    let (info, pcm) = decode_file("chirp_44k_stereo.mp3");
    assert_eq!((info.sample_rate, info.channels), (44_100, 2));
    // The fixture is 3 s = 132300 frames. Untrimmed, an MP3 decodes ~1300 frames longer; trimmed
    // twice (which is what happens if we trim on top of the decoder's own gapless trim) it comes up
    // short. The LAME header's length must match what we actually produce.
    assert_eq!(info.total_frames, Some(132_300));
    assert_eq!(pcm.len() / 2, 132_300);
    let hz = dominant_hz(&channel(&pcm, 2, 0), 44_100, WINDOW);
    assert_close(hz, chirp_hz_at_window(0, WINDOW, 44_100), 30.0, "frequency at start");
}

#[test]
fn mp3_seek_lands_on_the_requested_frame() {
    let path = fixture("chirp_44k_stereo.mp3");
    for secs in [0.0, 0.5, 1.37, 2.5] {
        let mut dec = decode::open(&path).unwrap();
        let rate = dec.info().sample_rate;
        dec.seek(secs).unwrap();
        let pcm = read_all(dec.as_mut());

        let target = (secs * rate as f64).round() as u64;
        let frames = (pcm.len() / 2) as u64;
        assert!(
            frames.abs_diff(132_300 - target) <= 1,
            "seek to {secs}s: {frames} frames remain, expected {}",
            132_300 - target
        );
        let hz = dominant_hz(&channel(&pcm, 2, 0), rate, WINDOW);
        let expected = chirp_hz_at_window(target as usize, WINDOW, rate);
        assert_close(hz, expected, CHIRP_TOLERANCE_HZ, &format!("frequency after seek to {secs}s"));
    }
}

#[test]
fn missing_and_garbage_files_error_instead_of_panicking() {
    assert!(decode::open(&fixture("does_not_exist.flac")).is_err());

    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("junk.flac");
    std::fs::write(&junk, vec![0x5Au8; 4096]).unwrap();
    assert!(decode::open(&junk).is_err());

    let junk_opus = dir.path().join("junk.opus");
    std::fs::write(&junk_opus, b"OggS\0\x02garbage garbage garbage garbage").unwrap();
    assert!(decode::open(&junk_opus).is_err());
}
