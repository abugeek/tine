//! Stage 5: Program-dependent vocal compressor.

use tine_protocol::params::{DspParams, FilterType};

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::biquad::{Biquad, Coeffs};
use crate::primitives::db::{DB_FLOOR, fast_db_to_lin, fast_sq_to_db};
use crate::primitives::envelope::{PeakEnvelope, RmsEnvelope};
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;

/// `10^(-6/20)` for `peak - 6 dB` comparison in linear domain.
const MINUS_6_DB_LIN: f32 = 0.501_187_2;

/// Sidechain high-pass cutoff (Hz) so sub-bass desk thumps and plosives do not pump vocal compression.
const SC_HPF_HZ: f64 = 110.0;

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
    sc_hpf: Biquad,
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
            sc_hpf: Biquad::new(Coeffs::design(
                FilterType::HighPass,
                48_000.0,
                SC_HPF_HZ,
                core::f64::consts::FRAC_1_SQRT_2,
                0.0,
            )),
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
        self.sc_hpf.set_coeffs(Coeffs::design(
            FilterType::HighPass,
            f64::from(fs),
            SC_HPF_HZ,
            core::f64::consts::FRAC_1_SQRT_2,
            0.0,
        ));
        self.rms_env.set_times(RMS_WINDOW_MS, RMS_WINDOW_MS, fs);
        self.peak_env.set_times(0.0, PEAK_RELEASE_MS, fs);
        self.makeup_smoother.set_time_constant(10.0, fs);
        self.program_window_samples = ((PROGRAM_RELEASE_WINDOW_MS * 1e-3 * fs) + 0.5) as usize;
        self.last_attack_ms = -1.0;
        self.last_release_ms = -1.0;
        self.reset();
    }

    fn reset(&mut self) {
        self.sc_hpf.reset();
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
            let sc = self.sc_hpf.process_sample(x);
            let mean_sq = self.rms_env.process_sample_sq(sc);
            let peak = self.peak_env.process_sample(sc);

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
    use crate::primitives::math::{sin_cos_f64, sqrt_f64};

    fn sine_2k_into(out: &mut alloc::vec::Vec<f32>, rms_amp: f32, len: usize, start_idx: usize, fs: f32) {
        let peak = f64::from(rms_amp) * core::f64::consts::SQRT_2;
        let w = 2.0 * core::f64::consts::PI * 2_000.0 / f64::from(fs);
        for k in 0..len {
            let (s, _) = sin_cos_f64(w * ((start_idx + k) as f64));
            out.push((peak * s) as f32);
        }
    }

    fn cycle_rms_db(chunk: &[f32]) -> f32 {
        let sum_sq: f64 = chunk.iter().map(|&x| f64::from(x) * f64::from(x)).sum();
        let rms = sqrt_f64(sum_sq / (chunk.len().max(1) as f64)) as f32;
        lin_to_db(rms)
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
        let mut buf = alloc::vec::Vec::with_capacity(preroll_len + high_len + low_len);
        sine_2k_into(&mut buf, preroll_amp, preroll_len, 0, fs);
        sine_2k_into(&mut buf, high_amp, high_len, preroll_len, fs);
        sine_2k_into(&mut buf, low_amp, low_len, preroll_len + high_len, fs);

        let mut meters = StageMeters::default();
        for chunk in buf.chunks_mut(480) {
            comp.process(chunk, &p, &mut meters);
        }

        let cycle: usize = 24; // 1 full cycle of 2 kHz @ 48 kHz = 0.5 ms
        let high_slice = &buf[preroll_len..preroll_len + high_len];
        let target_attack_gr = 4.5 * (1.0 - (-1.0f32).exp());
        let attack_cycle = high_slice
            .chunks_exact(cycle)
            .position(|c| (-24.0 - cycle_rms_db(c)) >= target_attack_gr)
            .expect("attack never reached 63.2%");
        let measured_attack_ms = ((attack_cycle * cycle + cycle / 2) as f32) * 1000.0 / fs;
        assert!(
            (measured_attack_ms - p.comp_attack_ms).abs() <= p.comp_attack_ms * 0.10,
            "attack_ms={measured_attack_ms:.2}, expected={} ±10%",
            p.comp_attack_ms
        );

        let low_slice = &buf[preroll_len + high_len..];
        let target_release_gr = 4.5 * (-1.0f32).exp();
        let release_cycle = low_slice
            .chunks_exact(cycle)
            .position(|c| (-60.0 - cycle_rms_db(c)) <= target_release_gr)
            .expect("release never reached 36.8%");
        let measured_release_ms = ((release_cycle * cycle + cycle / 2) as f32) * 1000.0 / fs;
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
        let cycle: usize = 24;

        let measure_release = |hold_ms: f32| -> f32 {
            let mut comp = CompressorStage::new();
            comp.prepare(fs, 480);
            let hold_len = (hold_ms * 1e-3 * fs) as usize;
            let low_len = (0.400 * fs) as usize;
            let mut buf = alloc::vec::Vec::with_capacity(hold_len + low_len);
            sine_2k_into(&mut buf, high_amp, hold_len, 0, fs);
            sine_2k_into(&mut buf, low_amp, low_len, hold_len, fs);
            let mut meters = StageMeters::default();
            for chunk in buf.chunks_mut(480) {
                comp.process(chunk, &p, &mut meters);
            }
            let start_gr = -60.0 - cycle_rms_db(&buf[hold_len..hold_len + cycle]);
            let target_gr = start_gr * (-1.0f32).exp();
            let c_idx = buf[hold_len..]
                .chunks_exact(cycle)
                .position(|c| (-60.0 - cycle_rms_db(c)) <= target_gr)
                .expect("release did not reach 1/e");
            ((c_idx * cycle) as f32) * 1000.0 / fs
        };

        let short_rel = measure_release(10.0);
        let long_rel = measure_release(100.0);
        assert!(
            short_rel < long_rel * 0.75,
            "expected shorter release for 10 ms transient: short={short_rel:.1} ms, long={long_rel:.1} ms"
        );
    }

    #[test]
    fn sidechain_hpf_ignores_sub_bass_plosive_pumping() {
        let fs = 48_000.0;
        let mut p = DspParams::DEFAULT;
        p.comp_enabled = true;
        p.comp_thresh_db = -20.0;
        p.comp_ratio = 4.0;
        p.comp_knee_db = 0.0;
        p.comp_attack_ms = 2.0;
        p.comp_release_ms = 100.0;
        p.comp_auto_makeup = false;
        p.comp_makeup_db = 0.0;

        let measure_gr = |freq_hz: f64| -> f32 {
            let mut comp = CompressorStage::new();
            comp.prepare(fs, 480);
            let peak = f64::from(db_to_lin(-10.0)) * core::f64::consts::SQRT_2;
            let w = 2.0 * core::f64::consts::PI * freq_hz / f64::from(fs);
            let mut buf: alloc::vec::Vec<f32> = (0..24_000i32)
                .map(|i| {
                    let (s, _) = sin_cos_f64(w * f64::from(i));
                    (peak * s) as f32
                })
                .collect();
            let mut meters = StageMeters::default();
            for chunk in buf.chunks_mut(480) {
                comp.process(chunk, &p, &mut meters);
            }
            meters.comp_gain_reduction_db
        };

        let gr_45hz = measure_gr(45.0);
        let gr_1khz = measure_gr(1_000.0);
        assert!(
            gr_45hz < 1.5,
            "expected 45 Hz plosive band to trigger < 1.5 dB GR, got {gr_45hz:.2} dB"
        );
        assert!(
            gr_1khz > 6.5,
            "expected 1 kHz vocal band to trigger > 6.5 dB GR, got {gr_1khz:.2} dB"
        );
    }
}
