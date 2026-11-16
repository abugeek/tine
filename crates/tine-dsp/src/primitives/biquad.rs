//! In-house RBJ Audio-EQ-Cookbook biquad filters (D-010).
//!
//! Coefficients are designed in `f64` and executed as Transposed Direct Form II
//! in `f32`.

use core::f64::consts::{FRAC_1_SQRT_2, PI};

use tine_protocol::params::FilterType;

use super::math::{log10_f64, pow10_f64, sin_cos_f64, sqrt_f64};

/// Normalized biquad filter coefficients (`a0 = 1`).
///
/// Difference equation:
/// `y[n] = b0*x[n] + b1*x[n-1] + b2*x[n-2] - a1*y[n-1] - a2*y[n-2]`
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Coeffs {
    /// Feedforward coefficient `b0 / a0`.
    pub b0: f32,
    /// Feedforward coefficient `b1 / a0`.
    pub b1: f32,
    /// Feedforward coefficient `b2 / a0`.
    pub b2: f32,
    /// Feedback coefficient `a1 / a0`.
    pub a1: f32,
    /// Feedback coefficient `a2 / a0`.
    pub a2: f32,
}

impl Default for Coeffs {
    fn default() -> Self {
        Self::IDENTITY
    }
}

impl Coeffs {
    /// Unity pass-through coefficients.
    pub const IDENTITY: Self = Self {
        b0: 1.0,
        b1: 0.0,
        b2: 0.0,
        a1: 0.0,
        a2: 0.0,
    };

