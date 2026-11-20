//! Microphone processing, 100 % local: RNNoise suppression and a noise gate.
//! Operates on interleaved stereo i16 at 48 kHz.

use std::collections::VecDeque;

use nnnoiseless::DenoiseState;

const FRAME: usize = DenoiseState::FRAME_SIZE; // 480 samples = 10 ms

/// RNNoise denoiser with a constant 10 ms latency (output length == input).
pub struct Denoiser {
    state: Box<DenoiseState<'static>>,
    input: Vec<f32>,
    output: VecDeque<f32>,
    frame_out: Vec<f32>,
}

impl Default for Denoiser {
    fn default() -> Self {
        Self::new()
    }
}

impl Denoiser {
    pub fn new() -> Self {
        Self {
            state: DenoiseState::new(),
            input: Vec::with_capacity(FRAME),
            // Pre-roll keeps the output exactly as long as the input.
            output: std::iter::repeat_n(0.0, FRAME).collect(),
            frame_out: vec![0.0; FRAME],
        }
    }

    /// Process stereo samples in place (downmixed to mono for the model).
    pub fn process(&mut self, stereo: &mut [i16]) {
        for pair in stereo.as_chunks::<2>().0 {
            self.input
                .push((f32::from(pair[0]) + f32::from(pair[1])) * 0.5);
            if self.input.len() == FRAME {
                self.state.process_frame(&mut self.frame_out, &self.input);
                self.output.extend(self.frame_out.iter().copied());
                self.input.clear();
            }
        }
        for pair in stereo.as_chunks_mut::<2>().0 {
            let v = self
                .output
                .pop_front()
                .unwrap_or(0.0)
                .clamp(-32768.0, 32767.0) as i16;
            pair[0] = v;
            pair[1] = v;
        }
    }
}

/// Simple downward gate with attack/release smoothing.
pub struct NoiseGate {
    threshold: f32,
    gain: f32,
    attack: f32,
    release: f32,
    hold_left: usize,
    hold: usize,
}

impl NoiseGate {
    pub fn new(threshold_db: i32) -> Self {
        let rate = 48_000.0f32;
        Self {
            threshold: 10f32.powf(threshold_db as f32 / 20.0) * 32768.0,
            gain: 0.0,
            // Per-sample smoothing coefficients: ~5 ms open, ~150 ms close.
            attack: 1.0 - (-1.0 / (0.005 * rate)).exp(),
            release: 1.0 - (-1.0 / (0.150 * rate)).exp(),
            hold_left: 0,
            hold: (0.100 * rate) as usize,
        }
    }

    pub fn process(&mut self, stereo: &mut [i16]) {
        // Level detection per 10 ms window, gain smoothing per sample.
        for window in stereo.chunks_mut(FRAME * 2) {
            let sum: f64 = window.iter().map(|s| f64::from(*s) * f64::from(*s)).sum();
            let rms = (sum / window.len().max(1) as f64).sqrt() as f32;
            let open = rms >= self.threshold;
            if open {
                self.hold_left = self.hold;
            }
            for pair in window.as_chunks_mut::<2>().0 {
                let target = if open || self.hold_left > 0 { 1.0 } else { 0.0 };
                self.hold_left = self.hold_left.saturating_sub(1);
                let k = if target > self.gain {
                    self.attack
                } else {
                    self.release
                };
                self.gain += (target - self.gain) * k;
                for s in pair {
                    *s = (f32::from(*s) * self.gain) as i16;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tone(len: usize, amp: f32) -> Vec<i16> {
        (0..len)
            .flat_map(|i| {
                let v = ((i as f32 / 48_000.0) * 440.0 * std::f32::consts::TAU).sin() * amp;
                [v as i16, v as i16]
            })
            .collect()
    }

    fn rms(s: &[i16]) -> f32 {
        (s.iter().map(|v| f32::from(*v).powi(2)).sum::<f32>() / s.len() as f32).sqrt()
    }

    #[test]
    fn gate_closes_on_quiet_and_opens_on_loud() {
        let mut g = NoiseGate::new(-40);
        let mut quiet = tone(48_000, 30.0); // ~ -60 dBFS
        g.process(&mut quiet);
        assert!(rms(&quiet[48_000..]) < 1.0);
        let mut loud = tone(48_000, 8000.0);
        g.process(&mut loud);
        assert!(rms(&loud[48_000..]) > 5000.0);
    }

    #[test]
    fn denoiser_keeps_length_and_reduces_noise() {
        let mut d = Denoiser::new();
        // White-ish noise from an LCG.
        let mut x = 12345u32;
        let mut noise: Vec<i16> = (0..96_000)
            .map(|_| {
                x = x.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((x >> 16) as i16) / 16
            })
            .collect();
        let before = rms(&noise);
        let len = noise.len();
        // Feed in odd-sized chunks like WASAPI does.
        for chunk in noise.chunks_mut(2 * 441) {
            d.process(chunk);
        }
        assert_eq!(noise.len(), len);
        let after = rms(&noise[48_000..]);
        assert!(after < before * 0.5, "before {before} after {after}");
    }
}
