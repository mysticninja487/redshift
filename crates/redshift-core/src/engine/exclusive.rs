//! Exclusive-mode output on Windows (WASAPI), so a song can reach the device at its own sample rate
//! and bit depth with nothing in between: no Windows mixer, no resampling.
//!
//! The shared-mode output in `output.rs` always runs at the rate Windows has chosen in Sound
//! settings, and Windows mixes our audio with everything else. Exclusive mode takes the device for
//! Redshift alone. It can fail (another program holds the device, the driver refuses the rate, or
//! exclusive mode is switched off for the device in Windows), and when it does the caller falls back
//! to shared mode and says why.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crossbeam_channel::Sender;

use super::mixer::Mixer;
use super::pcm::PcmFormat;

/// Rates tried, in the order [`rate_candidates`] wants them.
const STANDARD_RATES: [u32; 8] = [44_100, 48_000, 88_200, 96_000, 176_400, 192_000, 352_800, 384_000];

/// Formats tried at each rate: the widest first, since a 16-bit file is still bit-perfect inside
/// 24 bits (the extra bits are zeros), and the fewest dither/rounding questions arise there.
pub const FORMAT_ORDER: [PcmFormat; 4] = [PcmFormat::Int24In32, PcmFormat::Int24Packed, PcmFormat::Int32, PcmFormat::Int16];

/// The sample rates to ask the device for, best first: the song's own rate, then whole multiples of
/// it (a 44.1 kHz song is fine at 88.2 or 176.4 kHz, with simple resampling and no drift between
/// rate families), then the other standard rates above it, then the ones below.
pub fn rate_candidates(want: u32) -> Vec<u32> {
    let mut out = vec![want];
    let push = |r: u32, out: &mut Vec<u32>| {
        if !out.contains(&r) {
            out.push(r);
        }
    };
    for k in [2, 4, 8] {
        let r = want.saturating_mul(k);
        if r <= 384_000 {
            push(r, &mut out);
        }
    }
    for r in STANDARD_RATES {
        if r > want {
            push(r, &mut out);
        }
    }
    for r in STANDARD_RATES.iter().rev() {
        if *r < want {
            push(*r, &mut out);
        }
    }
    out
}

/// A stream that is open and ready to run.
pub struct Prepared {
    pub rate: u32,
    pub format: PcmFormat,
    pub device_name: String,
    #[cfg(windows)]
    inner: imp::Opened,
}

/// Opens `id` (or the default device) in exclusive mode, as near to `want_rate` as the device allows.
/// `Err` carries a short reason for the user.
pub fn prepare(id: &Option<String>, want_rate: Option<u32>) -> Result<Prepared, String> {
    #[cfg(windows)]
    {
        imp::prepare(id, want_rate)
    }
    #[cfg(not(windows))]
    {
        let _ = (id, want_rate);
        Err("exclusive mode is only available on Windows".into())
    }
}

