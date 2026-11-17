//! Stage 5: Program-dependent vocal compressor.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::db::{DB_FLOOR, LIN_FLOOR};
use crate::primitives::envelope::{PeakEnvelope, RmsEnvelope};
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;

/// `10^(-6/20)` for `peak - 6 dB` comparison in linear domain.
const MINUS_6_DB_LIN: f32 = 0.501_187_2;

/// Fast `20 * log10(abs)` in `f32` for positive finite `abs` (no floor check).
#[inline]
fn fast_lin_to_db_raw(abs: f32) -> f32 {
    let bits = abs.to_bits();
    let exp = (bits >> 23).cast_signed() - 127;
    let m = f32::from_bits((bits & 0x007f_ffff) | 0x3f80_0000);
    let u = (m - 1.0) / (m + 1.0);
    let u2 = u * u;
    let ln_m = 2.0 * u * (1.0 + u2 * (1.0 / 3.0 + u2 * (1.0 / 5.0 + u2 * (1.0 / 7.0))));
    // 20 / ln(10) = 8.68589, 20 * log10(2) = 6.0206
    (exp as f32) * 6.020_6 + ln_m * 8.685_89
}

/// Fast `10 * log10(sq)` in `f32` for squared linear amplitude, floored at [`DB_FLOOR`].
#[inline]
fn fast_sq_to_db(sq: f32) -> f32 {
    const LIN_FLOOR_SQ: f32 = LIN_FLOOR * LIN_FLOOR;
    if !sq.is_finite() || sq <= LIN_FLOOR_SQ {
        return DB_FLOOR;
    }
    0.5 * fast_lin_to_db_raw(sq)
}

/// Fast `lin_to_db` in `f32` (max error < 0.001 dB over `[-120, +48] dB`).
#[cfg(test)]
#[inline]
fn fast_lin_to_db(lin: f32) -> f32 {
    let abs = lin.abs();
    if !abs.is_finite() || abs <= LIN_FLOOR {
        return DB_FLOOR;
    }
    fast_lin_to_db_raw(abs)
}

/// Fast `db_to_lin` in `f32` (max error < 0.001 dB over `[-120, +48] dB`).
#[inline]
fn fast_db_to_lin(db: f32) -> f32 {
    if !db.is_finite() || db < DB_FLOOR {
        return 0.0;
    }
    let db = db.min(48.0);
    // x = db * ln(10) / 20
    let x = db * 0.115_129_255;
    let k_f = x * core::f32::consts::LOG2_E;
    let k = (k_f + if k_f >= 0.0 { 0.5 } else { -0.5 }) as i32;
    let r = x - (k as f32) * core::f32::consts::LN_2;
    let poly = 1.0 + r * (1.0 + r * (0.5 + r * (1.0 / 6.0 + r * (1.0 / 24.0 + r * (1.0 / 120.0)))));
    let scale = f32::from_bits(((k + 127) as u32) << 23);
    poly * scale
}

/// RMS detector window in milliseconds.
const RMS_WINDOW_MS: f32 = 10.0;

/// Peak transient detector release time in milliseconds.
const PEAK_RELEASE_MS: f32 = 10.0;

/// Over-threshold duration threshold for program-dependent release (ms).
const PROGRAM_RELEASE_WINDOW_MS: f32 = 50.0;

/// Gain reduction level below which the program-dependent timer resets (dB).
const RELEASE_RESET_GR_DB: f32 = 0.05;

/// Computes the one-pole time constant coefficient `exp(-1 / (ms * 1e-3 * fs))`.
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

/// Stage 5: Program-dependent vocal compressor with log-domain detection,
/// quadratic soft knee, and optional auto-makeup gain.
#[derive(Debug)]
pub struct CompressorStage {
    fs: f32,
    rms_env: RmsEnvelope,
    peak_env: PeakEnvelope,
    makeup_smoother: Smoother,
    gr_db: f32,
    over_thresh_samples: usize,
    program_window_samples: usize,
    last_attack_ms: f32,
    attack_coeff: f32,
    last_release_ms: f32,
    base_release_coeff: f32,
}

impl Default for CompressorStage {
    fn default() -> Self {
        Self::new()
    }
}

