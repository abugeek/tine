//! Stage 2: `RNNoise` AI denoiser (with 48 kHz resampling when needed) and downward expander/gate.

use alloc::boxed::Box;
use alloc::vec;

use nnnoiseless::DenoiseState;
use tine_protocol::params::{DspParams, ParamId};

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::db::{db_to_lin, lin_to_db};
use crate::primitives::delay::DelayLine;
use crate::primitives::envelope::PeakEnvelope;
use crate::primitives::fifo::FrameFifo;
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;
use crate::resample::{RNNOISE_FRAME, RnnoiseResampler};

/// Scale factor between `[-1.0, 1.0]` float audio and `RNNoise` 16-bit range.
const RNNOISE_SCALE: f32 = 32_768.0;
const RNNOISE_INV_SCALE: f32 = 1.0 / 32_768.0;

/// Soft-knee width in dB for the downward expander.
const EXPANDER_KNEE_DB: f32 = 6.0;

/// Minimum expander gain floor in dB.
const EXPANDER_FLOOR_DB: f32 = -80.0;

/// Smoother time constant (ms) for denoiser wet/dry mix.
const SMOOTH_MS: f32 = 10.0;

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

/// Stage 2: `RNNoise` denoiser with latency-aligned dry/wet mix + soft-knee hysteresis expander.
pub(crate) struct CleanupStage {
    fs: f32,
    denoise: Box<DenoiseState<'static>>,
    in_fifo: FrameFifo,
    out_fifo: FrameFifo,
    resampler: Option<RnnoiseResampler>,
    frame_in: [f32; RNNOISE_FRAME],
    frame_out: [f32; RNNOISE_FRAME],
    dry_delay: DelayLine,
    wet_buf: Box<[f32]>,
    mix_smoother: Smoother,
    latency_samples: u32,

    // Stage 2b: downward expander
    detector: PeakEnvelope,
    gate_open: bool,
    gain_env: f32,
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
        let mut s = Self {
            fs: 48_000.0,
            denoise: DenoiseState::new(),
            in_fifo: FrameFifo::new(RNNOISE_FRAME, 512),
            out_fifo: FrameFifo::new(RNNOISE_FRAME, RNNOISE_FRAME + 512),
            resampler: None,
            frame_in: [0.0; RNNOISE_FRAME],
            frame_out: [0.0; RNNOISE_FRAME],
            dry_delay: DelayLine::new(RNNOISE_FRAME * 2),
            wet_buf: vec![0.0; 512].into_boxed_slice(),
            mix_smoother: Smoother::new(SMOOTH_MS, 48_000.0, DspParams::DEFAULT.denoise_mix),
            latency_samples: (RNNOISE_FRAME * 2) as u32,
            detector: PeakEnvelope::new(
                DspParams::DEFAULT.gate_attack_ms,
                DspParams::DEFAULT.gate_release_ms,
                48_000.0,
            ),
            gate_open: false,
            gain_env: 1.0,
            last_attack_ms: DspParams::DEFAULT.gate_attack_ms,
            last_release_ms: DspParams::DEFAULT.gate_release_ms,
            attack_coeff: coeff_from_ms(DspParams::DEFAULT.gate_attack_ms, 48_000.0),
            release_coeff: coeff_from_ms(DspParams::DEFAULT.gate_release_ms, 48_000.0),
        };
        s.prepare(48_000.0, 512);
        s
    }

    fn process_48k_frame(
        denoise: &mut DenoiseState<'static>,
        in_48k: &[f32; RNNOISE_FRAME],
        out_48k: &mut [f32; RNNOISE_FRAME],
        scratch_in: &mut [f32; RNNOISE_FRAME],
    ) {
        for (dst, &src) in scratch_in.iter_mut().zip(in_48k.iter()) {
            *dst = src * RNNOISE_SCALE;
        }
        let _vad = denoise.process_frame(out_48k.as_mut_slice(), scratch_in.as_slice());
        for s in out_48k.iter_mut() {
            *s *= RNNOISE_INV_SCALE;
        }
    }
}

