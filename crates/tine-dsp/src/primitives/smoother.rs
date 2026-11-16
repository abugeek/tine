//! One-pole parameter smoother for zipper-free control transitions.

use super::math::{exp_f64, ln_f64};

/// One-pole exponential smoother (`y[n] = target + (y[n-1] - target) * coeff`).
#[derive(Clone, Copy, Debug)]
pub struct Smoother {
    current: f32,
    target: f32,
    coeff: f64,
}

impl Smoother {
    /// Creates a new smoother with time constant `tau_ms` at sample rate `fs`.
    #[must_use]
    pub fn new(tau_ms: f32, fs: f32, initial: f32) -> Self {
        let mut s = Self {
            current: initial,
            target: initial,
            coeff: 0.0,
        };
        s.set_time_constant(tau_ms, fs);
        s
    }

    /// Updates the time constant `tau_ms` and sample rate `fs`.
    pub fn set_time_constant(&mut self, tau_ms: f32, fs: f32) {
        let tau_s = f64::from(tau_ms) * 1e-3;
        let fs_f64 = f64::from(fs);
        if tau_s <= 0.0 || fs_f64 <= 0.0 || !tau_s.is_finite() || !fs_f64.is_finite() {
            self.coeff = 0.0;
        } else {
            self.coeff = exp_f64(-1.0 / (tau_s * fs_f64));
        }
    }

    /// Sets the target value to smooth toward.
    #[inline]
    pub fn set_target(&mut self, target: f32) {
        self.target = target;
    }

    /// Returns the current target value.
    #[inline]
    #[must_use]
    pub const fn target(&self) -> f32 {
        self.target
    }

    /// Returns the current smoothed value without advancing.
    #[inline]
    #[must_use]
    pub const fn current(&self) -> f32 {
        self.current
    }

    /// Advances the smoother by one sample and returns the new value.
    #[inline]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let alpha = (1.0 - self.coeff) as f32;
        self.current += (self.target - self.current) * alpha;
        self.current
    }

    /// Advances the smoother by `n` samples in closed form.
    pub fn skip(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if self.coeff <= 0.0 {
            self.current = self.target;
            return;
        }
        let decay = exp_f64((n as f64) * ln_f64(self.coeff));
        let next = f64::from(self.target) + (f64::from(self.current) - f64::from(self.target)) * decay;
        self.current = next as f32;
    }

    /// Returns `true` when the smoother has converged within `1e-5` of its target.
    #[inline]
    #[must_use]
    pub fn is_settled(&self) -> bool {
        (self.target - self.current).abs() <= 1e-5
    }

    /// Immediately snaps the current value to the target.
    #[inline]
    pub fn snap(&mut self) {
        self.current = self.target;
    }

    /// Immediately sets both current and target values to `val`.
    #[inline]
    pub fn snap_to(&mut self, val: f32) {
        self.current = val;
        self.target = val;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn step_reaches_63_percent_in_tau() {
        for &fs in &[44_100.0f32, 48_000.0, 96_000.0] {
            let tau_ms = 10.0f32;
            let mut s = Smoother::new(tau_ms, fs, 0.0);
            s.set_target(1.0);
            let n_tau = (tau_ms * 1e-3 * fs).round() as usize;
            for _ in 0..n_tau {
                s.next();
            }
            let val = s.current();
            let expected = 1.0 - (-1.0f32).exp(); // ~0.6321
            assert!(
                (val - expected).abs() <= 0.02,
                "fs={fs}: val={val}, expected={expected}"
            );
        }
    }

    #[test]
    fn skip_matches_sample_by_sample() {
        let mut a = Smoother::new(10.0, 48_000.0, -5.0);
        let mut b = Smoother::new(10.0, 48_000.0, -5.0);
        a.set_target(12.5);
        b.set_target(12.5);

        for _ in 0..333 {
            a.next();
        }
        b.skip(333);
        assert!((a.current() - b.current()).abs() < 1e-4);

        b.skip(48_000);
        assert!(b.is_settled());
    }
}