impl CompressorStage {
    /// Creates a new [`CompressorStage`] with default 48 kHz state.
    #[must_use]
    pub fn new() -> Self {
        let mut stage = Self {
            fs: 48_000.0,
            rms_env: RmsEnvelope::new(RMS_WINDOW_MS, 48_000.0),
            peak_env: PeakEnvelope::new(0.0, PEAK_RELEASE_MS, 48_000.0),
            makeup_smoother: Smoother::new(10.0, 48_000.0, 0.0),
            gr_db: 0.0,
            over_thresh_samples: 0,
            program_window_samples: 2_400,
            last_attack_ms: -1.0,
            attack_coeff: 0.0,
            last_release_ms: -1.0,
            base_release_coeff: 0.0,
        };
        stage.prepare(48_000.0, 512);
        stage
    }

    /// Evaluates the static compression transfer curve (pre-makeup) in dBFS.
    ///
    /// Uses a quadratic soft knee of width `p.comp_knee_db` centered at `p.comp_thresh_db`.
    #[inline]
    #[must_use]
    pub fn static_curve_db(p: &DspParams, input_db: f32) -> f32 {
        if !input_db.is_finite() {
            return DB_FLOOR;
        }
        let thresh = if p.comp_thresh_db.is_finite() {
            p.comp_thresh_db
        } else {
            -20.0
        };
        let ratio = if p.comp_ratio.is_finite() {
            p.comp_ratio.max(1.0)
        } else {
            1.0
        };
        let knee = if p.comp_knee_db.is_finite() {
            p.comp_knee_db.max(0.0)
        } else {
            0.0
        };

        if ratio <= 1.0 {
            return input_db;
        }

        let inv_ratio_minus_one = (1.0 / ratio) - 1.0;

        if knee <= 1e-6 {
            if input_db <= thresh {
                input_db
            } else {
                thresh + (input_db - thresh) / ratio
            }
        } else {
            let over = input_db - thresh;
            let half_knee = 0.5 * knee;
            if over <= -half_knee {
                input_db
            } else if over >= half_knee {
                thresh + over / ratio
            } else {
                let x = over + half_knee;
                input_db + inv_ratio_minus_one * (x * x) / (2.0 * knee)
            }
        }
    }

    /// Computes the automatic makeup gain in dB: `-(T - T/R) * 0.5`
    /// (half the static gain reduction at 0 dBFS).
    #[inline]
    #[must_use]
    pub fn auto_makeup_db(p: &DspParams) -> f32 {
        let thresh = if p.comp_thresh_db.is_finite() {
            p.comp_thresh_db.min(0.0)
        } else {
            -20.0
        };
        let ratio = if p.comp_ratio.is_finite() {
            p.comp_ratio.max(1.0)
        } else {
            1.0
        };
        -(thresh - thresh / ratio) * 0.5
    }
}

impl Stage for CompressorStage {
    fn prepare(&mut self, fs: f32, _max_block: usize) {
        let fs = if fs.is_finite() && fs > 0.0 { fs } else { 48_000.0 };
        self.fs = fs;
        self.rms_env.set_times(RMS_WINDOW_MS, RMS_WINDOW_MS, fs);
        self.peak_env.set_times(0.0, PEAK_RELEASE_MS, fs);
        self.makeup_smoother.set_time_constant(10.0, fs);
        self.program_window_samples = ((PROGRAM_RELEASE_WINDOW_MS * 1e-3 * fs) + 0.5) as usize;
        self.last_attack_ms = -1.0;
        self.last_release_ms = -1.0;
        self.reset();
    }

