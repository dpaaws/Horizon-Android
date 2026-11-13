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
}
