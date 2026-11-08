//! End-to-end tests: real files -> decoder threads -> resampler -> mixer -> an offline "sound card".
//! The audio that comes out is checked by measuring the chirp fixtures' frequency, which encodes
//! time, so these tests catch misaligned, dropped, repeated or mistimed audio.

mod common;

use std::sync::Arc;
use std::time::{Duration, Instant};

use common::*;
use crossbeam_channel::Receiver;
use parking_lot::Mutex;
use redshift_core::engine::{OfflineConfig, Player, PlayerConfig, PlayerEvent, Status};
use redshift_core::queue::RepeatMode;

const RATE: u32 = 48_000;
const WAIT: Duration = Duration::from_secs(30);

struct Rig {
    player: Player,
    events: Receiver<PlayerEvent>,
    sink: Arc<Mutex<Vec<f32>>>,
    seen: Vec<PlayerEvent>,
}

fn rig(speed: f32) -> Rig {
    let sink = Arc::new(Mutex::new(Vec::new()));
    let (player, events) = Player::new(PlayerConfig {
        device: None,
        offline: Some(OfflineConfig { sample_rate: RATE, sink: sink.clone(), speed }),
    })
    .unwrap();
    Rig { player, events, sink, seen: Vec::new() }
}

fn paths(names: &[&str]) -> Vec<std::path::PathBuf> {
    names.iter().map(|n| fixture(n)).collect()
}

