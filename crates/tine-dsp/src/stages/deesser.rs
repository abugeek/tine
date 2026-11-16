//! Stage 4: Split-band de-esser.

use tine_protocol::params::{DspParams, FilterType};

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::biquad::{Biquad, Coeffs, LinkwitzRiley4};
use crate::primitives::db::{db_to_lin, lin_to_db};
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;

const COEFF_UPDATE_CHUNK: usize = 32;
const RETUNE_EPS: f32 = 1e-4;
const SMOOTH_TAU_MS: f32 = 10.0;
const SIDECHAIN_Q: f32 = 1.4;
const SC_ATTACK_MS: f32 = 0.5;
const SC_RELEASE_MS: f32 = 40.0;

#[inline]
fn sanitize_finite(val: f32, min: f32, max: f32, fallback: f32) -> f32 {
    if val.is_finite() { val.clamp(min, max) } else { fallback }
}

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

/// Fast division-free inverse square root (`x^(-1/2)`) for `x > 0`, accurate to ~0.17% (~0.015 dB).
#[inline]
fn fast_inv_sqrt(x: f32) -> f32 {
    let half_x = 0.5 * x;
    let mut y = f32::from_bits(0x5f37_5a86_u32.wrapping_sub(x.to_bits() >> 1));
    y *= 1.5 - half_x * y * y;
    y
}

/// Division-free `u^(-3/8)` for `u = mean_sq / thresh_sq > 1.0`.
#[inline]
fn ratio4_gain_from_sq_ratio(u: f32, min_gain_lin: f32) -> f32 {
    let s1 = fast_inv_sqrt(u); // u^(-1/2)
    let s2 = fast_inv_sqrt(s1); // u^(+1/4)
    let s3 = fast_inv_sqrt(s1 * s2); // u^(+1/8)
    (s1 * s3).max(min_gain_lin)
}

/// Stage 4: Split-band de-esser with LR4 crossover and band-pass RMS sidechain.
#[derive(Debug)]
pub(crate) struct DeesserStage {
    fs: f64,
    first_process: bool,
    lr4: LinkwitzRiley4,
    sidechain_bp: Biquad,
    mean_sq: f32,
    att_coeff: f32,
    rel_coeff: f32,
    freq: Smoother,
    thresh_db: Smoother,
    amount_db: Smoother,
    enable: Smoother,
    cached_thresh_db: f32,
    cached_thresh_sq: f32,
    cached_inv_thresh_sq: f32,
    cached_amount_db: f32,
    cached_min_gain_lin: f32,
    bypassed: bool,
}

impl Default for DeesserStage {
    fn default() -> Self {
        Self::new()
    }
}

impl DeesserStage {
    pub fn new() -> Self {
        let d = DspParams::DEFAULT;
        let fs = 48_000.0;
        let fs_f64 = f64::from(fs);
        let freq_f64 = f64::from(d.deesser_freq_hz);
        let t_lin = db_to_lin(d.deesser_thresh_db);
        let t_sq = t_lin * t_lin;
        Self {
            fs: fs_f64,
            first_process: true,
            lr4: LinkwitzRiley4::new(fs_f64, freq_f64),
            sidechain_bp: Biquad::new(Coeffs::design(
                FilterType::BandPass,
                fs_f64,
                freq_f64,
                f64::from(SIDECHAIN_Q),
                0.0,
            )),
            mean_sq: 0.0,
            att_coeff: time_coeff(SC_ATTACK_MS, fs),
            rel_coeff: time_coeff(SC_RELEASE_MS, fs),
            freq: Smoother::new(SMOOTH_TAU_MS, fs, d.deesser_freq_hz),
            thresh_db: Smoother::new(SMOOTH_TAU_MS, fs, d.deesser_thresh_db),
            amount_db: Smoother::new(SMOOTH_TAU_MS, fs, d.deesser_amount_db),
            enable: Smoother::new(SMOOTH_TAU_MS, fs, if d.deesser_enabled { 1.0 } else { 0.0 }),
            cached_thresh_db: d.deesser_thresh_db,
            cached_thresh_sq: t_sq,
            cached_inv_thresh_sq: if t_sq > 0.0 { 1.0 / t_sq } else { 0.0 },
            cached_amount_db: d.deesser_amount_db,
            cached_min_gain_lin: db_to_lin(-d.deesser_amount_db),
            bypassed: !d.deesser_enabled,
        }
    }

    fn reset_filters(&mut self) {
        self.lr4.reset();
        self.sidechain_bp.reset();
        self.mean_sq = 0.0;
    }