/// Runs the stream, rendering `mixer` into it, until `stop` is set or the device fails.
pub fn run(prepared: Prepared, mixer: Mixer, stop: Arc<AtomicBool>, errors: Sender<String>) {
    #[cfg(windows)]
    {
        imp::run(prepared, mixer, stop, errors)
    }
    #[cfg(not(windows))]
    {
        let _ = (prepared, mixer, errors);
        while !stop.load(Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(windows)]
mod imp {
    use std::time::Duration;

    use crate::engine::pcm::Dither;

    use wasapi::{
        calculate_period_100ns, initialize_mta, AudioClient, AudioRenderClient, DeviceEnumerator, Direction, Handle,
        SampleType, StreamMode, WasapiError, WaveFormat,
    };

    use super::*;

    const MAX_RENDER_FRAMES: usize = 4096;
    const AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED: i32 = 0x8889_0019_u32 as i32;
    const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x8889_0004_u32 as i32;
    const AUDCLNT_E_DEVICE_IN_USE: i32 = 0x8889_000A_u32 as i32;
    const AUDCLNT_E_UNSUPPORTED_FORMAT: i32 = 0x8889_0008_u32 as i32;
    const AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED: i32 = 0x8889_000E_u32 as i32;
    const AUDCLNT_E_RESOURCES_INVALIDATED: i32 = 0x8889_0026_u32 as i32;

    pub struct Opened {
        client: AudioClient,
        render: AudioRenderClient,
        event: Handle,
        buffer_frames: u32,
    }

    fn code_of(e: &WasapiError) -> Option<i32> {
        match e {
            WasapiError::Windows(w) => Some(w.code().0),
            _ => None,
        }
    }

    /// A short, human reason for a failed open.
    fn explain(e: &WasapiError) -> String {
        match code_of(e) {
            Some(AUDCLNT_E_DEVICE_IN_USE) => "another program is using the device".into(),
            Some(AUDCLNT_E_EXCLUSIVE_MODE_NOT_ALLOWED) => {
                "exclusive mode is switched off for this device in Windows (Sound settings > device Properties > Advanced)".into()
            }
            Some(AUDCLNT_E_UNSUPPORTED_FORMAT) => "the device does not accept this rate or format".into(),
            _ => e.to_string(),
        }
    }

    pub fn prepare(id: &Option<String>, want_rate: Option<u32>) -> Result<Prepared, String> {
        // COM may already be set up on this thread; either result is fine.
        let _ = initialize_mta();
        let enumerator = DeviceEnumerator::new().map_err(|e| e.to_string())?;
        let device = match id {
            Some(id) => {
                let raw = id.strip_prefix("wasapi:").unwrap_or(id);
                enumerator.get_device(raw).map_err(|e| format!("device not found ({e})"))?
            }
            None => enumerator.get_default_device(Direction::Render).map_err(|e| e.to_string())?,
        };
        let device_name = device.get_friendlyname().unwrap_or_else(|_| "unknown device".into());
        let want = match want_rate {
            Some(r) => r,
            None => device.get_device_format().map(|f| f.get_samplespersec()).unwrap_or(48_000),
        };

        let mut last_error: Option<String> = None;
        for rate in rate_candidates(want) {
            for format in FORMAT_ORDER {
                let mut client = match device.get_iaudioclient() {
                    Ok(c) => c,
                    Err(e) => return Err(explain(&e)),
                };
                let wanted = WaveFormat::new(
                    format.container_bytes() * 8,
                    format.valid_bits() as usize,
                    SampleType::Int,
                    rate as usize,
                    2,
                    None,
                );
                let Ok(fmt) = client.is_supported_exclusive_with_quirks(&wanted) else { continue };
                let (default_period, _min) = match client.get_device_period() {
                    Ok(p) => p,
                    Err(e) => {
                        last_error = Some(explain(&e));
                        continue;
                    }
                };
                // 128-byte alignment satisfies Intel HD Audio devices, which want it.
                let Ok(period) = client.calculate_aligned_period_near(default_period, Some(128), &fmt) else { continue };
                let mode = StreamMode::EventsExclusive { period_hns: period };
                let mut init = client.initialize_client(&fmt, Direction::Render, mode, None);
                if let Err(e) = &init {
                    if code_of(e) == Some(AUDCLNT_E_BUFFER_SIZE_NOT_ALIGNED) {
                        // The documented recovery: ask for the aligned size the device wants, start over.
                        if let Ok(frames) = client.get_buffer_size() {
                            let aligned = calculate_period_100ns(frames as i64, rate as i64);
                            if let Ok(mut again) = device.get_iaudioclient() {
                                let mode = StreamMode::EventsExclusive { period_hns: aligned };
                                init = again.initialize_client(&fmt, Direction::Render, mode, None);
                                if init.is_ok() {
                                    client = again;
                                }
                            }
                        }
                    }
                }
                if let Err(e) = init {
                    last_error = Some(explain(&e));
                    continue;
                }
                let event = client.set_get_eventhandle().map_err(|e| explain(&e))?;
                let render = client.get_audiorenderclient().map_err(|e| explain(&e))?;
                let buffer_frames = client.get_buffer_size().map_err(|e| explain(&e))?;
                return Ok(Prepared {
                    rate,
                    format,
                    device_name,
                    inner: Opened { client, render, event, buffer_frames },
                });
            }
        }
        Err(last_error.unwrap_or_else(|| "the device accepts none of the rates and formats tried".into()))
    }

    pub fn run(prepared: Prepared, mut mixer: Mixer, stop: Arc<AtomicBool>, errors: Sender<String>) {
        let Prepared { format, inner, .. } = prepared;
        let Opened { client, render, event, buffer_frames } = inner;
        let mut scratch = vec![0.0f32; MAX_RENDER_FRAMES * 2];
        let mut bytes: Vec<u8> = Vec::new();
        let mut dither = Dither::new();

        // Renders `frames` frames and hands them to the device in one write.
        let mut fill = |frames: usize, mixer: &mut Mixer| -> Result<(), WasapiError> {
            bytes.clear();
            let mut left = frames;
            let use_dither = format == PcmFormat::Int16 && mixer.dither_wanted();
            while left > 0 {
                let n = left.min(MAX_RENDER_FRAMES);
                mixer.render(&mut scratch[..n * 2]);
                super::super::pcm::pack(&scratch[..n * 2], format, use_dither.then_some(&mut dither), &mut bytes);
                left -= n;
            }
            render.write_to_device(&bytes, None)
        };

        let report = |e: &WasapiError| {
            let lost = matches!(
                code_of(e),
                Some(AUDCLNT_E_DEVICE_INVALIDATED) | Some(AUDCLNT_E_RESOURCES_INVALIDATED)
            );
            let _ = errors.send(if lost { format!("DeviceNotAvailable ({e})") } else { format!("exclusive stream: {e}") });
        };

        // Start with a full buffer so playback begins with real audio, not silence.
        let first = client.get_available_space_in_frames().unwrap_or(buffer_frames) as usize;
        if let Err(e) = fill(first, &mut mixer) {
            report(&e);
            return;
        }
        if let Err(e) = client.start_stream() {
            report(&e);
            return;
        }
        let mut stalls = 0;
        while !stop.load(Ordering::Acquire) {
            if event.wait_for_event(200).is_err() {
                stalls += 1;
                if stalls > 25 {
                    let _ = errors.send("DeviceNotAvailable (the exclusive stream stopped responding)".into());
                    break;
                }
                continue;
            }
            stalls = 0;
            let frames = match client.get_available_space_in_frames() {
                Ok(f) => f as usize,
                Err(e) => {
                    report(&e);
                    break;
                }
            };
            if frames > 0 {
                if let Err(e) = fill(frames, &mut mixer) {
                    report(&e);
                    break;
                }
            }
        }
        let _ = client.stop_stream();
        // Give the driver a moment to let go of the buffer before the client is dropped.
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rates_are_tried_own_rate_first_then_multiples_then_the_rest() {
        assert_eq!(rate_candidates(44_100), vec![44_100, 88_200, 176_400, 352_800, 48_000, 96_000, 192_000, 384_000]);
        assert_eq!(rate_candidates(96_000), vec![96_000, 192_000, 384_000, 176_400, 352_800]
            .into_iter().chain([88_200, 48_000, 44_100]).collect::<Vec<_>>());
        let c = rate_candidates(48_000);
        assert_eq!(c[0], 48_000);
        assert!(c.iter().all(|r| *r <= 384_000));
        let mut sorted = c.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), c.len(), "no rate is tried twice");
        // An odd rate (a 22.05 kHz song) still gets a sensible list that includes the standard ones.
        let odd = rate_candidates(22_050);
        assert_eq!(&odd[..4], &[22_050, 44_100, 88_200, 176_400]);
        assert!(odd.contains(&48_000));
    }

    #[test]
    fn widest_format_is_tried_first() {
        assert_eq!(FORMAT_ORDER[0], PcmFormat::Int24In32);
        assert_eq!(*FORMAT_ORDER.last().unwrap(), PcmFormat::Int16);
    }
}
