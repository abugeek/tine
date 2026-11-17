//! Stage 2: Pluggable AI denoiser (with native-rate resampling when needed) and downward expander/gate.

use alloc::boxed::Box;
use alloc::vec;

use tine_protocol::params::{DspParams, ParamId};

use super::Stage;
use crate::denoise::{Denoiser, RnnoiseDenoiser};
use crate::meter::StageMeters;
use crate::primitives::db::{db_to_lin, lin_to_db};
use crate::primitives::delay::DelayLine;
use crate::primitives::envelope::PeakEnvelope;
use crate::primitives::fifo::FrameFifo;
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;
use crate::resample::NativeRateResampler;

/// Soft-knee width in dB for the downward expander.
const EXPANDER_KNEE_DB: f32 = 6.0;

/// Minimum expander gain floor in dB.
const EXPANDER_FLOOR_DB: f32 = -80.0;

/// Smoother time constant (ms) for denoiser wet/dry mix.
const SMOOTH_MS: f32 = 10.0;

/// Fast peak detector attack time (ms) so plosives/consonants pass unclipped.
const DETECTOR_ATTACK_MS: f32 = 0.2;

/// Fast peak detector release time (ms) decoupled from the gain smoother.
const DETECTOR_RELEASE_MS: f32 = 15.0;

/// Hold time (ms) before the expander begins closing on breathy word tails.
const GATE_HOLD_MS: f32 = 60.0;

#[inline]
fn clamp_param(id: ParamId, v: f32) -> f32 {
    let spec = id.spec();
    if v.is_finite() {
        v.clamp(spec.min, spec.max)
    } else {
        spec.default
    }
}

#[inline]
fn coeff_from_ms(ms: f32, fs: f32) -> f32 {
    let t = f64::from(ms) * 1e-3;
    let fs_f64 = f64::from(fs);
    if t <= 0.0 || fs_f64 <= 0.0 || !t.is_finite() || !fs_f64.is_finite() {
        0.0
    } else {
        exp_f64(-1.0 / (t * fs_f64)) as f32
    }
}

/// Stage 2: Denoiser with latency-aligned dry/wet mix + soft-knee hysteresis expander.
pub(crate) struct CleanupStage {
    fs: f32,
    denoise: Box<dyn Denoiser>,
    in_fifo: FrameFifo,
    out_fifo: FrameFifo,
    resampler: Option<NativeRateResampler>,
    frame_in: Box<[f32]>,
    frame_out: Box<[f32]>,
    dry_delay: DelayLine,
    wet_buf: Box<[f32]>,
    mix_smoother: Smoother,
    latency_samples: u32,

    // Stage 2b: downward expander
    detector: PeakEnvelope,
    gate_open: bool,
    gain_env: f32,
    hold_samples: u32,
    hold_counter: u32,
    last_attack_ms: f32,
    last_release_ms: f32,
    attack_coeff: f32,
    release_coeff: f32,
}

impl Default for CleanupStage {
    fn default() -> Self {
        Self::new()
    }
}

impl CleanupStage {
    pub fn new() -> Self {
        Self::with_denoiser(Box::new(RnnoiseDenoiser::new()))
    }

    pub fn with_denoiser(denoise: Box<dyn Denoiser>) -> Self {
        let frame_len = denoise.frame_len().max(1);
        let native_rate = denoise.native_rate().max(1) as f32;
        let algo_latency = denoise.algo_latency();
        let initial_latency = (frame_len as u32).saturating_add(algo_latency);

        let mut s = Self {
            fs: native_rate,
            denoise,
            in_fifo: FrameFifo::new(frame_len, 512),
            out_fifo: FrameFifo::new(frame_len, frame_len + 512),
            resampler: None,
            frame_in: vec![0.0; frame_len].into_boxed_slice(),
            frame_out: vec![0.0; frame_len].into_boxed_slice(),
            dry_delay: DelayLine::new(initial_latency as usize),
            wet_buf: vec![0.0; 512].into_boxed_slice(),
            mix_smoother: Smoother::new(SMOOTH_MS, 48_000.0, DspParams::DEFAULT.denoise_mix),
            latency_samples: initial_latency,
            detector: PeakEnvelope::new(DETECTOR_ATTACK_MS, DETECTOR_RELEASE_MS, 48_000.0),
            gate_open: false,
            gain_env: 0.0,
            hold_samples: ((f64::from(GATE_HOLD_MS) * 1e-3 * 48_000.0) + 0.5) as u32,
            hold_counter: ((f64::from(GATE_HOLD_MS) * 1e-3 * 48_000.0) + 0.5) as u32,
            last_attack_ms: DspParams::DEFAULT.gate_attack_ms,
            last_release_ms: DspParams::DEFAULT.gate_release_ms,
            attack_coeff: coeff_from_ms(DspParams::DEFAULT.gate_attack_ms, 48_000.0),
            release_coeff: coeff_from_ms(DspParams::DEFAULT.gate_release_ms, 48_000.0),
        };
        s.prepare(48_000.0, 512);
        s
    }
}

