//! The microphone test when no voice connection is running (no server
//! mod, or not joined yet): it listens and plays back on its own.

use crate::audio::{AudioIo, Devices, MicQueue};
use crate::mix::{Frame, Mixer};
use crate::net::{LOOPBACK, MicChain, Shared, test_hears};
use crate::opus::FRAME;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

pub struct LocalCheck {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    mixer: Arc<Mutex<Mixer>>,
    // Dropped last: it owns the sound devices.
    _audio: AudioIo,
}

impl LocalCheck {
    pub fn start(shared: Arc<Shared>, mixer: Arc<Mutex<Mixer>>, mic: Arc<MicQueue>, devices: Arc<Mutex<Devices>>) -> Self {
        let audio = AudioIo::start(mixer.clone(), mic.clone(), devices);
        let stop = Arc::new(AtomicBool::new(false));
        let (flag, m) = (stop.clone(), mixer.clone());
        let thread = std::thread::Builder::new()
            .name("voicechat-check".into())
            .spawn(move || {
                let mut chain = MicChain::default();
                while !flag.load(Ordering::Relaxed) {
                    let frame: Vec<f32> = {
                        let mut q = mic.samples.lock().unwrap();
                        if q.len() < FRAME {
                            q = mic.ready.wait_timeout(q, Duration::from_millis(100)).unwrap().0;
                        }
                        if q.len() < FRAME {
                            continue;
                        }
                        q.drain(..FRAME).collect()
                    };
                    let c = shared.controls.lock().unwrap().clone();
                    let (pcm, db) = chain.run(frame, &c, &shared);
                    let mut mix = m.lock().unwrap();
                    if c.loopback && test_hears(&c, db) {
                        let pcm = pcm.iter().map(|s| f32::from(*s) / 32768.0).collect();
                        mix.push(LOOPBACK, Frame { pcm, gain: [c.volume.min(1.0); 2] });
                    } else {
                        mix.end(LOOPBACK);
                    }
                }
            })
            .ok();
        Self { stop, thread, mixer, _audio: audio }
    }
}

impl Drop for LocalCheck {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
        self.mixer.lock().unwrap().end(LOOPBACK);
    }
}
