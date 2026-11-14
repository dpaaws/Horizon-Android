//! Microphone clean-up, cheap enough for a phone: a high-pass filter drops
//! rumble and DC, a soft expander fades out steady background noise between
//! words, and a slow automatic gain evens out a quiet or loud microphone.
//! All of it works in place on one 20 ms frame of 48 kHz mono.

/// High-pass corner, in Hz.
const CORNER_HZ: f32 = 90.0;
const RATE: f32 = 48_000.0;
/// The level the automatic gain aims for (RMS, linear).
const TARGET_RMS: f32 = 0.08;
/// Never boost more than this (about +18 dB) or cut below this.
const MAX_GAIN: f32 = 8.0;
const MIN_GAIN: f32 = 0.5;

pub struct Cleaner {
    /// One-pole high-pass state.
    x1: f32,
    y1: f32,
    a: f32,
    /// Noise floor estimate (RMS), follows the quietest recent frames.
    floor: f32,
    /// Smoothed expander gain, 0..1.
    open: f32,
    /// Smoothed automatic gain.
    gain: f32,
}

impl Default for Cleaner {
    fn default() -> Self {
        let rc = 1.0 / (2.0 * std::f32::consts::PI * CORNER_HZ);
        let dt = 1.0 / RATE;
        Self { x1: 0.0, y1: 0.0, a: rc / (rc + dt), floor: 0.002, open: 1.0, gain: 1.0 }
    }
}

fn rms(frame: &[f32]) -> f32 {
    if frame.is_empty() {
        return 0.0;
    }
    (frame.iter().map(|s| s * s).sum::<f32>() / frame.len() as f32).sqrt()
}

impl Cleaner {
    /// Cleans `frame` in place. `noise` turns the expander on, `auto` the
    /// automatic gain. The high-pass always runs.
    pub fn process(&mut self, frame: &mut [f32], noise: bool, auto: bool) {
        for s in frame.iter_mut() {
            let y = self.a * (self.y1 + *s - self.x1);
            self.x1 = *s;
            self.y1 = y;
            *s = y;
        }
        let level = rms(frame);
        // The floor drops to quiet frames at once and creeps up slowly, so
        // speech does not raise it.
        if level < self.floor {
            self.floor = level.max(0.0005);
        } else {
            self.floor = (self.floor * 1.002).min(0.05);
        }
        let speech = level > self.floor * 3.0 + 0.004;
        if noise {
            let target = if speech { 1.0 } else { 0.15 };
            // Open fast, close slowly: no clipped word starts.
            let k = if target > self.open { 0.6 } else { 0.08 };
            let from = self.open;
            self.open += (target - self.open) * k;
            ramp(frame, from, self.open);
        } else {
            self.open = 1.0;
        }
        if auto {
            if speech && level > 0.0 {
                let want = (TARGET_RMS / level).clamp(MIN_GAIN, MAX_GAIN);
                // Slow to follow, so it does not pump.
                self.gain += (want - self.gain) * 0.04;
            }
            let g = self.gain;
            for s in frame.iter_mut() {
                *s = (*s * g).clamp(-1.0, 1.0);
            }
        } else {
            self.gain = 1.0;
        }
    }
}

/// Fades `frame` from gain `a` to gain `b` across its length.
fn ramp(frame: &mut [f32], a: f32, b: f32) {
    let n = frame.len().max(1) as f32;
    for (i, s) in frame.iter_mut().enumerate() {
        *s *= a + (b - a) * (i as f32 / n);
    }
}

/// Estimates a voice's usual pitch (F0) from its frames. It listens at a
/// quarter of the sample rate, so it costs very little.
#[derive(Default)]
pub struct PitchTracker {
    /// Smoothed pitch in Hz, 0 until known.
    f0: f32,
    voiced: u32,
}

impl PitchTracker {
    const FIRST: usize = 30; // 400 Hz at 12 kHz
    const LAST: usize = 171; // 70 Hz

    /// Feeds one frame of 48 kHz samples (-1..1).
    pub fn feed(&mut self, frame: &[f32]) {
        let low: Vec<f32> = frame.chunks_exact(4).map(|c| (c[0] + c[1] + c[2] + c[3]) * 0.25).collect();
        if low.len() <= Self::LAST + 8 {
            return;
        }
        let r0: f32 = low.iter().map(|s| s * s).sum();
        if r0 / (low.len() as f32) < 1e-4 {
            return; // silence or hiss
        }
        let n = low.len();
        let r = |lag: usize| -> f32 { (0..n - lag).map(|i| low[i] * low[i + lag]).sum::<f32>() / r0 };
        let (mut best, mut at) = (0.0f32, 0usize);
        for lag in Self::FIRST..=Self::LAST {
            let v = r(lag);
            if v > best {
                best = v;
                at = lag;
            }
        }
        if best < 0.5 || at == 0 {
            return;
        }
        // Prefer the shorter period when it is nearly as strong, which
        // avoids reading an octave too low.
        let half = at / 2;
        if half >= Self::FIRST {
            let alt = (half.saturating_sub(1)..=half + 1).map(|l| (r(l), l)).fold((0.0, 0), |a, b| if b.0 > a.0 { b } else { a });
            if alt.0 >= best * 0.85 {
                at = alt.1;
            }
        }
        let hz = 12_000.0 / at as f32;
        self.voiced += 1;
        self.f0 = if self.f0 == 0.0 { hz } else { (self.f0.ln() * 0.9 + hz.ln() * 0.1).exp() };
    }

