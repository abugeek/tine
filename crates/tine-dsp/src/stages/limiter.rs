//! Stage 6: Brickwall lookahead limiter with optional 4x oversampled true-peak detection.

use core::f64::consts::PI;

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::db::db_to_lin;
use crate::primitives::delay::DelayLine;
use crate::primitives::math::{exp_f64, sin_cos_f64, sqrt_f64};
use crate::primitives::sliding_max::SlidingMax;

/// Lookahead time in milliseconds.
const LOOKAHEAD_MS: f32 = 1.5;

/// Exponential release time constant in milliseconds.
const RELEASE_MS: f32 = 50.0;

/// Maximum supported sample rate for preallocation in `prepare`.
const MAX_SAMPLE_RATE: f32 = 192_000.0;

/// Maximum lookahead samples at 192 kHz (`round(1.5 ms * 192 kHz) = 288`).
const MAX_LOOKAHEAD_SAMPLES: usize = 288;

/// Number of taps per polyphase FIR branch for 4x oversampled true-peak detection.
const FIR_TAPS: usize = 9;

/// Group delay (in 1x samples) of the symmetric `FIR_TAPS`-tap polyphase interpolator.
const FIR_HALF: usize = FIR_TAPS / 2;

/// Kaiser window shape parameter for the 4x polyphase FIR.
const KAISER_BETA: f64 = 6.5;

/// Computes the zeroth-order modified Bessel function of the first kind `I0(x)`.
fn bessel_i0(x: f64) -> f64 {
    let mut sum = 1.0;
    let mut term = 1.0;
    let half_x = 0.5 * x;
    let half_x_sq = half_x * half_x;
    for k in 1..25 {
        let kf = f64::from(k);
        term *= half_x_sq / (kf * kf);
        sum += term;
        if term < 1e-16 * sum {
            break;
        }
    }
    sum
}

/// Designs one normalized polyphase FIR branch at fractional offset `mu` in `(0, 1)`.
fn design_polyphase_branch(mu: f64) -> [f32; FIR_TAPS] {
    let mut coeffs = [0.0f64; FIR_TAPS];
    let inv_i0_beta = 1.0 / bessel_i0(KAISER_BETA);
    let radius = (FIR_HALF as f64) + 0.5;
    let mut sum = 0.0;

    for (k, slot) in coeffs.iter_mut().enumerate() {
        let t = (FIR_HALF as f64) - (k as f64) - mu;
        let sinc = if t.abs() < 1e-12 {
            1.0
        } else {
            let pit = PI * t;
            let (s, _) = sin_cos_f64(pit);
            s / pit
        };
        let norm = t / radius;
        let win = if norm.abs() >= 1.0 {
            0.0
        } else {
            bessel_i0(KAISER_BETA * sqrt_f64(1.0 - norm * norm)) * inv_i0_beta
        };
        let v = sinc * win;
        *slot = v;
        sum += v;
    }

    let mut out = [0.0f32; FIR_TAPS];
    let inv_sum = if sum.abs() > 1e-12 { 1.0 / sum } else { 1.0 };
    for (dst, &src) in out.iter_mut().zip(coeffs.iter()) {
        *dst = (src * inv_sum) as f32;
    }
    out
}

/// Power-of-two ring capacity for `fir_hist`.
const FIR_RING_CAP: usize = 16;
const FIR_RING_MASK: usize = FIR_RING_CAP - 1;

/// Stage 6: Brickwall lookahead limiter with optional 4x oversampled true-peak detection.
///
/// The polyphase FIR group delay (`FIR_HALF` = 4 samples) is always included in the
/// audio delay line so toggling `limiter_true_peak` keeps latency constant.
#[derive(Debug)]
pub struct LimiterStage {
    lookahead_samples: usize,
    total_latency: usize,
    release_coeff: f32,
    delay: DelayLine,
    sliding_max: SlidingMax,
    fir_hist: [f32; FIR_RING_CAP],
    fir_pos: usize,
    fir_phases: [[f32; FIR_TAPS]; 3],
    gain: f32,
    ramp_step: f32,
    #[cfg(debug_assertions)]
    safety_clamp_count: u64,
}