impl Stage for CleanupStage {
    fn prepare(&mut self, fs: f32, max_block: usize) {
        let fs_safe = if fs.is_finite() && fs > 0.0 {
            fs.clamp(8_000.0, 192_000.0)
        } else {
            48_000.0
        };
        let max_block = max_block.max(1);
        self.fs = fs_safe;

        self.denoise.warm_up();
        let frame_len = self.denoise.frame_len().max(1);
        let native_rate = self.denoise.native_rate().max(1);
        let algo_latency = self.denoise.algo_latency();

        self.frame_in = vec![0.0; frame_len].into_boxed_slice();
        self.frame_out = vec![0.0; frame_len].into_boxed_slice();
        self.wet_buf = vec![0.0; max_block].into_boxed_slice();
        self.mix_smoother = Smoother::new(SMOOTH_MS, fs_safe, DspParams::DEFAULT.denoise_mix);

        let is_native_rate = (fs_safe - native_rate as f32).abs() < 1.0;
        if is_native_rate {
            self.resampler = None;
            self.in_fifo = FrameFifo::new(frame_len, max_block);
            self.out_fifo = FrameFifo::new(frame_len, frame_len + max_block);
            self.out_fifo.push_zeros(frame_len);
            self.latency_samples = (frame_len as u32).saturating_add(algo_latency);
        } else if let Some(resampler) = NativeRateResampler::new(fs_safe, native_rate, frame_len, max_block) {
            let scaled_algo = ((f64::from(algo_latency) * f64::from(fs_safe)) / f64::from(native_rate) + 0.5) as u32;
            self.latency_samples = resampler.latency().saturating_add(scaled_algo);
            self.resampler = Some(resampler);
        } else {
            self.resampler = None;
            self.in_fifo = FrameFifo::new(frame_len, max_block);
            self.out_fifo = FrameFifo::new(frame_len, frame_len + max_block);
            self.out_fifo.push_zeros(frame_len);
            self.latency_samples = (frame_len as u32).saturating_add(algo_latency);
        }

        self.dry_delay = DelayLine::new(self.latency_samples as usize);
        self.frame_in.fill(0.0);
        self.frame_out.fill(0.0);

        self.last_attack_ms = DspParams::DEFAULT.gate_attack_ms;
        self.last_release_ms = DspParams::DEFAULT.gate_release_ms;
        self.detector = PeakEnvelope::new(DETECTOR_ATTACK_MS, DETECTOR_RELEASE_MS, fs_safe);
        self.attack_coeff = coeff_from_ms(self.last_attack_ms, fs_safe);
        self.release_coeff = coeff_from_ms(self.last_release_ms, fs_safe);
        self.hold_samples = ((f64::from(GATE_HOLD_MS) * 1e-3 * f64::from(fs_safe)) + 0.5) as u32;
        self.hold_counter = self.hold_samples;
        self.gate_open = false;
        self.gain_env = 0.0;
    }