    fn reset(&mut self) {
        self.rms_env.reset();
        self.peak_env.reset();
        self.makeup_smoother.snap_to(0.0);
        self.gr_db = 0.0;
        self.over_thresh_samples = 0;
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters) {
        if !p.comp_enabled {
            self.reset();
            m.comp_gain_reduction_db = 0.0;
            return;
        }

        let attack_ms = if p.comp_attack_ms.is_finite() {
            p.comp_attack_ms.max(0.05)
        } else {
            5.0
        };
        let release_ms = if p.comp_release_ms.is_finite() {
            p.comp_release_ms.max(1.0)
        } else {
            120.0
        };

        if (attack_ms - self.last_attack_ms).abs() > 1e-6 {
            self.attack_coeff = time_coeff(attack_ms, self.fs);
            self.last_attack_ms = attack_ms;
        }
        if (release_ms - self.last_release_ms).abs() > 1e-6 {
            self.base_release_coeff = time_coeff(release_ms, self.fs);
            self.last_release_ms = release_ms;
        }

        let target_makeup = if p.comp_auto_makeup {
            Self::auto_makeup_db(p)
        } else if p.comp_makeup_db.is_finite() {
            p.comp_makeup_db.max(0.0)
        } else {
            0.0
        };
        self.makeup_smoother.set_target(target_makeup);

        let att_coeff = self.attack_coeff;
        let base_rel_coeff = self.base_release_coeff;
        let thresh = if p.comp_thresh_db.is_finite() {
            p.comp_thresh_db
        } else {
            -20.0
        };
        let prog_win = self.program_window_samples.max(1);

        let mut gr_db = self.gr_db;
        let mut over_samples = self.over_thresh_samples;
        let mut max_gr_db = 0.0f32;

        // Cache program-dependent release coefficient for this block if releasing.
        let mut cached_rel_bucket = usize::MAX;
        let mut cached_rel_coeff = base_rel_coeff;

        for sample in buf.iter_mut() {
            let x = *sample;
            let mean_sq = self.rms_env.process_sample_sq(x);
            let peak = self.peak_env.process_sample(x);

            let peak_scaled = peak * MINUS_6_DB_LIN;
            let det_sq = mean_sq.max(peak_scaled * peak_scaled);
            let level_db = fast_sq_to_db(det_sq);

            let curve_out_db = Self::static_curve_db(p, level_db);
            let target_gr_db = (level_db - curve_out_db).max(0.0);

            if level_db > thresh {
                if over_samples < prog_win {
                    over_samples += 1;
                }
            } else if gr_db < RELEASE_RESET_GR_DB && target_gr_db <= 0.0 {
                over_samples = 0;
            }

            if target_gr_db > gr_db {
                gr_db = target_gr_db + att_coeff * (gr_db - target_gr_db);
            } else {
                let rel_coeff = if over_samples >= prog_win {
                    base_rel_coeff
                } else {
                    let bucket = over_samples >> 5;
                    if cached_rel_bucket != bucket {
                        let quantized_samples = bucket << 5;
                        let frac = (quantized_samples as f32) / (prog_win as f32);
                        let eff_rel_ms = release_ms * (0.5 + 0.5 * frac);
                        cached_rel_coeff = time_coeff(eff_rel_ms, self.fs);
                        cached_rel_bucket = bucket;
                    }
                    cached_rel_coeff
                };
                gr_db = target_gr_db + rel_coeff * (gr_db - target_gr_db);
            }

            if gr_db > max_gr_db {
                max_gr_db = gr_db;
            }

            let makeup_db = self.makeup_smoother.next();
            let total_gain_db = makeup_db - gr_db;
            let gain = fast_db_to_lin(total_gain_db);
            *sample = x * gain;
        }

        self.gr_db = gr_db;
        self.over_thresh_samples = over_samples;
        m.comp_gain_reduction_db = max_gr_db;
    }

