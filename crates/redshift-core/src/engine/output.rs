//! Getting audio out: a real device through cpal (WASAPI on Windows), or an offline sink that runs
//! the very same mixer without hardware, which is how the engine is tested end to end.
//!
//! Both run the mixer on a dedicated thread. cpal streams aren't `Send` on every platform, so the
//! stream is created, played and dropped on that one thread.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{FromSample, SampleFormat, SizedSample};
use crossbeam_channel::{bounded, unbounded, Receiver};
use parking_lot::Mutex;
use serde::Serialize;

use super::exclusive;
use super::mixer::Mixer;
use crate::error::{Error, Result};

/// Frames rendered per mixer call inside the device callback; larger device buffers are split.
const MAX_RENDER_FRAMES: usize = 4096;

#[derive(Debug, Clone, Serialize)]
pub struct DeviceInfo {
    /// Stable identifier to pass back to [`OutputKind::Device`].
    pub id: String,
    pub name: String,
    pub is_default: bool,
}

pub fn list_output_devices() -> Vec<DeviceInfo> {
    let host = cpal::default_host();
    let default_id = host.default_output_device().and_then(|d| d.id().ok()).map(|id| id.to_string());
    let Ok(devices) = host.output_devices() else { return Vec::new() };
    devices
        .filter_map(|d| {
            let id = d.id().ok()?.to_string();
            let name = d.description().map(|desc| desc.name().to_string()).unwrap_or_else(|_| id.clone());
            Some(DeviceInfo { is_default: Some(&id) == default_id.as_ref(), id, name })
        })
        .collect()
}

/// What the output actually is, for the readout in the app.
#[derive(Debug, Clone, Serialize, Default, PartialEq)]
pub struct OutputInfo {
    pub device_name: String,
    pub sample_rate: u32,
    /// The device was opened exclusively (nothing else is mixed in, nothing resamples).
    pub exclusive: bool,
    /// Bits per sample sent to the device (valid bits for integer formats).
    pub bits: u8,
    /// Floating point samples (shared mode through Windows usually is).
    pub float: bool,
    /// Why exclusive mode was asked for but not used, when that happened.
    pub fallback: Option<String>,
}

/// How to open a hardware output.
#[derive(Debug, Clone, Default)]
pub struct DeviceOpts {
    /// Device id, or `None` for the system default.
    pub id: Option<String>,
    /// Try to take the device exclusively, at the rate below (Windows only).
    pub exclusive: bool,
    /// The rate to open at in exclusive mode; shared mode always uses the device's own rate.
    pub rate_hint: Option<u32>,
}

pub enum OutputKind {
    /// A hardware device.
    Device(DeviceOpts),
    /// No hardware: render at `sample_rate` into `sink`, `speed` times faster than real time. With
    /// `follow_rate`, a `rate_hint` is honoured the way an exclusive device would (for tests).
    Offline { sample_rate: u32, sink: Arc<Mutex<Vec<f32>>>, speed: f32, follow_rate: bool, rate_hint: Option<u32> },
}