    /// The pitch once enough speech has been heard.
    pub fn f0(&self) -> Option<f32> {
        (self.voiced >= 25 && self.f0 > 0.0).then_some(self.f0)
    }
}

/// Voice changer presets. `Off` leaves the voice alone.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Fx {
    #[default]
    Off,
    Deep,
    Giant,
    High,
    Robot,
    Radio,
    /// Match another player's pitch (set from the Players tab).
    Copy,
}

impl Fx {
    pub const ALL: [Fx; 7] = [Fx::Off, Fx::Deep, Fx::Giant, Fx::High, Fx::Robot, Fx::Radio, Fx::Copy];

    pub fn name(self) -> &'static str {
        match self {
            Fx::Off => "Off",
            Fx::Deep => "Deep",
            Fx::Giant => "Giant",
            Fx::High => "High",
            Fx::Robot => "Robot",
            Fx::Radio => "Radio",
            Fx::Copy => "Copy voice",
        }
    }

    pub fn from_index(i: usize) -> Fx {
        Self::ALL.get(i).copied().unwrap_or(Fx::Off)
    }

    pub fn index(self) -> usize {
        Self::ALL.iter().position(|f| *f == self).unwrap_or(0)
    }

    /// The button's next preset. Copy is only chosen from the Players tab.
    pub fn next(self) -> Fx {
        match self {
            Fx::Copy | Fx::Radio => Fx::Off,
            f => Self::from_index(f.index() + 1),
        }
    }
}

/// Delay-line window of the pitch shifter, in samples (about 21 ms).
const WINDOW: usize = 1024;

/// The voice changer. Pitch is shifted with two crossfading read taps on a
/// short delay line, which is cheap and keeps the speech understandable.
pub struct Changer {
    buf: Vec<f32>,
    write: usize,
    phase: f32,
    t: f32,
    lp1: f32,
    lp2: f32,
}

impl Default for Changer {
    fn default() -> Self {
        Self { buf: vec![0.0; WINDOW * 2], write: 0, phase: 0.0, t: 0.0, lp1: 0.0, lp2: 0.0 }
    }
}

impl Changer {
    fn pitch(&mut self, x: f32, ratio: f32) -> f32 {
        let n = self.buf.len();
        self.buf[self.write] = x;
        let mut out = 0.0;
        for k in 0..2 {
            let p = (self.phase + k as f32 * 0.5).rem_euclid(1.0);
            let delay = p * WINDOW as f32;
            let pos = (self.write as f32 + n as f32 - delay).rem_euclid(n as f32);
            let i = pos as usize % n;
            let f = pos - pos.floor();
            let s = self.buf[i] * (1.0 - f) + self.buf[(i + 1) % n] * f;
            let g = (std::f32::consts::PI * p).sin().powi(2);
            out += s * g;
        }
        self.write = (self.write + 1) % n;
        self.phase = (self.phase + (1.0 - ratio) / WINDOW as f32).rem_euclid(1.0);
        out
    }