    fn latency(&self) -> u32 {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::primitives::db::{db_to_lin, lin_to_db};

    #[test]
    fn fast_db_approximations_within_0_01_db() {
        for i in -1190..=240 {
            let db = (i as f32) * 0.1;
            let exact_lin = db_to_lin(db);
            let fast_lin = fast_db_to_lin(db);
            let err_db = (lin_to_db(fast_lin) - db).abs();
            assert!(
                err_db < 0.01,
                "fast_db_to_lin({db}): err={err_db} dB (exact={exact_lin}, fast={fast_lin})"
            );

            let exact_db = lin_to_db(exact_lin);
            let fast_db = fast_lin_to_db(exact_lin);
            assert!(
                (fast_db - exact_db).abs() < 0.01,
                "fast_lin_to_db({exact_lin}): exact={exact_db}, fast={fast_db}"
            );
        }
    }

    #[test]
    fn attack_and_release_time_constants_within_10_percent() {
        let fs = 48_000.0;
        let mut comp = CompressorStage::new();
        comp.prepare(fs, 480);

        let mut p = DspParams::DEFAULT;
        p.comp_enabled = true;
        p.comp_thresh_db = -30.0;
        p.comp_ratio = 4.0;
        p.comp_knee_db = 0.0;
        p.comp_attack_ms = 80.0;
        p.comp_release_ms = 200.0;
        p.comp_auto_makeup = false;
        p.comp_makeup_db = 0.0;

        let preroll_amp = db_to_lin(-30.5);
        let high_amp = db_to_lin(-24.0); // 6 dB above -30 -> steady GR = 4.5 dB
        let low_amp = db_to_lin(-60.0);

        let preroll_len = (0.100 * fs) as usize;
        let high_len = (0.400 * fs) as usize;
        let low_len = (0.600 * fs) as usize;
        let mut buf = alloc::vec![preroll_amp; preroll_len];
        buf.extend(core::iter::repeat_n(high_amp, high_len));
        buf.extend(core::iter::repeat_n(low_amp, low_len));

        let mut meters = StageMeters::default();
        for chunk in buf.chunks_mut(480) {
            comp.process(chunk, &p, &mut meters);
        }

        let high_slice = &buf[preroll_len..preroll_len + high_len];
        let target_attack_gr = 4.5 * (1.0 - (-1.0f32).exp());
        let attack_samples = high_slice
            .iter()
            .position(|&y| (-24.0 - lin_to_db(y.abs())) >= target_attack_gr)
            .expect("attack never reached 63.2%");
        let measured_attack_ms = (attack_samples as f32) * 1000.0 / fs;
        assert!(
            (measured_attack_ms - p.comp_attack_ms).abs() <= p.comp_attack_ms * 0.10,
            "attack_ms={measured_attack_ms:.2}, expected={} ±10%",
            p.comp_attack_ms
        );

        let low_slice = &buf[preroll_len + high_len..];
        let target_release_gr = 4.5 * (-1.0f32).exp();
        let release_samples = low_slice
            .iter()
            .position(|&y| (-60.0 - lin_to_db(y.abs())) <= target_release_gr)
            .expect("release never reached 36.8%");
        let measured_release_ms = (release_samples as f32) * 1000.0 / fs;
        assert!(
            (measured_release_ms - p.comp_release_ms).abs() <= p.comp_release_ms * 0.10,
            "release_ms={measured_release_ms:.2}, expected={} ±10%",
            p.comp_release_ms
        );
    }

    #[test]
    fn program_dependent_release_shortens_on_short_transient() {
        let fs = 48_000.0;
        let mut p = DspParams::DEFAULT;
        p.comp_enabled = true;
        p.comp_thresh_db = -30.0;
        p.comp_ratio = 4.0;
        p.comp_knee_db = 0.0;
        p.comp_attack_ms = 1.0;
        p.comp_release_ms = 200.0;
        p.comp_auto_makeup = false;
        p.comp_makeup_db = 0.0;

        let high_amp = db_to_lin(-24.0);
        let low_amp = db_to_lin(-60.0);

        let measure_release = |hold_ms: f32| -> f32 {
            let mut comp = CompressorStage::new();
            comp.prepare(fs, 480);
            let hold_len = (hold_ms * 1e-3 * fs) as usize;
            let low_len = (0.400 * fs) as usize;
            let mut buf = alloc::vec![high_amp; hold_len];
            buf.extend(core::iter::repeat_n(low_amp, low_len));
            let mut meters = StageMeters::default();
            for chunk in buf.chunks_mut(480) {
                comp.process(chunk, &p, &mut meters);
            }
            let start_gr = -60.0 - lin_to_db(buf[hold_len].abs());
            let target_gr = start_gr * (-1.0f32).exp();
            let samples = buf[hold_len..]
                .iter()
                .position(|&y| (-60.0 - lin_to_db(y.abs())) <= target_gr)
                .expect("release did not reach 1/e");
            (samples as f32) * 1000.0 / fs
        };

        let short_rel = measure_release(10.0);
        let long_rel = measure_release(100.0);
        assert!(
            short_rel < long_rel * 0.75,
            "expected shorter release for 10 ms transient: short={short_rel:.1} ms, long={long_rel:.1} ms"
        );
    }
}