    fn retune(&mut self) {
        let f = self.freq.current();
        self.lr4.retune_if_changed(self.fs, f, RETUNE_EPS);
        self.sidechain_bp
            .retune_if_changed(FilterType::BandPass, self.fs, f, SIDECHAIN_Q, 0.0, RETUNE_EPS);
    }

    fn update_thresh_cache(&mut self, t: f32) {
        self.cached_thresh_db = t;
        let t_lin = db_to_lin(t);
        let t_sq = t_lin * t_lin;
        self.cached_thresh_sq = t_sq;
        self.cached_inv_thresh_sq = if t_sq > 0.0 { 1.0 / t_sq } else { 0.0 };
    }

    #[inline]
    fn current_thresh_sq(&mut self) -> (f32, f32) {
        let t = self.thresh_db.current();
        if (t - self.cached_thresh_db).abs() > 1e-5 {
            self.update_thresh_cache(t);
        }
        (self.cached_thresh_sq, self.cached_inv_thresh_sq)
    }

    #[inline]
    fn current_min_gain_lin(&mut self) -> f32 {
        let a = self.amount_db.current();
        if (a - self.cached_amount_db).abs() > 1e-5 {
            self.cached_amount_db = a;
            self.cached_min_gain_lin = db_to_lin(-a);
        }
        self.cached_min_gain_lin
    }
}

impl Stage for DeesserStage {
    fn prepare(&mut self, fs: f32, _max_block: usize) {
        let fs_val = if fs.is_finite() && fs > 0.0 { fs } else { 48_000.0 };
        self.fs = f64::from(fs_val);
        self.first_process = true;
        self.att_coeff = time_coeff(SC_ATTACK_MS, fs_val);
        self.rel_coeff = time_coeff(SC_RELEASE_MS, fs_val);
        self.freq.set_time_constant(SMOOTH_TAU_MS, fs_val);
        self.thresh_db.set_time_constant(SMOOTH_TAU_MS, fs_val);
        self.amount_db.set_time_constant(SMOOTH_TAU_MS, fs_val);
        self.enable.set_time_constant(SMOOTH_TAU_MS, fs_val);

        let d = DspParams::DEFAULT;
        self.freq.snap_to(d.deesser_freq_hz);
        self.thresh_db.snap_to(d.deesser_thresh_db);
        self.amount_db.snap_to(d.deesser_amount_db);
        self.enable.snap_to(if d.deesser_enabled { 1.0 } else { 0.0 });
        self.update_thresh_cache(d.deesser_thresh_db);
        self.cached_amount_db = d.deesser_amount_db;
        self.cached_min_gain_lin = db_to_lin(-d.deesser_amount_db);
        self.bypassed = !d.deesser_enabled;

        self.reset_filters();
        self.retune();
    }