    fn reset(&mut self) {
        self.denoise.reset();
        self.in_fifo.reset();
        self.out_fifo.reset();
        if self.resampler.is_none() {
            self.out_fifo.push_zeros(self.denoise.frame_len().max(1));
        }
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.frame_in.fill(0.0);
        self.frame_out.fill(0.0);
        self.dry_delay.reset();
        self.mix_smoother.snap();
        self.detector.reset();
        self.gate_open = false;
        self.gain_env = 0.0;
        self.hold_counter = self.hold_samples;
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters) {
        if buf.is_empty() {
            return;
        }

        let n = buf.len();
        let Some(wet_slice) = self.wet_buf.get_mut(..n) else {
            return;
        };

        // Stage 2a: Always run denoiser so latency and CPU stay constant across mix changes.
        if let Some(resampler) = self.resampler.as_mut() {
            let denoise = &mut self.denoise;
            resampler.process_frames(buf, wet_slice, |in_frame, out_frame| {
                let _vad = denoise.process_frame(in_frame, out_frame);
            });
        } else {
            self.in_fifo.push_slice(buf);
            while self.in_fifo.pop_frame(&mut self.frame_in) {
                let _vad = self.denoise.process_frame(&self.frame_in, &mut self.frame_out);
                self.out_fifo.push_slice(&self.frame_out);
            }
            let popped = self.out_fifo.pop_slice(wet_slice);
            if popped < n
                && let Some(tail) = wet_slice.get_mut(popped..)
            {
                tail.fill(0.0);
            }
        }

        let mix_target = clamp_param(ParamId::DenoiseMix, p.denoise_mix);
        self.mix_smoother.set_target(mix_target);
        let delay = self.latency_samples as usize;

        for (sample, &wet) in buf.iter_mut().zip(wet_slice.iter()) {
            let dry = self.dry_delay.push_read(*sample, delay);
            let mix = self.mix_smoother.next();
            *sample = dry * (1.0 - mix) + wet * mix;
        }

        // Stage 2b: Soft-knee hysteresis downward expander.
        let attack_ms = clamp_param(ParamId::GateAttackMs, p.gate_attack_ms);
        let release_ms = clamp_param(ParamId::GateReleaseMs, p.gate_release_ms);
        if (attack_ms - self.last_attack_ms).abs() > 1e-4 || (release_ms - self.last_release_ms).abs() > 1e-4 {
            self.last_attack_ms = attack_ms;
            self.last_release_ms = release_ms;
            self.attack_coeff = coeff_from_ms(attack_ms, self.fs);
            self.release_coeff = coeff_from_ms(release_ms, self.fs);
        }

        let threshold = clamp_param(ParamId::GateThresholdDb, p.gate_threshold_db);
        let hysteresis = clamp_param(ParamId::GateHysteresisDb, p.gate_hysteresis_db);
        let ratio = clamp_param(ParamId::GateRatio, p.gate_ratio);
        let slope = ratio - 1.0;
        let half_knee = EXPANDER_KNEE_DB * 0.5;
        let close_threshold = threshold - hysteresis;

        let mut gate_open = self.gate_open;
        let mut gain_env = self.gain_env;
        let mut hold_counter = self.hold_counter;
        let hold_samples = self.hold_samples;
        let att_c = self.attack_coeff;
        let rel_c = self.release_coeff;
        let mut min_gain_env_db = 0.0f32;

        for sample in buf.iter_mut() {
            let env = self.detector.process_sample(*sample);
            let target_gain_db = if p.gate_enabled {
                let level_db = lin_to_db(env);
                if gate_open {
                    if level_db < close_threshold {
                        gate_open = false;
                    }
                } else if level_db >= threshold {
                    gate_open = true;
                }

                let eff_thresh = if gate_open { close_threshold } else { threshold };
                let gain_db = if level_db >= eff_thresh + half_knee {
                    0.0
                } else if level_db > eff_thresh - half_knee {
                    let d = (eff_thresh + half_knee) - level_db;
                    -slope * (d * d) / (2.0 * EXPANDER_KNEE_DB)
                } else {
                    slope * (level_db - eff_thresh)
                };
                gain_db.max(EXPANDER_FLOOR_DB)
            } else {
                gate_open = true;
                0.0
            };

            if target_gain_db >= gain_env {
                hold_counter = hold_samples;
                gain_env = target_gain_db + att_c * (gain_env - target_gain_db);
            } else if hold_counter > 0 {
                hold_counter -= 1;
            } else {
                gain_env = target_gain_db + rel_c * (gain_env - target_gain_db);
            }

            if gain_env < min_gain_env_db {
                min_gain_env_db = gain_env;
            }
            *sample *= db_to_lin(gain_env);
        }

        self.gate_open = gate_open;
        self.gain_env = gain_env;
        self.hold_counter = hold_counter;
        m.gate_attenuation_db = if p.gate_enabled { min_gain_env_db } else { 0.0 };
    }

    fn latency(&self) -> u32 {
        self.latency_samples
    }
}