    /// Designs normalized RBJ Audio-EQ-Cookbook coefficients in `f64`.
    #[must_use]
    pub fn design(filter_type: FilterType, fs: f64, freq_hz: f64, q: f64, gain_db: f64) -> Self {
        if !fs.is_finite() || fs <= 0.0 {
            return Self::IDENTITY;
        }
        let f_clamped = freq_hz.clamp(10.0, fs * 0.495);
        let q_clamped = q.clamp(0.05, 20.0);
        let gain_clamped = gain_db.clamp(-36.0, 36.0);

        let w0 = 2.0 * PI * f_clamped / fs;
        let (sin_w0, cos_w0) = sin_cos_f64(w0);
        let alpha = sin_w0 / (2.0 * q_clamped);
        let a = pow10_f64(gain_clamped / 40.0);

        let (b0, b1, b2, a0, a1, a2) = match filter_type {
            FilterType::LowPass => {
                let one_minus_cos = 1.0 - cos_w0;
                (
                    one_minus_cos * 0.5,
                    one_minus_cos,
                    one_minus_cos * 0.5,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            FilterType::HighPass => {
                let one_plus_cos = 1.0 + cos_w0;
                (
                    one_plus_cos * 0.5,
                    -one_plus_cos,
                    one_plus_cos * 0.5,
                    1.0 + alpha,
                    -2.0 * cos_w0,
                    1.0 - alpha,
                )
            }
            FilterType::BandPass => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            FilterType::Notch => (1.0, -2.0 * cos_w0, 1.0, 1.0 + alpha, -2.0 * cos_w0, 1.0 - alpha),
            FilterType::Peaking => (
                1.0 + alpha * a,
                -2.0 * cos_w0,
                1.0 - alpha * a,
                1.0 + alpha / a,
                -2.0 * cos_w0,
                1.0 - alpha / a,
            ),
            FilterType::LowShelf => {
                let beta = 2.0 * sqrt_f64(a) * alpha;
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                (
                    a * (ap1 - am1 * cos_w0 + beta),
                    2.0 * a * (am1 - ap1 * cos_w0),
                    a * (ap1 - am1 * cos_w0 - beta),
                    ap1 + am1 * cos_w0 + beta,
                    -2.0 * (am1 + ap1 * cos_w0),
                    ap1 + am1 * cos_w0 - beta,
                )
            }
            FilterType::HighShelf => {
                let beta = 2.0 * sqrt_f64(a) * alpha;
                let ap1 = a + 1.0;
                let am1 = a - 1.0;
                (
                    a * (ap1 + am1 * cos_w0 + beta),
                    -2.0 * a * (am1 + ap1 * cos_w0),
                    a * (ap1 + am1 * cos_w0 - beta),
                    ap1 - am1 * cos_w0 + beta,
                    2.0 * (am1 - ap1 * cos_w0),
                    ap1 - am1 * cos_w0 - beta,
                )
            }
        };

        let inv_a0 = 1.0 / a0;
        Self {
            b0: (b0 * inv_a0) as f32,
            b1: (b1 * inv_a0) as f32,
            b2: (b2 * inv_a0) as f32,
            a1: (a1 * inv_a0) as f32,
            a2: (a2 * inv_a0) as f32,
        }
    }

    /// Evaluates the analytic magnitude response in decibels at frequency `freq_hz`.
    #[must_use]
    pub fn magnitude_db(&self, freq_hz: f64, fs: f64) -> f64 {
        if fs <= 0.0 || freq_hz <= 0.0 {
            return 0.0;
        }
        let w = 2.0 * PI * freq_hz / fs;
        let (sin_w, cos_w) = sin_cos_f64(w);
        let (sin_2w, cos_2w) = sin_cos_f64(2.0 * w);

        let b0 = f64::from(self.b0);
        let b1 = f64::from(self.b1);
        let b2 = f64::from(self.b2);
        let a1 = f64::from(self.a1);
        let a2 = f64::from(self.a2);

        let num_re = b0 + b1 * cos_w + b2 * cos_2w;
        let num_im = -(b1 * sin_w + b2 * sin_2w);
        let den_re = 1.0 + a1 * cos_w + a2 * cos_2w;
        let den_im = -(a1 * sin_w + a2 * sin_2w);

        let num_sq = num_re * num_re + num_im * num_im;
        let den_sq = den_re * den_re + den_im * den_im;
        if den_sq <= 1e-24 || num_sq <= 1e-24 {
            return -120.0;
        }
        (10.0 * log10_f64(num_sq / den_sq)).max(-120.0)
    }
}

/// Transposed Direct Form II biquad filter.
#[derive(Clone, Copy, Debug)]
pub struct Biquad {
    coeffs: Coeffs,
    z1: f32,
    z2: f32,
    last_type: FilterType,
    last_fs: f64,
    last_freq: f32,
    last_q: f32,
    last_gain_db: f32,
}

impl Default for Biquad {
    fn default() -> Self {
        Self::new(Coeffs::IDENTITY)
    }
}

impl Biquad {
    /// Creates a new [`Biquad`] with the given coefficients.
    #[must_use]
    pub const fn new(coeffs: Coeffs) -> Self {
        Self {
            coeffs,
            z1: 0.0,
            z2: 0.0,
            last_type: FilterType::Peaking,
            last_fs: 0.0,
            last_freq: 0.0,
            last_q: 0.0,
            last_gain_db: 0.0,
        }
    }

    /// Returns the active filter coefficients.
    #[inline]
    #[must_use]
    pub const fn coeffs(&self) -> Coeffs {
        self.coeffs
    }

    /// Sets new coefficients directly without resetting filter state.
    #[inline]
    pub fn set_coeffs(&mut self, coeffs: Coeffs) {
        self.coeffs = coeffs;
    }

    /// Clears the internal delay state.
    #[inline]
    pub fn reset(&mut self) {
        self.z1 = 0.0;
        self.z2 = 0.0;
    }

    /// Recomputes coefficients only if any parameter moved by more than `eps`.
    ///
    /// Returns `true` if coefficients were recomputed.
    pub fn retune_if_changed(
        &mut self,
        filter_type: FilterType,
        fs: f64,
        freq_hz: f32,
        q: f32,
        gain_db: f32,
        eps: f32,
    ) -> bool {
        let changed = self.last_fs != fs
            || self.last_type != filter_type
            || (self.last_freq - freq_hz).abs() > eps * self.last_freq.max(1.0)
            || (self.last_q - q).abs() > eps
            || (self.last_gain_db - gain_db).abs() > eps;
        if changed {
            self.coeffs = Coeffs::design(filter_type, fs, f64::from(freq_hz), f64::from(q), f64::from(gain_db));
            self.last_type = filter_type;
            self.last_fs = fs;
            self.last_freq = freq_hz;
            self.last_q = q;
            self.last_gain_db = gain_db;
        }
        changed
    }

    /// Processes a single sample through the TDF-II filter.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> f32 {
        let c = self.coeffs;
        let y = c.b0 * x + self.z1;
        self.z1 = c.b1 * x - c.a1 * y + self.z2;
        self.z2 = c.b2 * x - c.a2 * y;
        y
    }

    /// Processes a block of samples in-place, keeping state in registers.
    pub fn process_block(&mut self, buf: &mut [f32]) {
        let c = self.coeffs;
        let mut z1 = self.z1;
        let mut z2 = self.z2;
        for sample in buf.iter_mut() {
            let x = *sample;
            let y = c.b0 * x + z1;
            z1 = c.b1 * x - c.a1 * y + z2;
            z2 = c.b2 * x - c.a2 * y;
            *sample = y;
        }
        self.z1 = z1;
        self.z2 = z2;
    }
}

/// 4th-order Linkwitz–Riley crossover (two cascaded 2nd-order Butterworth stages).
#[derive(Clone, Copy, Debug)]
pub struct LinkwitzRiley4 {
    lp1: Biquad,
    lp2: Biquad,
    hp1: Biquad,
    hp2: Biquad,
}

impl Default for LinkwitzRiley4 {
    fn default() -> Self {
        Self::new(48_000.0, 6_800.0)
    }
}

impl LinkwitzRiley4 {
    /// Creates a new 4th-order Linkwitz–Riley crossover at `freq_hz`.
    #[must_use]
    pub fn new(fs: f64, freq_hz: f64) -> Self {
        let lp_c = Coeffs::design(FilterType::LowPass, fs, freq_hz, FRAC_1_SQRT_2, 0.0);
        let hp_c = Coeffs::design(FilterType::HighPass, fs, freq_hz, FRAC_1_SQRT_2, 0.0);
        Self {
            lp1: Biquad::new(lp_c),
            lp2: Biquad::new(lp_c),
            hp1: Biquad::new(hp_c),
            hp2: Biquad::new(hp_c),
        }
    }