impl Default for LimiterStage {
    fn default() -> Self {
        Self::new()
    }
}

impl LimiterStage {
    /// Creates a new [`LimiterStage`] preallocated for up to 192 kHz.
    #[must_use]
    pub fn new() -> Self {
        let fir_phases = [
            design_polyphase_branch(0.25),
            design_polyphase_branch(0.50),
            design_polyphase_branch(0.75),
        ];
        let mut stage = Self {
            lookahead_samples: 72,
            total_latency: 72 + FIR_HALF,
            release_coeff: 0.0,
            delay: DelayLine::new(MAX_LOOKAHEAD_SAMPLES + FIR_HALF),
            sliding_max: SlidingMax::new(MAX_LOOKAHEAD_SAMPLES + 1),
            fir_hist: [0.0; FIR_RING_CAP],
            fir_pos: 0,
            fir_phases,
            gain: 1.0,
            ramp_step: 0.0,
            #[cfg(debug_assertions)]
            safety_clamp_count: 0,
        };
        stage.prepare(48_000.0, 512);
        stage
    }

    /// Returns the number of times the final safety clamp engaged (debug builds only).
    #[cfg(debug_assertions)]
    #[must_use]
    pub const fn safety_clamp_count(&self) -> u64 {
        self.safety_clamp_count
    }

    #[inline]
    fn eval_true_peak(&self) -> f32 {
        let center_idx = self.fir_pos.wrapping_sub(FIR_HALF) & FIR_RING_MASK;
        let mut peak = self.fir_hist.get(center_idx).copied().unwrap_or(0.0).abs();
        for coeffs in &self.fir_phases {
            let mut acc = 0.0f32;
            for (k, &c) in coeffs.iter().enumerate() {
                let idx = self.fir_pos.wrapping_sub(k) & FIR_RING_MASK;
                acc += self.fir_hist.get(idx).copied().unwrap_or(0.0) * c;
            }
            let abs_acc = acc.abs();
            if abs_acc > peak {
                peak = abs_acc;
            }
        }
        peak
    }
}