impl Rig {
    /// Waits for an event matching `pred`, remembering everything seen on the way.
    fn wait_event(&mut self, what: &str, mut pred: impl FnMut(&PlayerEvent) -> bool) -> PlayerEvent {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(left) {
                Ok(e) => {
                    self.seen.push(e.clone());
                    if pred(&e) {
                        return e;
                    }
                }
                Err(_) => panic!("timed out waiting for {what}; saw {:#?}", self.seen),
            }
        }
    }
    fn wait_ended(&mut self) {
        self.wait_event("the queue to end", |e| *e == PlayerEvent::Ended);
    }
    fn wait_position(&self, what: &str, pred: impl Fn(f64) -> bool) -> f64 {
        let deadline = Instant::now() + WAIT;
        loop {
            let p = self.player.position_secs();
            if pred(p) && self.player.status() != Status::Stopped {
                return p;
            }
            assert!(Instant::now() < deadline, "timed out waiting for {what} (position {p})");
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    fn left(&self) -> Vec<f32> {
        self.sink.lock().chunks_exact(2).map(|f| f[0]).collect()
    }
    fn entry_ids(&self) -> Vec<u64> {
        self.player.queue().entries.iter().map(|e| e.id).collect()
    }
}

/// The audible part of the output: from the first non-silent frame to the last.
fn audible(left: &[f32]) -> (usize, usize) {
    let first = left.iter().position(|v| v.abs() > 1e-5).expect("nothing was played");
    let last = left.iter().rposition(|v| v.abs() > 1e-5).unwrap();
    (first, last + 1)
}

fn assert_close(actual: f32, expected: f32, tol: f32, what: &str) {
    assert!((actual - expected).abs() <= tol, "{what}: got {actual}, expected {expected} +/- {tol}");
}

fn assert_chirp_at(left: &[f32], start: usize, audio_frame: usize, what: &str) {
    let hz = dominant_hz(&left[start + audio_frame..], RATE, 4096);
    assert_close(hz, chirp_hz_at_window(audio_frame, 4096, RATE), 70.0, what);
}

#[test]
fn a_44k_flac_plays_through_a_48k_device_complete_and_in_time() {
    let mut r = rig(6.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    r.wait_ended();
    let left = r.left();
    let (first, last) = audible(&left);
    assert!((last - first).abs_diff(144_000) < 1_500, "played {} frames, expected ~144000", last - first);
    for frame in [10_000, 50_000, 100_000, 130_000] {
        assert_chirp_at(&left, first, frame, &format!("resampled chirp at frame {frame}"));
    }
}

#[test]
fn opus_plays_end_to_end_and_reports_its_format() {
    let mut r = rig(6.0);
    r.player.set_queue(paths(&["chirp_48k_stereo.opus"]), 0, true);
    r.wait_event("track start", |e| matches!(e, PlayerEvent::TrackChanged { entry_id: Some(_) }));
    let now = r.player.queue().current.expect("current track info");
    assert_eq!((now.codec, now.source_rate, now.source_channels), ("opus", 48_000, 2));
    assert!((now.duration_secs.unwrap() - 3.0).abs() < 0.03);
    r.wait_ended();
    let left = r.left();
    let (first, last) = audible(&left);
    assert!((last - first).abs_diff(144_000) < 1_500);
    assert_chirp_at(&left, first, 70_000, "opus chirp");
}

#[test]
fn consecutive_tracks_join_without_a_gap() {
    let mut r = rig(3.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus", "chirp_44k_stereo.mp3"]), 0, true);
    r.wait_ended();
    let ids = r.entry_ids();
    let changes: Vec<_> = r.seen.iter().filter_map(|e| match e {
        PlayerEvent::TrackChanged { entry_id: Some(id) } => Some(*id),
        _ => None,
    }).collect();
    assert_eq!(changes, ids, "each track announced once, in order");

    let left = r.left();
    let (first, last) = audible(&left);
    assert!((last - first).abs_diff(3 * 144_000) < 3_000, "total {} frames", last - first);
    // The chirp has no silences, apart from isolated zero crossings.
    let mut run = 0;
    let mut longest = 0;
    for v in &left[first + 2_000..last - 2_000] {
        run = if v.abs() < 2e-3 { run + 1 } else { 0 };
        longest = longest.max(run);
    }
    assert!(longest <= 4, "a silent run of {longest} frames: audible gap between tracks");
    // Track 2 starts right where track 1 ends: at its start the chirp is back at 400 Hz.
    let join = first + 144_000;
    let hz = dominant_hz(&left[join + 500..], RATE, 4096);
    assert!(hz < 700.0, "after the first join the chirp should restart low, got {hz} Hz");
}

#[test]
fn crossfade_overlaps_tracks_and_blends_both() {
    let mut r = rig(3.0);
    r.player.set_crossfade_secs(1.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus"]), 0, true);
    r.wait_ended();
    let left = r.left();
    let (first, last) = audible(&left);
    let len = last - first;
    assert!(len.abs_diff(2 * 144_000 - 48_000) < 3_000, "expected ~{} frames, got {len}", 2 * 144_000 - 48_000);

    // How loud does one of these chirps read in this measurement? (A Hann-windowed FFT of a
    // sweeping tone reads below its amplitude, so calibrate on a plain stretch of track 1, at
    // the same sweep rate and amplitude as everything we measure below.)
    let reference = band_peak(&left[first + 40_000..], RATE, 1_000.0, 3_000.0, 4096);
    assert!(reference > 0.25, "calibration level {reference}");

    // Middle of the overlap: track 1 is at 2.5 s (~2900 Hz) and track 2 at 0.5 s (~900 Hz).
    let mid = first + 144_000 - 24_000 - 2_048;
    let w = &left[mid..mid + 4096];
    let old = band_peak(w, RATE, 2_700.0, 3_100.0, 4096);
    let new = band_peak(w, RATE, 700.0, 1_100.0, 4096);
    // Equal power: at the midpoint each track is at -3 dB of its own level.
    let expected = reference * std::f32::consts::FRAC_1_SQRT_2;
    assert_close(old, expected, 0.15 * reference, "old track level mid-fade");
    assert_close(new, expected, 0.15 * reference, "new track level mid-fade");

    // Early in the fade the old track dominates; late in it the new one does.
    let early_w = &left[first + 144_000 - 46_000..][..4096]; // ~2.0 s fade-in-progress start
    let late_w = &left[first + 144_000 - 2_048 - 2_000..][..4096];
    let (old_early, new_early) = (band_peak(early_w, RATE, 2_000.0, 3_400.0, 4096), band_peak(early_w, RATE, 400.0, 800.0, 4096));
    let (old_late, new_late) = (band_peak(late_w, RATE, 3_200.0, 3_500.0, 4096), band_peak(late_w, RATE, 1_300.0, 1_500.0, 4096));
    assert!(old_early > new_early, "old track should lead early in the fade ({old_early} vs {new_early})");
    assert!(new_late > old_late, "new track should lead late in the fade ({new_late} vs {old_late})");
}

#[test]
fn seeking_jumps_to_the_requested_position() {
    let r = rig(2.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    r.wait_position("playback to start", |p| p > 0.2);
    r.player.seek(2.0);
    // Wait until the whole 4096-frame analysis window (85 ms) is post-seek audio.
    let p = r.wait_position("the seek to land", |p| (2.15..2.7).contains(&p));
    let left = r.left();
    let tail = &left[left.len() - 4096..];
    let hz = dominant_hz(tail, RATE, 4096);
    // The tail ends at (about) the current position; its centre is half a window earlier.
    let expected = 400.0 + 1000.0 * (p as f32 - 2048.0 / RATE as f32);
    assert!((hz - expected).abs() < 200.0, "after seeking to 2 s the audio is at {hz} Hz, expected near {expected} (position {p})");
    assert!(hz > 2_000.0, "must have jumped forward");
    r.player.seek(0.5);
    r.wait_position("seek backwards", |p| (0.7..1.3).contains(&p));
    let left = r.left();
    let hz = dominant_hz(&left[left.len() - 4096..], RATE, 4096);
    assert!(hz < 1_700.0, "after seeking back the chirp should be low again, got {hz} Hz");
}

#[test]
fn pause_silences_and_holds_the_position_then_resume_continues() {
    let r = rig(2.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    r.wait_position("playback", |p| p > 0.3);
    r.player.pause();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.player.status(), Status::Paused);
    let held = r.player.position_secs();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.player.position_secs(), held, "position must not move while paused");
    let left = r.left();
    assert!(left[left.len() - 3000..].iter().all(|&v| v == 0.0), "silence while paused");

    r.player.resume();
    r.wait_position("playback to continue", |p| p > held + 0.3);
    assert_eq!(r.player.status(), Status::Playing);
    r.player.toggle_pause();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.player.status(), Status::Paused);
}

#[test]
fn next_previous_and_the_end_of_the_queue() {
    let mut r = rig(4.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus", "chirp_44k_stereo.mp3"]), 0, true);
    r.wait_event("start", |e| matches!(e, PlayerEvent::TrackChanged { entry_id: Some(_) }));
    let ids = r.entry_ids();
    let changed_to = |r: &mut Rig, id: u64| {
        r.wait_event(&format!("track {id}"), |e| *e == PlayerEvent::TrackChanged { entry_id: Some(id) });
    };
    r.player.next();
    changed_to(&mut r, ids[1]);
    r.player.next();
    changed_to(&mut r, ids[2]);
    assert_eq!(r.player.queue().current_entry, Some(ids[2]));
    r.player.next(); // already last, nothing repeats: stays put
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(r.player.queue().current_entry, Some(ids[2]));
    // Early in a track, "previous" goes back a track; later it restarts the current one.
    r.player.previous();
    changed_to(&mut r, ids[1]);
    r.wait_position("a bit of playback", |p| p > 0.1);
    r.player.play_entry(ids[0]);
    changed_to(&mut r, ids[0]);
}

#[test]
fn the_queue_view_lists_tracks_in_the_order_they_will_play() {
    let mut r = rig(6.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus", "chirp_44k_stereo.mp3"]), 0, true);
    r.wait_event("start", |e| matches!(e, PlayerEvent::TrackChanged { entry_id: Some(_) }));
    r.player.set_shuffle(true);
    let deadline = Instant::now() + WAIT;
    while !r.player.queue().shuffle {
        assert!(Instant::now() < deadline, "shuffle never applied");
        std::thread::sleep(Duration::from_millis(2));
    }
    let view = r.player.queue();
    assert_eq!(view.entries.len(), 3);
    assert_eq!(Some(view.entries[0].id), view.current_entry, "shuffle puts the playing track first");
    let second = view.entries[1].id;
    r.player.next();
    r.wait_event("the track the queue listed second", |e| *e == PlayerEvent::TrackChanged { entry_id: Some(second) });
}

#[test]
fn repeat_one_loops_the_same_track() {
    let mut r = rig(6.0);
    r.player.set_repeat(RepeatMode::One);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus"]), 0, true);
    let mut starts = 0;
    let mut which = None;
    r.wait_event("the track to loop", |e| {
        if let PlayerEvent::TrackChanged { entry_id: Some(id) } = e {
            assert!(which.is_none() || which == Some(*id), "repeat-one must not move to another entry");
            which = Some(*id);
            starts += 1;
        }
        starts >= 3
    });
    r.player.stop();
}

#[test]
fn an_unplayable_file_is_reported_and_skipped() {
    let mut r = rig(6.0);
    let dir = tempfile::tempdir().unwrap();
    let junk = dir.path().join("broken.flac");
    std::fs::write(&junk, vec![7u8; 5000]).unwrap();
    let mut list = vec![junk, std::path::PathBuf::from("/nonexistent/missing.opus")];
    list.extend(paths(&["chirp_44k_stereo.flac"]));
    r.player.set_queue(list, 0, true);
    r.wait_event("playback of the good track", |e| matches!(e, PlayerEvent::TrackChanged { entry_id: Some(_) }));
    let ids = r.entry_ids();
    let errors = r.seen.iter().filter(|e| matches!(e, PlayerEvent::Error { .. })).count();
    assert_eq!(errors, 2, "one error per bad file: {:#?}", r.seen);
    assert_eq!(r.player.queue().current_entry, Some(ids[2]), "landed on the playable entry");
    assert_eq!(r.player.queue().entries.len(), 3, "bad entries stay in the queue");
    r.wait_ended();
}

#[test]
fn queue_editing_while_playing_changes_what_plays_next() {
    let mut r = rig(4.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac", "chirp_48k_stereo.opus"]), 0, true);
    r.wait_event("start", |e| matches!(e, PlayerEvent::TrackChanged { entry_id: Some(_) }));
    r.player.play_next(paths(&["chirp_44k_stereo.mp3"]));
    r.wait_event("queue update", |e| *e == PlayerEvent::QueueChanged);
    let q = r.player.queue();
    let mp3 = q.entries[1].id;
    assert!(q.entries[1].path.ends_with("chirp_44k_stereo.mp3"));
    // The mp3 must be what plays after the flac, not the opus that was queued before.
    r.wait_event("the inserted track to play", |e| *e == PlayerEvent::TrackChanged { entry_id: Some(mp3) });
    r.player.remove(r.entry_ids()[2]);
    r.wait_ended();
    assert!(r.seen.iter().all(|e| !matches!(e, PlayerEvent::Error { .. })), "{:#?}", r.seen);
}

#[test]
fn reopening_the_device_resumes_playback_where_it_was() {
    let mut r = rig(2.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    let before = r.wait_position("playback", |p| p > 0.8);
    r.player.set_device(None);
    r.wait_event("device reopened", |e| matches!(e, PlayerEvent::DeviceChanged { .. }));
    let after = r.wait_position("playback to resume", |p| p > 0.0);
    assert!(after > before - 0.4 && after < before + 1.5, "resumed at {after}s after being at {before}s");
    assert_eq!(r.player.status(), Status::Playing);
    r.wait_ended();
}

#[test]
fn visualiser_sees_the_music_and_goes_quiet_after() {
    let r = rig(2.0);
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    r.wait_position("playback", |p| p > 0.5);
    let mut loudest = 0.0f32;
    for _ in 0..10 {
        std::thread::sleep(Duration::from_millis(20));
        let f = r.player.viz();
        loudest = loudest.max(f.bars.iter().cloned().fold(0.0, f32::max));
        assert!(f.peak_left <= 1.0);
    }
    assert!(loudest > 0.5, "spectrum stayed flat during playback ({loudest})");
    r.player.stop();
    std::thread::sleep(Duration::from_millis(300));
    for _ in 0..80 {
        std::thread::sleep(Duration::from_millis(25));
        r.player.viz();
    }
    let f = r.player.viz();
    assert!(f.bars.iter().all(|&b| b < 0.05), "bars should have fallen after stopping: {:?}", f.bars.iter().cloned().fold(0.0, f32::max));
}

#[test]
fn volume_and_eq_reach_the_output() {
    let r = rig(2.0);
    r.player.set_volume(0.5); // amplitude 0.25
    r.player.set_queue(paths(&["chirp_44k_stereo.flac"]), 0, true);
    r.wait_position("playback", |p| p > 0.5);
    std::thread::sleep(Duration::from_millis(50));
    let left = r.left();
    let peak = left[left.len() - 4800..].iter().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak < 0.2 && peak > 0.05, "volume 0.5 should give ~0.5*0.25 peak, got {peak}");
    assert_close(r.player.volume(), 0.5, 1e-4, "volume readback");
    r.player.stop();
}

// ---- ReplayGain ----
// tagged.flac is a 0.3-amplitude 660 Hz sine, one second long, tagged with track gain -6.5 dB
// (peak 0.9) and album gain -5.25 dB. chirp_44k_stereo.flac has no ReplayGain tags.

use redshift_core::engine::ReplayGainMode;

fn db(x: f32) -> f32 {
    10f32.powf(x / 20.0)
}

/// RMS of the steady middle of whatever was played (skipping fades at both ends).
fn mid_rms(left: &[f32]) -> f32 {
    let (a, b) = audible(left);
    rms(&left[a + 4800..b - 4800])
}

/// Plays `file` to the end with the given ReplayGain settings made before it starts.
fn play_rms(file: &str, mode: ReplayGainMode, preamp_db: f32) -> f32 {
    let mut r = rig(4.0);
    r.player.set_replaygain(mode, preamp_db);
    r.player.set_queue(paths(&[file]), 0, true);
    r.wait_ended();
    mid_rms(&r.left())
}

#[test]
fn replaygain_scales_the_output_by_the_tagged_gain() {
    let off = play_rms("tagged.flac", ReplayGainMode::Off, 0.0);
    assert_close(off, 0.3 / 2f32.sqrt(), 0.01, "untouched level (RMS of a 0.3 sine)");

    let track = play_rms("tagged.flac", ReplayGainMode::Track, 0.0);
    assert_close(track / off, db(-6.5), 0.01, "track gain -6.5 dB");

    let album = play_rms("tagged.flac", ReplayGainMode::Album, 0.0);
    assert_close(album / off, db(-5.25), 0.01, "album gain -5.25 dB");

    let boosted = play_rms("tagged.flac", ReplayGainMode::Track, 6.0);
    assert_close(boosted / off, db(-0.5), 0.01, "-6.5 dB plus a +6 dB pre-amp");
}

#[test]
fn replaygain_never_boosts_a_track_past_its_own_peak() {
    let off = play_rms("tagged.flac", ReplayGainMode::Off, 0.0);
    // +10 dB pre-amp would be +3.5 dB overall, but the 0.9 peak tag caps the gain at 1/0.9.
    let capped = play_rms("tagged.flac", ReplayGainMode::Track, 10.0);
    assert_close(capped / off, 1.0 / 0.9, 0.01, "limited by the peak tag");
}

#[test]
fn untagged_files_are_unchanged_by_replaygain() {
    let off = play_rms("chirp_44k_stereo.flac", ReplayGainMode::Off, 0.0);
    let on = play_rms("chirp_44k_stereo.flac", ReplayGainMode::Track, 6.0);
    assert_close(on / off, 1.0, 0.005, "no tags, no change");
}

#[test]
fn changing_replaygain_applies_to_the_playing_track_and_the_preloaded_one() {
    // Two copies back to back, played at half speed so there's time to change the setting
    // part-way through the first. The second was already preloaded at the old setting.
    let mut r = rig(0.5);
    r.player.set_queue(paths(&["tagged.flac", "tagged.flac"]), 0, true);
    r.wait_position("0.35 s in", |p| p > 0.35);
    r.player.set_replaygain(ReplayGainMode::Track, 0.0);
    r.wait_ended();

    let left = r.left();
    let (a, b) = audible(&left);
    let before = rms(&left[a + 4800..a + 9600]); // 0.1 s - 0.2 s into the first track
    let first_end = rms(&left[a + 33_600..a + 43_200]); // 0.7 s - 0.9 s into the first
    let second = rms(&left[b - 14_400..b - 4_800]); // near the end of the second
    assert_close(before, 0.3 / 2f32.sqrt(), 0.01, "starts untouched");
    assert_close(first_end / before, db(-6.5), 0.02, "the playing track was reloaded at the new gain");
    assert_close(second / before, db(-6.5), 0.02, "so was the preloaded next track");
}

#[test]
fn replaygain_change_while_paused_stays_paused_and_keeps_position() {
    let r = rig(1.0);
    r.player.set_queue(paths(&["tagged.flac"]), 0, true);
    r.wait_position("0.3 s in", |p| p > 0.3);
    r.player.pause();
    std::thread::sleep(Duration::from_millis(150));
    let before = r.player.position_secs();
    r.player.set_replaygain(ReplayGainMode::Album, 0.0);
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(r.player.status(), Status::Paused);
    assert_close(r.player.position_secs() as f32, before as f32, 0.05, "position held");
    let s = r.player.state();
    assert_eq!(s.replaygain, ReplayGainMode::Album);
    r.player.stop();
}

#[test]
fn replaygain_settings_are_clamped_and_readable() {
    let r = rig(1.0);
    assert_eq!(r.player.replaygain(), (ReplayGainMode::Off, 0.0));
    r.player.set_replaygain(ReplayGainMode::Track, 99.0);
    assert_eq!(r.player.replaygain(), (ReplayGainMode::Track, 12.0));
    r.player.set_replaygain(ReplayGainMode::Track, f32::NAN);
    assert_eq!(r.player.replaygain(), (ReplayGainMode::Track, 0.0), "NaN pre-amp is ignored");
    r.player.set_replaygain(ReplayGainMode::Album, -99.0);
    assert_eq!(r.player.replaygain(), (ReplayGainMode::Album, -12.0));
    let s = r.player.state();
    assert_eq!((s.replaygain, s.replaygain_preamp_db), (ReplayGainMode::Album, -12.0));
}