impl Stage for CleanupStage {
    fn prepare(&mut self, fs: f32, max_block: usize) {
        let fs_safe = if fs.is_finite() && fs > 0.0 { fs } else { 48_000.0 };
        let max_block = max_block.max(1);
        self.fs = fs_safe;

        // Warm up nnnoiseless global OnceCell state outside RT, then start with a fresh DenoiseState.
        let mut warmup = DenoiseState::new();
        let zeros = [0.0f32; RNNOISE_FRAME];
        let mut dummy_out = [0.0f32; RNNOISE_FRAME];
        let _ = warmup.process_frame(&mut dummy_out, &zeros);
        self.denoise = DenoiseState::new();

        self.wet_buf = vec![0.0; max_block].into_boxed_slice();
        self.mix_smoother = Smoother::new(SMOOTH_MS, fs_safe, DspParams::DEFAULT.denoise_mix);

        let is_48k = (fs_safe - 48_000.0).abs() < 1.0;
        if is_48k {
            self.resampler = None;
            self.in_fifo = FrameFifo::new(RNNOISE_FRAME, max_block);
            self.out_fifo = FrameFifo::new(RNNOISE_FRAME, RNNOISE_FRAME + max_block);
            self.out_fifo.push_zeros(RNNOISE_FRAME);
            // 480 samples FIFO framing + 480 samples RNNoise overlap-add = 960 samples (20 ms).
            self.latency_samples = (RNNOISE_FRAME * 2) as u32;
        } else {
            let resampler = RnnoiseResampler::new(fs_safe, max_block);
            self.latency_samples = resampler.latency();
            self.resampler = Some(resampler);
        }

        self.dry_delay = DelayLine::new(self.latency_samples as usize);
        self.frame_in.fill(0.0);
        self.frame_out.fill(0.0);

        self.last_attack_ms = DspParams::DEFAULT.gate_attack_ms;
        self.last_release_ms = DspParams::DEFAULT.gate_release_ms;
        self.detector = PeakEnvelope::new(self.last_attack_ms, self.last_release_ms, fs_safe);
        self.attack_coeff = coeff_from_ms(self.last_attack_ms, fs_safe);
        self.release_coeff = coeff_from_ms(self.last_release_ms, fs_safe);
        self.gate_open = false;
        self.gain_env = 1.0;
    }

    fn reset(&mut self) {
        self.denoise = DenoiseState::new();
        self.in_fifo.reset();
        self.out_fifo.reset();
        if self.resampler.is_none() {
            self.out_fifo.push_zeros(RNNOISE_FRAME);
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
        self.gain_env = 1.0;
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters) {
        if buf.is_empty() {
            return;
        }

        let n = buf.len();
        let Some(wet_slice) = self.wet_buf.get_mut(..n) else {
            return;
        };

        // Stage 2a: Always run RNNoise so latency and CPU stay constant across mix changes.
        if let Some(resampler) = self.resampler.as_mut() {
            let denoise = &mut self.denoise;
            let scratch_in = &mut self.frame_in;
            resampler.process_frames(buf, wet_slice, |in_48k, out_48k| {
                Self::process_48k_frame(denoise, in_48k, out_48k, scratch_in);
            });
        } else {
            self.in_fifo.push_slice(buf);
            while self.in_fifo.pop_frame(&mut self.frame_in) {
                for s in &mut self.frame_in {
                    *s *= RNNOISE_SCALE;
                }
                let _vad = self
                    .denoise
                    .process_frame(self.frame_out.as_mut_slice(), self.frame_in.as_slice());
                for s in &mut self.frame_out {
                    *s *= RNNOISE_INV_SCALE;
                }
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
            self.detector.set_times(attack_ms, release_ms, self.fs);
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
        let att_c = self.attack_coeff;
        let rel_c = self.release_coeff;
        let mut min_gain_env = 1.0f32;

        for sample in buf.iter_mut() {
            let env = self.detector.process_sample(*sample);
            let target_gain = if p.gate_enabled {
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
                db_to_lin(gain_db.max(EXPANDER_FLOOR_DB))
            } else {
                gate_open = true;
                1.0
            };

            let coeff = if target_gain > gain_env { att_c } else { rel_c };
            gain_env = target_gain + coeff * (gain_env - target_gain);
            if gain_env < min_gain_env {
                min_gain_env = gain_env;
            }
            *sample *= gain_env;
        }

        self.gate_open = gate_open;
        self.gain_env = gain_env;
        m.gate_attenuation_db = if p.gate_enabled { lin_to_db(min_gain_env) } else { 0.0 };
    }

    fn latency(&self) -> u32 {
        self.latency_samples
    }
}