    fn reset(&mut self) {
        self.reset_filters();
        self.freq.snap();
        self.thresh_db.snap();
        self.amount_db.snap();
        self.enable.snap();
        let t = self.thresh_db.current();
        self.update_thresh_cache(t);
        let a = self.amount_db.current();
        self.cached_amount_db = a;
        self.cached_min_gain_lin = db_to_lin(-a);
        self.retune();
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters) {
        if buf.is_empty() {
            m.deesser_reduction_db = 0.0;
            return;
        }

        let freq_t = sanitize_finite(p.deesser_freq_hz, 5_500.0, 9_500.0, 6_800.0);
        let thresh_t = sanitize_finite(p.deesser_thresh_db, -60.0, 0.0, -30.0);
        let amount_t = sanitize_finite(p.deesser_amount_db, 0.0, 12.0, 6.0);
        let enable_t = if p.deesser_enabled { 1.0 } else { 0.0 };

        if self.first_process {
            self.first_process = false;
            self.freq.snap_to(freq_t);
            self.thresh_db.snap_to(thresh_t);
            self.amount_db.snap_to(amount_t);
            self.enable.snap_to(enable_t);
            self.update_thresh_cache(thresh_t);
            self.cached_amount_db = amount_t;
            self.cached_min_gain_lin = db_to_lin(-amount_t);
            self.bypassed = !p.deesser_enabled;
            self.retune();
        } else {
            self.freq.set_target(freq_t);
            self.thresh_db.set_target(thresh_t);
            self.amount_db.set_target(amount_t);
            self.enable.set_target(enable_t);
        }

        // Fast bypass path when disabled and cross-fade has settled.
        if self.enable.is_settled() && self.enable.current() < 0.001 {
            if !self.bypassed {
                self.reset_filters();
                self.bypassed = true;
            }
            if !self.freq.is_settled() {
                self.freq.skip(buf.len());
            }
            if !self.thresh_db.is_settled() {
                self.thresh_db.skip(buf.len());
            }
            if !self.amount_db.is_settled() {
                self.amount_db.skip(buf.len());
            }
            m.deesser_reduction_db = 0.0;
            return;
        }
        self.bypassed = false;

        let mut min_gain = 1.0f32;
        let enable_settled_on = self.enable.is_settled() && self.enable.current() >= 0.999;
        let att = self.att_coeff;
        let rel = self.rel_coeff;

        // Fast path: all smoothers settled and de-esser enabled.
        if enable_settled_on && self.freq.is_settled() && self.thresh_db.is_settled() && self.amount_db.is_settled() {
            self.retune();
            let (thresh_sq, inv_thresh_sq) = self.current_thresh_sq();
            let min_gain_lin = self.current_min_gain_lin();
            let mut lr4 = self.lr4;
            let mut sc_bp = self.sidechain_bp;
            let mut mean_sq = self.mean_sq;

            for sample in buf.iter_mut() {
                let x = *sample;
                let (lo, hi) = lr4.process_sample(x);
                let sc = sc_bp.process_sample(x);
                let sq = sc * sc;
                let coeff = if sq > mean_sq { att } else { rel };
                mean_sq = sq + coeff * (mean_sq - sq);

                let gain = if mean_sq > thresh_sq {
                    ratio4_gain_from_sq_ratio(mean_sq * inv_thresh_sq, min_gain_lin)
                } else {
                    1.0
                };
                if gain < min_gain {
                    min_gain = gain;
                }
                *sample = lo + hi * gain;
            }

            self.lr4 = lr4;
            self.sidechain_bp = sc_bp;
            self.mean_sq = mean_sq;
            m.deesser_reduction_db = if min_gain < 0.999_99 { -lin_to_db(min_gain) } else { 0.0 };
            return;
        }

        for chunk in buf.chunks_mut(COEFF_UPDATE_CHUNK) {
            let n = chunk.len();
            if !self.freq.is_settled() {
                self.freq.skip(n);
            }
            if !self.thresh_db.is_settled() {
                self.thresh_db.skip(n);
            }
            if !self.amount_db.is_settled() {
                self.amount_db.skip(n);
            }

            self.retune();
            let (thresh_sq, inv_thresh_sq) = self.current_thresh_sq();
            let min_gain_lin = self.current_min_gain_lin();

            if enable_settled_on {
                for sample in chunk.iter_mut() {
                    let x = *sample;
                    let (lo, hi) = self.lr4.process_sample(x);
                    let sc = self.sidechain_bp.process_sample(x);
                    let sq = sc * sc;
                    let coeff = if sq > self.mean_sq { att } else { rel };
                    self.mean_sq = sq + coeff * (self.mean_sq - sq);

                    let gain = if self.mean_sq > thresh_sq {
                        ratio4_gain_from_sq_ratio(self.mean_sq * inv_thresh_sq, min_gain_lin)
                    } else {
                        1.0
                    };
                    if gain < min_gain {
                        min_gain = gain;
                    }
                    *sample = lo + hi * gain;
                }
            } else {
                for sample in chunk.iter_mut() {
                    let x = *sample;
                    let (lo, hi) = self.lr4.process_sample(x);
                    let sc = self.sidechain_bp.process_sample(x);
                    let sq = sc * sc;
                    let coeff = if sq > self.mean_sq { att } else { rel };
                    self.mean_sq = sq + coeff * (self.mean_sq - sq);

                    let gain = if self.mean_sq > thresh_sq {
                        ratio4_gain_from_sq_ratio(self.mean_sq * inv_thresh_sq, min_gain_lin)
                    } else {
                        1.0
                    };
                    let wet = lo + hi * gain;
                    let mix = self.enable.next().clamp(0.0, 1.0);
                    *sample = x + (wet - x) * mix;

                    let eff_gain = 1.0 + (gain - 1.0) * mix;
                    if eff_gain < min_gain {
                        min_gain = eff_gain;
                    }
                }
            }
        }

        m.deesser_reduction_db = if min_gain < 0.999_99 { -lin_to_db(min_gain) } else { 0.0 };
    }

    fn latency(&self) -> u32 {
        0
    }
}
