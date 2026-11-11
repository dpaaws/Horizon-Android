//! The sound card: the microphone feeds 48 kHz mono samples to the mic
//! thread, the speakers pull from the mixer. cpal's streams live on a thread
//! of their own, as they aren't `Send` everywhere. On Android the streams are AAudio's (`aaudio`).
use crate::mix::{Mixer, Resampler};
use crate::opus::SAMPLE_RATE;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

/// Microphone samples waiting for the mic thread; a second at most.
#[derive(Default)]
pub struct MicQueue {
    pub samples: Mutex<VecDeque<f32>>,
    pub ready: Condvar,
}

impl MicQueue {
    pub fn push(&self, samples: &[f32]) {
        let mut q = self.samples.lock().unwrap();
        q.extend(samples);
        let excess = q.len().saturating_sub(SAMPLE_RATE as usize);
        q.drain(..excess);
        drop(q);
        self.ready.notify_one();
    }
}

/// What opened, for the HUD and `/vc status`.
#[derive(Clone, Debug)]
pub struct Devices {
    pub input: Result<String, String>,
    pub output: Result<String, String>,
}

impl Default for Devices {
    fn default() -> Self {
        Self { input: Err("not open".into()), output: Err("not open".into()) }
    }
}

pub struct AudioIo {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AudioIo {
    pub fn start(mixer: Arc<Mutex<Mixer>>, mic: Arc<MicQueue>, devices: Arc<Mutex<Devices>>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("voicechat-audio".into())
            .spawn(move || backend::run(mixer, mic, devices, flag))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for AudioIo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[cfg(not(target_os = "android"))]
use cpal_backend as backend;
#[cfg(target_os = "android")]
use aaudio as backend;

#[cfg(not(target_os = "android"))]
mod cpal_backend {
    use super::*;
    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{SampleFormat, SizedSample, Stream, StreamConfig, SupportedStreamConfig};

    pub fn run(mixer: Arc<Mutex<Mixer>>, mic: Arc<MicQueue>, devices: Arc<Mutex<Devices>>, flag: Arc<AtomicBool>) {
        let host = cpal::default_host();
        let (output, out_info) = match open_output(&host, mixer) {
            Ok((s, name)) => (Some(s), Ok(name)),
            Err(e) => (None, Err(e)),
        };
        let (input, in_info) = match open_input(&host, mic) {
            Ok((s, name)) => (Some(s), Ok(name)),
            Err(e) => (None, Err(e)),
        };
        *devices.lock().unwrap() = Devices { input: in_info, output: out_info };
        while !flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
        drop(input);
        drop(output);
    }

    /// The device's config at 48 kHz when it has one, else its own rate.
    fn choose(
        configs: Vec<cpal::SupportedStreamConfigRange>,
        default: Option<SupportedStreamConfig>,
    ) -> Option<SupportedStreamConfig> {
        let usable = |f: SampleFormat| matches!(f, SampleFormat::F32 | SampleFormat::I16 | SampleFormat::I32 | SampleFormat::U16);
        let at_48k = |c: &cpal::SupportedStreamConfigRange| {
            c.min_sample_rate().0 <= SAMPLE_RATE && SAMPLE_RATE <= c.max_sample_rate().0 && usable(c.sample_format())
        };
        let mut best: Vec<_> = configs.into_iter().filter(at_48k).collect();
        // Fewer channels and float samples first.
        best.sort_by_key(|c| (c.channels(), c.sample_format() != SampleFormat::F32));
        best.into_iter()
            .next()
            .map(|c| c.with_sample_rate(cpal::SampleRate(SAMPLE_RATE)))
            .or(default.filter(|d| usable(d.sample_format())))
    }

    fn open_output(host: &cpal::Host, mixer: Arc<Mutex<Mixer>>) -> Result<(Stream, String), String> {
        let device = host.default_output_device().ok_or("no speakers")?;
        let name = device.name().unwrap_or_else(|_| "speakers".into());
        let configs = device.supported_output_configs().map(|c| c.collect()).unwrap_or_default();
        let supported = choose(configs, device.default_output_config().ok()).ok_or("no usable speaker format")?;
        let config: StreamConfig = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => output::<f32>(&device, &config, mixer, |v| v),
            SampleFormat::I16 => output::<i16>(&device, &config, mixer, |v| (v * 32767.0) as i16),
            SampleFormat::I32 => output::<i32>(&device, &config, mixer, |v| (v as f64 * 2147483647.0) as i32),
            SampleFormat::U16 => output::<u16>(&device, &config, mixer, |v| ((v + 1.0) * 32767.5) as u16),
            _ => return Err("no usable speaker format".into()),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, name))
    }

    fn output<T: SizedSample + Send + 'static>(
        device: &cpal::Device,
        config: &StreamConfig,
        mixer: Arc<Mutex<Mixer>>,
        convert: fn(f32) -> T,
    ) -> Result<Stream, String> {
        let channels = config.channels as usize;
        let rate = config.sample_rate.0;
        device
            .build_output_stream(
                config,
                move |data: &mut [T], _| {
                    let mut m = mixer.lock().unwrap();
                    for frame in data.chunks_mut(channels) {
                        let [l, r] = m.sample(rate);
                        match frame.len() {
                            1 => frame[0] = convert((l + r) * 0.5),
                            _ => {
                                frame[0] = convert(l);
                                frame[1] = convert(r);
                                for s in &mut frame[2..] {
                                    *s = convert(0.0);
                                }
                            }
                        }
                    }
                },
                |e| eprintln!("voicechat: speaker error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())
    }

    fn open_input(host: &cpal::Host, mic: Arc<MicQueue>) -> Result<(Stream, String), String> {
        let device = host.default_input_device().ok_or("no microphone")?;
        let name = device.name().unwrap_or_else(|_| "microphone".into());
        let configs = device.supported_input_configs().map(|c| c.collect()).unwrap_or_default();
        let supported = choose(configs, device.default_input_config().ok()).ok_or("no usable microphone format")?;
        let config: StreamConfig = supported.config();
        let stream = match supported.sample_format() {
            SampleFormat::F32 => input::<f32>(&device, &config, mic, |v| v),
            SampleFormat::I16 => input::<i16>(&device, &config, mic, |v| v as f32 / 32768.0),
            SampleFormat::I32 => input::<i32>(&device, &config, mic, |v| (v as f64 / 2147483648.0) as f32),
            SampleFormat::U16 => input::<u16>(&device, &config, mic, |v| v as f32 / 32768.0 - 1.0),
            _ => return Err("no usable microphone format".into()),
        }?;
        stream.play().map_err(|e| e.to_string())?;
        Ok((stream, name))
    }

    fn input<T: SizedSample + Send + 'static>(
        device: &cpal::Device,
        config: &StreamConfig,
        mic: Arc<MicQueue>,
        convert: fn(T) -> f32,
    ) -> Result<Stream, String> {
        let channels = config.channels as usize;
        let mut resampler = Resampler::new(config.sample_rate.0);
        let mut mono = Vec::new();
        device
            .build_input_stream(
                config,
                move |data: &[T], _| {
                    mono.clear();
                    for frame in data.chunks(channels) {
                        let sum: f32 = frame.iter().map(|s| convert(*s)).sum();
                        resampler.push(sum / frame.len() as f32, &mut mono);
                    }
                    mic.push(&mono);
                },
                |e| eprintln!("voicechat: microphone error: {e}"),
                None,
            )
            .map_err(|e| e.to_string())
    }
}

#[cfg(target_os = "android")]
mod aaudio;