/// A running output. Dropping it stops the audio thread and releases the device.
pub struct OutputHandle {
    pub sample_rate: u32,
    pub device_name: String,
    pub info: OutputInfo,
    /// Stream errors, as text, for the player to react to (a device that vanished, say).
    pub errors: Receiver<String>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl OutputHandle {
    /// Stops the stream and waits for the device to be released (exclusive mode needs that before
    /// the device can be opened again).
    pub fn close(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for OutputHandle {
    fn drop(&mut self) {
        self.close();
    }
}

/// Starts an output. `make_mixer` is called on the audio thread, once the device's sample rate is
/// known, so the mixer can be built for it.
pub fn start_output(
    kind: OutputKind,
    make_mixer: impl FnOnce(u32) -> Mixer + Send + 'static,
) -> Result<OutputHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = bounded::<Result<OutputInfo>>(1);
    let (err_tx, err_rx) = unbounded::<String>();
    let thread_stop = stop.clone();

    let thread = thread::Builder::new()
        .name("redshift-audio".into())
        .spawn(move || match kind {
            OutputKind::Device(opts) => run_device(opts, make_mixer, thread_stop, ready_tx, err_tx),
            OutputKind::Offline { sample_rate, sink, speed, follow_rate, rate_hint } => {
                let sample_rate = if follow_rate { rate_hint.unwrap_or(sample_rate) } else { sample_rate };
                let info = OutputInfo {
                    device_name: "offline".into(),
                    sample_rate,
                    exclusive: follow_rate,
                    // A rate-following offline output stands in for an exclusive integer device.
                    bits: if follow_rate { 24 } else { 32 },
                    float: !follow_rate,
                    fallback: None,
                };
                let _ = ready_tx.send(Ok(info));
                run_offline(make_mixer(sample_rate), sample_rate, sink, speed, thread_stop);
            }
        })
        .map_err(Error::Io)?;

    match ready_rx.recv_timeout(Duration::from_secs(10)) {
        Ok(Ok(info)) => Ok(OutputHandle {
            sample_rate: info.sample_rate,
            device_name: info.device_name.clone(),
            info,
            errors: err_rx,
            stop,
            thread: Some(thread),
        }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => {
            stop.store(true, Ordering::Release);
            Err(Error::Device("timed out opening the audio device".into()))
        }
    }
}

fn run_offline(mut mixer: Mixer, sample_rate: u32, sink: Arc<Mutex<Vec<f32>>>, speed: f32, stop: Arc<AtomicBool>) {
    const BLOCK: usize = 480;
    let pause = Duration::from_secs_f64(BLOCK as f64 / sample_rate as f64 / speed.max(0.1) as f64);
    let mut buf = vec![0.0f32; BLOCK * 2];
    while !stop.load(Ordering::Acquire) {
        mixer.render(&mut buf);
        sink.lock().extend_from_slice(&buf);
        thread::sleep(pause);
    }
}

fn run_device(
    opts: DeviceOpts,
    make_mixer: impl FnOnce(u32) -> Mixer,
    stop: Arc<AtomicBool>,
    ready: crossbeam_channel::Sender<Result<OutputInfo>>,
    errors: crossbeam_channel::Sender<String>,
) {
    // Exclusive first, when asked for. If it can't be had, say why and carry on in shared mode.
    let mut fallback = None;
    if opts.exclusive {
        match exclusive::prepare(&opts.id, opts.rate_hint) {
            Ok(prepared) => {
                let info = OutputInfo {
                    device_name: prepared.device_name.clone(),
                    sample_rate: prepared.rate,
                    exclusive: true,
                    bits: prepared.format.valid_bits(),
                    float: false,
                    fallback: None,
                };
                let mixer = make_mixer(prepared.rate);
                let _ = ready.send(Ok(info));
                exclusive::run(prepared, mixer, stop, errors);
                return;
            }
            Err(reason) => fallback = Some(reason),
        }
    }
    let id = opts.id;
    let setup = || -> Result<(cpal::Device, cpal::SupportedStreamConfig, String)> {
        let host = cpal::default_host();
        let device = match &id {
            Some(id) => {
                let parsed = id.parse().map_err(|e| Error::Device(format!("bad device id {id:?}: {e}")))?;
                host.device_by_id(&parsed).ok_or_else(|| Error::Device(format!("device {id} is not available")))?
            }
            None => host.default_output_device().ok_or_else(|| Error::Device("no audio output device found".into()))?,
        };
        let name = device.description().map(|d| d.name().to_string()).unwrap_or_else(|_| "unknown device".into());
        let config = device.default_output_config().map_err(|e| Error::Device(format!("{name}: {e}")))?;
        Ok((device, config, name))
    };
    let (device, config, name) = match setup() {
        Ok(v) => v,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };

    let sample_rate = config.sample_rate();
    let mixer = make_mixer(sample_rate);
    let format = config.sample_format();
    let stream_config = config.config();
    let built = match format {
        SampleFormat::F32 => build::<f32>(&device, &stream_config, mixer, errors),
        SampleFormat::I16 => build::<i16>(&device, &stream_config, mixer, errors),
        SampleFormat::I32 => build::<i32>(&device, &stream_config, mixer, errors),
        SampleFormat::U16 => build::<u16>(&device, &stream_config, mixer, errors),
        SampleFormat::F64 => build::<f64>(&device, &stream_config, mixer, errors),
        other => Err(Error::Device(format!("{name}: unsupported sample format {other}"))),
    };
    let stream = match built.and_then(|s| s.play().map(|_| s).map_err(|e| Error::Device(format!("{name}: {e}")))) {
        Ok(s) => s,
        Err(e) => {
            let _ = ready.send(Err(e));
            return;
        }
    };
    let info = OutputInfo {
        device_name: name,
        sample_rate,
        exclusive: false,
        bits: (format.sample_size() * 8) as u8,
        float: format.is_float(),
        fallback,
    };
    let _ = ready.send(Ok(info));

    while !stop.load(Ordering::Acquire) {
        thread::sleep(Duration::from_millis(50));
    }
    drop(stream);
}

fn build<T>(
    device: &cpal::Device,
    config: &cpal::StreamConfig,
    mut mixer: Mixer,
    errors: crossbeam_channel::Sender<String>,
) -> Result<cpal::Stream>
where
    T: SizedSample + FromSample<f32>,
{
    let channels = config.channels as usize;
    let mut scratch = vec![0.0f32; MAX_RENDER_FRAMES * 2];
    device
        .build_output_stream(
            config.clone(),
            move |data: &mut [T], _| {
                for block in data.chunks_mut(MAX_RENDER_FRAMES * channels) {
                    let frames = block.len() / channels;
                    mixer.render(&mut scratch[..frames * 2]);
                    for (i, frame) in block.chunks_exact_mut(channels).enumerate() {
                        let (l, r) = (scratch[i * 2], scratch[i * 2 + 1]);
                        match channels {
                            1 => frame[0] = T::from_sample(0.5 * (l + r)),
                            _ => {
                                frame[0] = T::from_sample(l);
                                frame[1] = T::from_sample(r);
                                // Any further channels (surround, multichannel interfaces) stay silent.
                                for s in &mut frame[2..] {
                                    *s = T::from_sample(0.0);
                                }
                            }
                        }
                    }
                }
            },
            move |err| {
                let _ = errors.send(format!("{} ({:?})", err, err.kind()));
            },
            None,
        )
        .map_err(|e| Error::Device(e.to_string()))
}
