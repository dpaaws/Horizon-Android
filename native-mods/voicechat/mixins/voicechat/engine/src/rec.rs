//! A voice note, like in a chat app: record what the microphone sends, then
//! play it back to hear what the other players hear (after the clean-up, the
//! voice changer and the codec).

use crate::mix::{Frame, Mixer};
use crate::net::Shared;
use crate::opus::{Decoder, Encoder, APPLICATION_VOIP, FRAME};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// The longest note, in 20 ms frames (30 s).
pub const MAX_FRAMES: usize = 50 * 30;
/// The mixer channel a note plays on.
pub const PLAYBACK: u128 = u128::MAX - 1;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum RecState {
    #[default]
    Idle,
    Recording,
    Playing,
}

#[derive(Default)]
pub struct Rec {
    pub state: RecState,
    frames: Vec<Vec<i16>>,
    /// The loudest sample of each frame, 0..=255, for the picture.
    peaks: Vec<u8>,
    codec: Option<(Encoder, Decoder)>,
    /// Frames played so far.
    played: usize,
    /// Bumped on every start and stop, so an old playback thread gives up.
    generation: u64,
}

/// What the menu shows of the note.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RecView {
    pub state: RecState,
    pub seconds: f32,
    /// A bar chart of the note, each 0..=1.
    pub bars: Vec<f32>,
    /// How far playback is, 0..=1.
    pub progress: f32,
}

impl Rec {
    pub fn has_note(&self) -> bool {
        !self.frames.is_empty()
    }

    pub fn busy(&self) -> bool {
        self.state != RecState::Idle
    }

    /// Adds one frame while recording. The frame goes through the codec and
    /// back, so the note sounds as it does to others.
    pub fn feed(&mut self, pcm: &[i16; FRAME]) {
        if self.state != RecState::Recording {
            return;
        }
        let heard = match &mut self.codec {
            Some((enc, dec)) => match enc.encode(pcm) {
                Ok(data) => dec.decode(Some(&data), false),
                Err(_) => pcm.to_vec(),
            },
            None => pcm.to_vec(),
        };
        let peak = heard.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        self.peaks.push((u32::from(peak) * 255 / 32768).min(255) as u8);
        self.frames.push(heard);
        if self.frames.len() >= MAX_FRAMES {
            self.state = RecState::Idle;
        }
    }

    pub fn start_recording(&mut self) {
        self.generation += 1;
        self.frames.clear();
        self.peaks.clear();
        self.played = 0;
        self.codec = match (Encoder::new(APPLICATION_VOIP, 1000), Decoder::new()) {
            (Ok(e), Ok(d)) => Some((e, d)),
            _ => None,
        };
        self.state = RecState::Recording;
    }

    pub fn stop(&mut self) {
        self.generation += 1;
        self.state = RecState::Idle;
        self.codec = None;
    }

    pub fn clear(&mut self) {
        self.stop();
        self.frames.clear();
        self.peaks.clear();
        self.played = 0;
    }

    pub fn view(&self, bars: usize) -> RecView {
        let n = self.peaks.len();
        let mut out = Vec::new();
        if n > 0 && bars > 0 {
            for i in 0..bars.min(n) {
                let (a, b) = (i * n / bars.min(n), ((i + 1) * n / bars.min(n)).max(i * n / bars.min(n) + 1));
                let m = self.peaks[a..b.min(n)].iter().copied().max().unwrap_or(0);
                // Quiet voices still show: a gentle curve.
                out.push((f32::from(m) / 255.0).powf(0.6));
            }
        }
        RecView {
            state: self.state,
            seconds: self.frames.len() as f32 * 0.02,
            bars: out,
            progress: if n == 0 { 0.0 } else { self.played as f32 / n as f32 },
        }
    }
}

/// Plays the note on the mixer, one frame every 20 ms, until it ends or is
/// stopped. Returns false when there was nothing to play.
pub fn play(shared: &Arc<Shared>, mixer: &Arc<Mutex<Mixer>>, gain: f32) -> bool {
    let (frames, mine) = {
        let mut r = shared.rec.lock().unwrap();
        if r.frames.is_empty() || r.state == RecState::Recording {
            return false;
        }
        r.generation += 1;
        r.state = RecState::Playing;
        r.played = 0;
        (r.frames.clone(), r.generation)
    };
    let (worker_shared, worker_mixer) = (shared.clone(), mixer.clone());
    let spawned = std::thread::Builder::new().name("voicechat-note".into()).spawn(move || {
        let start = Instant::now();
        for (i, f) in frames.iter().enumerate() {
            {
                let mut r = worker_shared.rec.lock().unwrap();
                if r.generation != mine {
                    worker_mixer.lock().unwrap().end(PLAYBACK);
                    return;
                }
                r.played = i + 1;
            }
            let pcm = f.iter().map(|s| f32::from(*s) / 32768.0).collect();
            worker_mixer.lock().unwrap().push(PLAYBACK, Frame { pcm, gain: [gain; 2] });
            let due = start + Duration::from_millis(20 * (i as u64 + 1));
            std::thread::sleep(due.saturating_duration_since(Instant::now()));
        }
        let mut r = worker_shared.rec.lock().unwrap();
        if r.generation == mine {
            r.state = RecState::Idle;
            r.played = 0;
        }
        worker_mixer.lock().unwrap().end(PLAYBACK);
    });
    if spawned.is_err() {
        shared.rec.lock().unwrap().state = RecState::Idle;
        return false;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(v: i16) -> [i16; FRAME] {
        [v; FRAME]
    }

    #[test]
    fn records_only_while_recording_and_stops_when_full() {
        let mut r = Rec::default();
        r.feed(&frame(1000));
        assert!(!r.has_note());
        r.start_recording();
        // No codec on this machine is fine: the frames are kept as they are.
        for _ in 0..MAX_FRAMES + 10 {
            r.feed(&frame(1000));
        }
        assert_eq!(r.state, RecState::Idle);
        assert_eq!(r.frames.len(), MAX_FRAMES);
        assert!(r.has_note());
    }

    #[test]
    fn the_picture_follows_the_loudness() {
        let mut r = Rec::default();
        r.start_recording();
        r.codec = None;
        for k in 0..100 {
            r.feed(&frame(if k < 50 { 300 } else { 30_000 }));
        }
        let v = r.view(10);
        assert_eq!(v.bars.len(), 10);
        assert!(v.bars[0] < v.bars[9] && v.bars[9] > 0.9, "{:?}", v.bars);
        assert!((v.seconds - 2.0).abs() < 0.01);
        r.clear();
        assert!(r.view(10).bars.is_empty() && !r.has_note());
    }

    #[test]
    fn playback_runs_to_the_end_and_can_be_stopped() {
        let shared = Arc::new(Shared::default());
        let mixer = Arc::new(Mutex::new(Mixer::default()));
        assert!(!play(&shared, &mixer, 1.0), "nothing recorded yet");
        {
            let mut r = shared.rec.lock().unwrap();
            r.start_recording();
            r.codec = None;
            for _ in 0..10 {
                r.feed(&frame(1000));
            }
            r.stop();
        }
        assert!(play(&shared, &mixer, 1.0));
        assert_eq!(shared.rec.lock().unwrap().state, RecState::Playing);
        std::thread::sleep(Duration::from_millis(400));
        assert_eq!(shared.rec.lock().unwrap().state, RecState::Idle);
        // Stop part way.
        assert!(play(&shared, &mixer, 1.0));
        shared.rec.lock().unwrap().stop();
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(shared.rec.lock().unwrap().state, RecState::Idle);
    }
}
