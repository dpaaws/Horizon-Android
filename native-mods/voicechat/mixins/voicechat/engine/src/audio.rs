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
use null_backend as backend;
#[cfg(target_os = "android")]
use aaudio as backend;

/// Anywhere but Android (a host build of the workspace): no sound card is opened. The sound is
/// Android's AAudio; a bundle crate can't carry a platform-specific dependency for another one.
#[cfg(not(target_os = "android"))]
mod null_backend {
    use super::*;

    pub fn run(_mixer: Arc<Mutex<Mixer>>, _mic: Arc<MicQueue>, devices: Arc<Mutex<Devices>>, flag: Arc<AtomicBool>) {
        let unavailable = || Err("sound is only available on Android in this build".to_string());
        *devices.lock().unwrap() = Devices { input: unavailable(), output: unavailable() };
        while !flag.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}

#[cfg(target_os = "android")]
mod aaudio;
