//! Voices to the speakers: each channel's decoded frames wait in a short
//! buffer, then play mixed at 48 kHz, panned and faded by distance as
//! Simple Voice Chat's OpenAL sources are (linear distance model, reference
//! distance 0).
use crate::opus::FRAME;
use std::collections::{HashMap, VecDeque};

/// Frames a voice holds back before it plays: 60 ms against jitter.
const PREBUFFER: usize = 3;
/// More than this waiting means the voice fell behind: drop the oldest.
const MAX_QUEUED: usize = 16;

/// One 20 ms mono frame with its left and right gains.
pub struct Frame {
    pub pcm: Vec<f32>,
    pub gain: [f32; 2],
}

#[derive(Default)]
struct Voice {
    frames: VecDeque<Frame>,
    playing: bool,
    /// The sender stopped: play out what is left without waiting.
    ended: bool,
}

#[derive(Default)]
pub struct Mixer {
    voices: HashMap<u128, Voice>,
    /// Mixed 48 kHz stereo not yet played.
    out: VecDeque<[f32; 2]>,
    /// The resampler: the last two source samples and the position between.
    prev: [f32; 2],
    next: [f32; 2],
    phase: f64,
}

impl Mixer {
    pub fn push(&mut self, channel: u128, frame: Frame) {
        let v = self.voices.entry(channel).or_default();
        v.ended = false;
        v.frames.push_back(frame);
        while v.frames.len() > MAX_QUEUED {
            v.frames.pop_front();
        }
    }

    pub fn end(&mut self, channel: u128) {
        if let Some(v) = self.voices.get_mut(&channel) {
            v.ended = true;
        }
    }

    pub fn clear(&mut self) {
        self.voices.clear();
        self.out.clear();
    }

    /// Mixes the next 20 ms of every voice that is ready.
    fn render(&mut self) {
        let mut block = vec![[0f32; 2]; FRAME];
        self.voices.retain(|_, v| {
            if !v.playing && (v.frames.len() >= PREBUFFER || (v.ended && !v.frames.is_empty())) {
                v.playing = true;
            }
            if v.playing {
                match v.frames.pop_front() {
                    Some(f) => {
                        for (o, s) in block.iter_mut().zip(&f.pcm) {
                            o[0] += s * f.gain[0];
                            o[1] += s * f.gain[1];
                        }
                    }
                    // Ran dry: wait for the buffer to fill again.
                    None => v.playing = false,
                }
            }
            !(v.ended && v.frames.is_empty())
        });
        self.out.extend(block.into_iter().map(|[l, r]| [soft_clip(l), soft_clip(r)]));
    }

    fn source(&mut self) -> [f32; 2] {
        if self.out.is_empty() {
            self.render();
        }
        self.out.pop_front().unwrap_or_default()
    }

    /// The next stereo sample at the device's `rate`.
    pub fn sample(&mut self, rate: u32) -> [f32; 2] {
        if rate == crate::opus::SAMPLE_RATE {
            return self.source();
        }
        self.phase += f64::from(crate::opus::SAMPLE_RATE) / f64::from(rate);
        while self.phase >= 1.0 {
            self.phase -= 1.0;
            self.prev = self.next;
            self.next = self.source();
        }
        let t = self.phase as f32;
        [self.prev[0] + (self.next[0] - self.prev[0]) * t, self.prev[1] + (self.next[1] - self.prev[1]) * t]
    }

    pub fn voices(&self) -> usize {
        self.voices.len()
    }
}

fn soft_clip(x: f32) -> f32 {
    if x.abs() <= 0.9 { x } else { x.signum() * (0.9 + (x.abs() - 0.9).tanh() * 0.1) }
}

/// Converts any rate to 48 kHz, for the microphone.
pub struct Resampler {
    rate: u32,
    phase: f64,
    prev: f32,
}

impl Resampler {
    pub fn new(rate: u32) -> Self {
        Self { rate, phase: 0.0, prev: 0.0 }
    }