impl Stage for LimiterStage {
    fn prepare(&mut self, fs: f32, _max_block: usize) {
        let fs = if fs.is_finite() && fs > 0.0 {
            fs.min(MAX_SAMPLE_RATE)
        } else {
            48_000.0
        };
        let lookahead = ((f64::from(LOOKAHEAD_MS) * 1e-3 * f64::from(fs)) + 0.5) as usize;
        self.lookahead_samples = lookahead.clamp(1, MAX_LOOKAHEAD_SAMPLES);
        self.total_latency = self.lookahead_samples + FIR_HALF;
        self.release_coeff = exp_f64(-1.0 / (f64::from(RELEASE_MS) * 1e-3 * f64::from(fs))) as f32;
        if self.delay.max_delay() < self.total_latency {
            self.delay = DelayLine::new(MAX_LOOKAHEAD_SAMPLES + FIR_HALF);
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.delay.reset();
        self.sliding_max.reset();
        self.fir_hist.fill(0.0);
        self.fir_pos = 0;
        self.gain = 1.0;
        self.ramp_step = 0.0;
        #[cfg(debug_assertions)]
        {
            self.safety_clamp_count = 0;
        }
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, _m: &mut StageMeters) {
        let ceiling_db = if p.limiter_ceiling_db.is_finite() {
            p.limiter_ceiling_db.clamp(-12.0, -0.1)
        } else {
            -1.0
        };
        let ceiling = db_to_lin(ceiling_db);
        let true_peak = p.limiter_true_peak;
        let lookahead = self.lookahead_samples;
        let total_delay = self.total_latency;
        let window = lookahead + 1;
        let ramp_div = (lookahead.saturating_sub(1)).max(1) as f32;
        let rel_coeff = self.release_coeff;

        let mut gain = self.gain;
        let mut ramp_step = self.ramp_step;

        for sample in buf.iter_mut() {
            let x = *sample;
            let delayed_x = self.delay.push_read(x, total_delay);

            self.fir_pos = self.fir_pos.wrapping_add(1) & FIR_RING_MASK;
            if let Some(slot) = self.fir_hist.get_mut(self.fir_pos) {
                *slot = if x.is_finite() { x } else { 0.0 };
            }

            let det_peak = if true_peak {
                self.eval_true_peak()
            } else {
                let center_idx = self.fir_pos.wrapping_sub(FIR_HALF) & FIR_RING_MASK;
                self.fir_hist.get(center_idx).copied().unwrap_or(0.0).abs()
            };

            let max_peak = self.sliding_max.push(det_peak, window);
            let target_gain = if max_peak > ceiling {
                let raw = (f64::from(ceiling) / f64::from(max_peak)) as f32;
                // Bias down by 1 ULP so `max_peak * target_gain <= ceiling` under f32 rounding.
                f32::from_bits(raw.to_bits().saturating_sub(1))
            } else {
                1.0
            };

            if target_gain < gain {
                let needed_step = (gain - target_gain) / ramp_div;
                if needed_step > ramp_step {
                    ramp_step = needed_step;
                }
                gain = (gain - ramp_step).max(target_gain);
            } else {
                ramp_step = 0.0;
                gain = target_gain + rel_coeff * (gain - target_gain);
            }

            let mut out = delayed_x * gain;
            if out > ceiling {
                #[cfg(debug_assertions)]
                {
                    self.safety_clamp_count = self.safety_clamp_count.saturating_add(1);
                }
                out = ceiling;
            } else if out < -ceiling {
                #[cfg(debug_assertions)]
                {
                    self.safety_clamp_count = self.safety_clamp_count.saturating_add(1);
                }
                out = -ceiling;
            }

            *sample = out;
        }

        self.gain = gain;
        self.ramp_step = ramp_step;
    }

    fn latency(&self) -> u32 {
        self.total_latency as u32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safety_clamp_never_engages_over_1m_samples() {
        let mut limiter = LimiterStage::new();
        limiter.prepare(48_000.0, 480);
        let mut p = DspParams::DEFAULT;
        p.limiter_ceiling_db = -1.0;
        p.limiter_true_peak = false;
        let ceiling = db_to_lin(-1.0);
        let mut meters = StageMeters::default();

        let mut rng = 0x1234_5678_9abc_def1u64;
        let mut block = [0.0f32; 480];
        for b in 0..2084 {
            for (i, slot) in block.iter_mut().enumerate() {
                rng ^= rng >> 12;
                rng ^= rng << 25;
                rng ^= rng >> 27;
                let u = ((rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 40) as f32) / 16_777_216.0;
                *slot = if b % 4 == 0 && i == 10 {
                    16.0
                } else {
                    (u * 2.0 - 1.0) * 12.0
                };
            }
            limiter.process(&mut block, &p, &mut meters);
            for &s in &block {
                assert!(s.abs() <= ceiling, "sample exceeded ceiling: {s}");
            }
        }
        #[cfg(debug_assertions)]
        assert_eq!(limiter.safety_clamp_count(), 0, "safety clamp engaged!");
    }

    #[test]
    fn below_ceiling_is_bit_exact_apart_from_delay() {
        let fs = 48_000.0;
        let mut limiter = LimiterStage::new();
        limiter.prepare(fs, 480);
        let delay = limiter.latency() as usize;
        assert!(delay >= 72);

        let mut p = DspParams::DEFAULT;
        p.limiter_ceiling_db = -1.0;
        p.limiter_true_peak = false;
        let mut meters = StageMeters::default();

        let w = 2.0 * core::f32::consts::PI * 440.0 / fs;
        let input: alloc::vec::Vec<f32> = (0..4800).map(|n| 0.5 * (w * (n as f32)).sin()).collect();
        let mut output = input.clone();
        for chunk in output.chunks_mut(480) {
            limiter.process(chunk, &p, &mut meters);
        }
        assert_eq!(&output[delay..], &input[..input.len() - delay]);
    }
}