    /// Changes `frame` in place.
    /// `ratio` is the pitch ratio Copy aims for.
    pub fn process(&mut self, frame: &mut [f32], fx: Fx, ratio: f32) {
        match fx {
            Fx::Off => {}
            Fx::Copy => frame.iter_mut().for_each(|s| *s = self.pitch(*s, ratio.clamp(0.5, 2.0))),
            Fx::Deep => frame.iter_mut().for_each(|s| *s = self.pitch(*s, 0.78)),
            Fx::Giant => frame.iter_mut().for_each(|s| *s = self.pitch(*s, 0.6)),
            Fx::High => frame.iter_mut().for_each(|s| *s = self.pitch(*s, 1.5)),
            Fx::Robot => {
                let step = 2.0 * std::f32::consts::PI * 70.0 / RATE;
                for s in frame.iter_mut() {
                    *s *= (self.t).sin();
                    self.t = (self.t + step) % (2.0 * std::f32::consts::PI);
                }
            }
            Fx::Radio => {
                // Roughly 400 Hz to 3 kHz, pushed a little into clipping.
                for s in frame.iter_mut() {
                    self.lp1 += 0.05 * (*s - self.lp1);
                    let hp = *s - self.lp1;
                    self.lp2 += 0.35 * (hp - self.lp2);
                    *s = (self.lp2 * 3.0).tanh() * 0.8;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(amp: f32, hz: f32, n: usize, at: usize) -> Vec<f32> {
        (0..n)
            .map(|i| amp * ((at + i) as f32 * hz * 2.0 * std::f32::consts::PI / RATE).sin())
            .collect()
    }

    #[test]
    fn drops_dc() {
        let mut c = Cleaner::default();
        let mut last = 1.0;
        for _ in 0..50 {
            let mut f = vec![0.3f32; 960];
            c.process(&mut f, false, false);
            last = rms(&f);
        }
        assert!(last < 0.01, "dc left: {last}");
    }

    #[test]
    fn quiet_hiss_is_faded_and_speech_is_kept() {
        let mut c = Cleaner::default();
        let mut hiss = 1.0;
        for k in 0..60 {
            let mut f = tone(0.003, 3000.0, 960, k * 960);
            c.process(&mut f, true, false);
            hiss = rms(&f);
        }
        assert!(hiss < 0.0006, "hiss left: {hiss}");
        let mut loud = 0.0;
        for k in 0..10 {
            let mut f = tone(0.2, 400.0, 960, k * 960);
            c.process(&mut f, true, false);
            loud = rms(&f);
        }
        assert!(loud > 0.12, "speech cut: {loud}");
    }

    #[test]
    fn auto_gain_lifts_a_quiet_voice_toward_the_target() {
        let mut c = Cleaner::default();
        // A noise floor first, then a quiet voice.
        for k in 0..20 {
            let mut f = tone(0.001, 3000.0, 960, k * 960);
            c.process(&mut f, false, true);
        }
        let mut out = 0.0;
        for k in 0..400 {
            let mut f = tone(0.04, 400.0, 960, k * 960);
            c.process(&mut f, false, true);
            out = rms(&f);
        }
        let before = 0.04 / 2f32.sqrt();
        assert!(out > before * 1.5 && out < 0.11, "gain off: {out}");
    }

    /// Frequency from upward zero crossings.
    fn freq(frame: &[f32]) -> f32 {
        let c = frame.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
        c as f32 * RATE / frame.len() as f32
    }

    #[test]
    fn pitch_moves_by_the_preset_ratio() {
        for (fx, ratio) in [(Fx::Deep, 0.78), (Fx::High, 1.5), (Fx::Giant, 0.6)] {
            let mut ch = Changer::default();
            let mut all = Vec::new();
            for k in 0..60 {
                let mut f = tone(0.3, 400.0, 960, k * 960);
                ch.process(&mut f, fx, 1.0);
                if k >= 10 {
                    all.extend(f);
                }
            }
            let got = freq(&all);
            assert!((got / 400.0 - ratio).abs() < 0.06, "{fx:?}: {got} Hz");
        }
    }

    #[test]
    fn every_preset_stays_in_range_and_off_is_untouched() {
        for fx in Fx::ALL {
            let mut ch = Changer::default();
            for k in 0..20 {
                let src = tone(0.5, 300.0, 960, k * 960);
                let mut f = src.clone();
                ch.process(&mut f, fx, 1.0);
                assert!(f.iter().all(|s| s.is_finite() && s.abs() <= 1.5), "{fx:?}");
                if fx == Fx::Off {
                    assert_eq!(f, src);
                }
            }
        }
        assert_eq!(Fx::from_index(Fx::Radio.index()), Fx::Radio);
        assert_eq!(Fx::Radio.next(), Fx::Off);
    }

    /// A voice-like tone: a fundamental with harmonics.
    fn voice(hz: f32, n: usize, at: usize) -> Vec<f32> {
        (0..n)
            .map(|i| {
                let t = (at + i) as f32 / RATE;
                [(1.0, 0.5), (2.0, 0.3), (3.0, 0.2)].iter().map(|(m, a)| a * (2.0 * std::f32::consts::PI * hz * m * t).sin()).sum::<f32>() * 0.4
            })
            .collect()
    }

    #[test]
    fn the_tracker_finds_a_voices_pitch() {
        for hz in [110.0f32, 160.0, 220.0, 300.0] {
            let mut t = PitchTracker::default();
            assert!(t.f0().is_none());
            for k in 0..40 {
                t.feed(&voice(hz, 960, k * 960));
            }
            let got = t.f0().expect("heard");
            assert!((got / hz - 1.0).abs() < 0.05, "{hz} read as {got}");
        }
        let mut t = PitchTracker::default();
        for _ in 0..40 {
            t.feed(&vec![0.0; 960]);
        }
        assert!(t.f0().is_none());
    }

    #[test]
    fn copy_moves_a_voice_to_the_target_pitch() {
        let (own, target) = (120.0f32, 200.0f32);
        let mut ch = Changer::default();
        let mut all = Vec::new();
        for k in 0..60 {
            let mut f = voice(own, 960, k * 960);
            ch.process(&mut f, Fx::Copy, target / own);
            if k >= 10 {
                all.extend(f);
            }
        }
        let mut t = PitchTracker::default();
        for c in all.chunks_exact(960) {
            t.feed(c);
        }
        let got = t.f0().unwrap();
        assert!((got / target - 1.0).abs() < 0.12, "{got}");
    }
}