    pub fn push(&mut self, sample: f32, out: &mut impl Extend<f32>) {
        if self.rate == crate::opus::SAMPLE_RATE {
            out.extend([sample]);
            return;
        }
        let step = f64::from(self.rate) / f64::from(crate::opus::SAMPLE_RATE);
        while self.phase < 1.0 {
            let t = self.phase as f32;
            out.extend([self.prev + (sample - self.prev) * t]);
            self.phase += step;
        }
        self.phase -= 1.0;
        self.prev = sample;
    }
}

/// Where the player hears from: the eyes and the way they look.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Listener {
    pub eye: [f64; 3],
    /// Degrees; 0 faces south (+Z), 90 west.
    pub yaw: f32,
}

/// Left and right gains for a voice at `at`, heard up to `max_distance`:
/// none past it, full at the listener, linear between, and panned with equal
/// power by the side it comes from. None when out of reach.
pub fn spatial(listener: &Listener, at: [f64; 3], max_distance: f32) -> Option<[f32; 2]> {
    let d = [at[0] - listener.eye[0], at[1] - listener.eye[1], at[2] - listener.eye[2]];
    let distance = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt() as f32;
    if distance > max_distance + 1.0 {
        return None;
    }
    let gain = (1.0 - distance / max_distance.max(0.001)).clamp(0.0, 1.0);
    let yaw = listener.yaw.to_radians();
    // Facing south, the right hand points west (-X).
    let right = [-yaw.cos() as f64, -yaw.sin() as f64];
    let flat = (d[0] * d[0] + d[2] * d[2]).sqrt();
    let pan = if flat < 1e-6 { 0.0 } else { ((d[0] * right[0] + d[2] * right[1]) / flat) as f32 };
    // A source straight ahead sounds as loud as one heard without panning.
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    let norm = std::f32::consts::SQRT_2;
    Some([gain * angle.cos() * norm, gain * angle.sin() * norm])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: f32) -> Frame {
        Frame { pcm: vec![v; FRAME], gain: [1.0, 1.0] }
    }

    #[test]
    fn waits_for_the_buffer_then_plays() {
        let mut m = Mixer::default();
        m.push(1, frame(0.5));
        assert_eq!(m.sample(48_000), [0.0, 0.0]);
        for _ in 0..FRAME - 1 {
            m.sample(48_000);
        }
        m.push(1, frame(0.5));
        m.push(1, frame(0.5));
        assert_eq!(m.sample(48_000), [0.5, 0.5]);
    }

    #[test]
    fn an_ended_voice_plays_out_and_goes() {
        let mut m = Mixer::default();
        m.push(1, frame(0.25));
        m.end(1);
        assert_eq!(m.sample(48_000), [0.25, 0.25]);
        assert_eq!(m.voices(), 0);
    }

    #[test]
    fn distance_fades_linearly_and_pans_by_side() {
        let l = Listener { eye: [0.0, 64.0, 0.0], yaw: 0.0 };
        let ahead = spatial(&l, [0.0, 64.0, 24.0], 48.0).unwrap();
        assert!((ahead[0] - 0.5).abs() < 1e-4 && (ahead[1] - 0.5).abs() < 1e-4);
        // Facing south, west (-X) is on the right.
        let west = spatial(&l, [-10.0, 64.0, 0.0], 48.0).unwrap();
        assert!(west[1] > 0.9 && west[0] < 1e-3);
        assert!(spatial(&l, [0.0, 64.0, 60.0], 48.0).is_none());
        let close = spatial(&l, [0.0, 64.0, 0.0], 48.0).unwrap();
        assert!((close[0] - 1.0).abs() < 1e-4);
    }

    #[test]
    fn resamples_to_48k() {
        let mut r = Resampler::new(44_100);
        let mut out = Vec::new();
        for _ in 0..44_100 {
            r.push(0.1, &mut out);
        }
        assert!((out.len() as i64 - 48_000).abs() <= 2, "{}", out.len());
    }
}