    /// Updates the crossover frequency if it changed by more than `eps`.
    pub fn retune_if_changed(&mut self, fs: f64, freq_hz: f32, eps: f32) -> bool {
        let q = FRAC_1_SQRT_2 as f32;
        let changed = self
            .lp1
            .retune_if_changed(FilterType::LowPass, fs, freq_hz, q, 0.0, eps);
        if changed {
            let lp_c = self.lp1.coeffs();
            self.lp2.set_coeffs(lp_c);
            let hp_c = Coeffs::design(FilterType::HighPass, fs, f64::from(freq_hz), FRAC_1_SQRT_2, 0.0);
            self.hp1.set_coeffs(hp_c);
            self.hp2.set_coeffs(hp_c);
        }
        changed
    }

    /// Clears all internal filter states.
    pub fn reset(&mut self) {
        self.lp1.reset();
        self.lp2.reset();
        self.hp1.reset();
        self.hp2.reset();
    }

    /// Splits a single sample into `(low_band, high_band)`.
    #[inline]
    pub fn process_sample(&mut self, x: f32) -> (f32, f32) {
        let lo = self.lp2.process_sample(self.lp1.process_sample(x));
        let hi = self.hp2.process_sample(self.hp1.process_sample(x));
        (lo, hi)
    }

    /// Splits `input` into `low_out` and `high_out` slices of the same length.
    pub fn process_block(&mut self, input: &[f32], low_out: &mut [f32], high_out: &mut [f32]) {
        for ((&x, lo), hi) in input.iter().zip(low_out.iter_mut()).zip(high_out.iter_mut()) {
            let (l, h) = self.process_sample(x);
            *lo = l;
            *hi = h;
        }
    }
}

#[cfg(test)]
mod tests {
    use core::f64::consts::PI;

