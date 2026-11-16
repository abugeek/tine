//! Peak and RMS envelope followers.

use super::math::{exp_f64, sqrt_f32};

#[inline]
fn time_coeff(ms: f32, fs: f32) -> f32 {
    let t = f64::from(ms) * 1e-3;
    let fs_f64 = f64::from(fs);
    if t <= 0.0 || fs_f64 <= 0.0 || !t.is_finite() || !fs_f64.is_finite() {
        0.0
    } else {
        exp_f64(-1.0 / (t * fs_f64)) as f32
    }
}

/// Asymmetric attack/release peak envelope follower.
#[derive(Clone, Copy, Debug)]
pub struct PeakEnvelope {
    env: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl PeakEnvelope {
    /// Creates a new [`PeakEnvelope`] with `attack_ms` and `release_ms` at sample rate `fs`.
    #[must_use]
    pub fn new(attack_ms: f32, release_ms: f32, fs: f32) -> Self {
        let mut e = Self {
            env: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
        };
        e.set_times(attack_ms, release_ms, fs);
        e
    }

    /// Updates attack and release time constants.
    pub fn set_times(&mut self, attack_ms: f32, release_ms: f32, fs: f32) {
        self.attack_coeff = time_coeff(attack_ms, fs);
        self.release_coeff = time_coeff(release_ms, fs);
    }

    /// Returns the current envelope value.
    #[inline]
    #[must_use]
    pub const fn current(&self) -> f32 {
        self.env
    }

    /// Resets the envelope state to `0.0`.
    #[inline]
    pub fn reset(&mut self) {
        self.env = 0.0;
    }

    /// Processes a single sample and returns the updated envelope.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let level = x.abs();
        let coeff = if level > self.env {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.env = level + coeff * (self.env - level);
        self.env
    }

    /// Processes `input` and writes the envelope for each sample into `env_out`.
    pub fn process_block(&mut self, input: &[f32], env_out: &mut [f32]) {
        let att = self.attack_coeff;
        let rel = self.release_coeff;
        let mut env = self.env;
        for (&x, out) in input.iter().zip(env_out.iter_mut()) {
            let level = x.abs();
            let coeff = if level > env { att } else { rel };
            env = level + coeff * (env - level);
            *out = env;
        }
        self.env = env;
    }
}

/// One-pole RMS envelope follower operating on `x^2`.
#[derive(Clone, Copy, Debug)]
pub struct RmsEnvelope {
    mean_sq: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl RmsEnvelope {
    /// Creates a symmetric [`RmsEnvelope`] with a single `window_ms` time constant.
    #[must_use]
    pub fn new(window_ms: f32, fs: f32) -> Self {
        Self::with_attack_release(window_ms, window_ms, fs)
    }

    /// Creates an [`RmsEnvelope`] with separate attack and release time constants on `x^2`.
    #[must_use]
    pub fn with_attack_release(attack_ms: f32, release_ms: f32, fs: f32) -> Self {
        let mut e = Self {
            mean_sq: 0.0,
            attack_coeff: 0.0,
            release_coeff: 0.0,
        };
        e.set_times(attack_ms, release_ms, fs);
        e
    }

    /// Updates the attack and release time constants.
    pub fn set_times(&mut self, attack_ms: f32, release_ms: f32, fs: f32) {
        self.attack_coeff = time_coeff(attack_ms, fs);
        self.release_coeff = time_coeff(release_ms, fs);
    }

    /// Returns the current RMS value.
    #[inline]
    #[must_use]
    pub fn current(&self) -> f32 {
        sqrt_f32(self.mean_sq)
    }

    /// Resets the mean-square accumulator to `0.0`.
    #[inline]
    pub fn reset(&mut self) {
        self.mean_sq = 0.0;
    }

    /// Processes a single sample and returns the updated RMS level.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let sq = x * x;
        let coeff = if sq > self.mean_sq {
            self.attack_coeff
        } else {
            self.release_coeff
        };
        self.mean_sq = sq + coeff * (self.mean_sq - sq);
        sqrt_f32(self.mean_sq)
    }

    /// Processes `input` and writes the RMS envelope for each sample into `env_out`.
    pub fn process_block(&mut self, input: &[f32], env_out: &mut [f32]) {
        let att = self.attack_coeff;
        let rel = self.release_coeff;
        let mut mean_sq = self.mean_sq;
        for (&x, out) in input.iter().zip(env_out.iter_mut()) {
            let sq = x * x;
            let coeff = if sq > mean_sq { att } else { rel };
            mean_sq = sq + coeff * (mean_sq - sq);
            *out = sqrt_f32(mean_sq);
        }
        self.mean_sq = mean_sq;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_tracks_step_and_decays() {
        let mut p = PeakEnvelope::new(1.0, 50.0, 48_000.0);
        for _ in 0..480 {
            p.process_sample(1.0);
        }
        assert!(p.current() > 0.99);
        for _ in 0..2_400 {
            p.process_sample(0.0);
        }
        assert!(p.current() < 0.4 && p.current() > 0.3);
    }

    #[test]
    fn rms_of_full_scale_sine_converges_to_inv_sqrt_2() {
        let fs = 48_000.0;
        let mut r = RmsEnvelope::new(20.0, fs);
        let w = 2.0 * core::f32::consts::PI * 1_000.0 / fs;
        let mut last = 0.0;
        for n in 0..9_600 {
            let x = (w * (n as f32)).sin();
            last = r.process_sample(x);
        }
        let expected = core::f32::consts::FRAC_1_SQRT_2;
        assert!((last - expected).abs() < 0.02, "last={last}, expected={expected}");
    }
}