    use super::*;

    fn measure_sine_gain_db(biquad: &mut Biquad, freq_hz: f64, fs: f64) -> f64 {
        biquad.reset();
        let settle = (fs * 0.5).max(4096.0) as usize;
        // Measure over an integer number of cycles (>= 0.25 s) to avoid windowing error.
        let cycles = (freq_hz * 0.25).ceil().max(1.0);
        let measure = ((cycles * fs) / freq_hz).round() as usize;
        let total_samples = settle + measure;
        let w = 2.0 * PI * freq_hz / fs;
        let mut in_sq = 0.0f64;
        let mut out_sq = 0.0f64;
        for n in 0..total_samples {
            let x = (w * (n as f64)).sin() as f32;
            let y = biquad.process_sample(x);
            if n >= settle {
                in_sq += f64::from(x) * f64::from(x);
                out_sq += f64::from(y) * f64::from(y);
            }
        }
        if out_sq <= 1e-24 {
            return -120.0;
        }
        10.0 * (out_sq / in_sq).log10()
    }

    #[test]
    fn all_filter_types_match_analytic_magnitude() {
        let sample_rates = [44_100.0, 48_000.0, 96_000.0];
        let filter_types = [
            FilterType::LowShelf,
            FilterType::HighShelf,
            FilterType::Peaking,
            FilterType::Notch,
            FilterType::LowPass,
            FilterType::HighPass,
            FilterType::BandPass,
        ];

        // 20 log-spaced frequencies from 40 Hz to 16 kHz
        let mut freqs = [0.0f64; 20];
        let f_min = 40.0f64.ln();
        let f_max = 16_000.0f64.ln();
        for (i, slot) in freqs.iter_mut().enumerate() {
            let t = (i as f64) / 19.0;
            *slot = (f_min + t * (f_max - f_min)).exp();
        }

        for &fs in &sample_rates {
            for &ft in &filter_types {
                let coeffs = Coeffs::design(ft, fs, 1_200.0, 1.2, 6.0);
                let mut biquad = Biquad::new(coeffs);
                for &f in &freqs {
                    let expected = coeffs.magnitude_db(f, fs);
                    if expected <= -50.0 {
                        continue; // skip deep stopband/notch null where finite-window DFT/sine leakage dominates
                    }
                    let measured = measure_sine_gain_db(&mut biquad, f, fs);
                    assert!(
                        (measured - expected).abs() < 0.05,
                        "fs={fs} ft={ft:?} f={f:.1}: measured={measured:.4} expected={expected:.4}"
                    );
                }
            }
        }
    }

    #[test]
    fn linkwitz_riley_crossover_sums_flat() {
        let fs = 48_000.0;
        let mut lr = LinkwitzRiley4::new(fs, 2_000.0);
        let mut max_err_db = 0.0f64;
        for &f in &[100.0, 500.0, 1_000.0, 2_000.0, 4_000.0, 10_000.0] {
            lr.reset();
            let n_tot = 24_000usize;
            let settle = 12_000usize;
            let w = 2.0 * PI * f / fs;
            let mut in_sq = 0.0;
            let mut sum_sq = 0.0;
            for n in 0..n_tot {
                let x = (w * (n as f64)).sin() as f32;
                let (lo, hi) = lr.process_sample(x);
                let s = lo + hi;
                if n >= settle {
                    in_sq += f64::from(x) * f64::from(x);
                    sum_sq += f64::from(s) * f64::from(s);
                }
            }
            let err_db = (10.0 * (sum_sq / in_sq).log10()).abs();
            max_err_db = max_err_db.max(err_db);
        }
        assert!(max_err_db < 0.05, "LR4 sum error = {max_err_db} dB");
    }
}
