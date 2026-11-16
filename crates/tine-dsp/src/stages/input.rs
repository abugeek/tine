//! Stage 1: Input gain, DC blocker, and 2nd-order Butterworth HPF.

use core::f64::consts::{FRAC_1_SQRT_2, PI};

use tine_protocol::params::{DspParams, FilterType, ParamId};

use super::Stage;
use crate::meter::StageMeters;
use crate::primitives::biquad::Biquad;
use crate::primitives::db::db_to_lin;
use crate::primitives::math::exp_f64;
use crate::primitives::smoother::Smoother;

/// Corner frequency of the DC-blocking one-pole high-pass filter (Hz).
const DC_BLOCK_HZ: f64 = 5.0;

/// Sub-block size for smoother-driven biquad retuning.
const SUB_BLOCK_SIZE: usize = 32;

/// Relative parameter threshold for biquad coefficient recomputation.
const RETUNE_EPS: f32 = 1e-4;

/// Smoother time constant (ms) for control transitions.
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

/// Stage 1: smoothed input gain, 5 Hz DC blocker, and cross-faded 2nd-order Butterworth HPF.
#[derive(Clone, Debug)]
pub(crate) struct InputStage {
    fs_f64: f64,
    dc_r: f32,
    dc_x1: f32,
    dc_y1: f32,
    hpf: Biquad,
    gain_smoother: Smoother,
    hpf_freq_smoother: Smoother,
    hpf_mix_smoother: Smoother,
}

impl Default for InputStage {
    fn default() -> Self {
        Self::new()
    }
}

impl InputStage {
    pub fn new() -> Self {
        let mut s = Self {
            fs_f64: 48_000.0,
            dc_r: 0.999_345_7,
            dc_x1: 0.0,
            dc_y1: 0.0,
            hpf: Biquad::default(),
            gain_smoother: Smoother::new(SMOOTH_MS, 48_000.0, 1.0),
            hpf_freq_smoother: Smoother::new(SMOOTH_MS, 48_000.0, DspParams::DEFAULT.hpf_freq_hz),
            hpf_mix_smoother: Smoother::new(
                SMOOTH_MS,
                48_000.0,
                if DspParams::DEFAULT.hpf_enabled { 1.0 } else { 0.0 },
            ),
        };
        s.prepare(48_000.0, 512);
        s
    }
}

impl Stage for InputStage {
    fn prepare(&mut self, fs: f32, _max_block: usize) {
        let fs_safe = if fs.is_finite() && fs > 0.0 { fs } else { 48_000.0 };
        self.fs_f64 = f64::from(fs_safe);
        self.dc_r = exp_f64(-2.0 * PI * DC_BLOCK_HZ / self.fs_f64) as f32;
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;

        self.gain_smoother = Smoother::new(SMOOTH_MS, fs_safe, 1.0);
        self.hpf_freq_smoother = Smoother::new(SMOOTH_MS, fs_safe, DspParams::DEFAULT.hpf_freq_hz);
        self.hpf_mix_smoother = Smoother::new(
            SMOOTH_MS,
            fs_safe,
            if DspParams::DEFAULT.hpf_enabled { 1.0 } else { 0.0 },
        );

        self.hpf.reset();
        self.hpf.retune_if_changed(
            FilterType::HighPass,
            self.fs_f64,
            DspParams::DEFAULT.hpf_freq_hz,
            FRAC_1_SQRT_2 as f32,
            0.0,
            0.0,
        );
    }

    fn reset(&mut self) {
        self.dc_x1 = 0.0;
        self.dc_y1 = 0.0;
        self.hpf.reset();
        self.gain_smoother.snap();
        self.hpf_freq_smoother.snap();
        self.hpf_mix_smoother.snap();
    }

    fn process(&mut self, buf: &mut [f32], p: &DspParams, _m: &mut StageMeters) {
        if buf.is_empty() {
            return;
        }

        let gain_db = clamp_param(ParamId::InputGainDb, p.input_gain_db);
        self.gain_smoother.set_target(db_to_lin(gain_db));

        let hpf_freq = clamp_param(ParamId::HpfFreqHz, p.hpf_freq_hz);
        self.hpf_freq_smoother.set_target(hpf_freq);
        self.hpf_mix_smoother.set_target(if p.hpf_enabled { 1.0 } else { 0.0 });

        let dc_r = self.dc_r;
        let mut dc_x1 = self.dc_x1;
        let mut dc_y1 = self.dc_y1;

        for sub in buf.chunks_mut(SUB_BLOCK_SIZE) {
            self.hpf_freq_smoother.skip(sub.len());
            let smoothed_freq = self.hpf_freq_smoother.current();
            self.hpf.retune_if_changed(
                FilterType::HighPass,
                self.fs_f64,
                smoothed_freq,
                FRAC_1_SQRT_2 as f32,
                0.0,
                RETUNE_EPS,
            );

            for sample in sub.iter_mut() {
                let g = self.gain_smoother.next();
                let xg = *sample * g;

                let dc_out = xg - dc_x1 + dc_r * dc_y1;
                dc_x1 = xg;
                dc_y1 = dc_out;

                let hpf_out = self.hpf.process_sample(dc_out);
                let hpf_mix = self.hpf_mix_smoother.next();
                *sample = dc_out * (1.0 - hpf_mix) + hpf_out * hpf_mix;
            }
        }

        self.dc_x1 = dc_x1;
        self.dc_y1 = dc_y1;
    }

    fn latency(&self) -> u32 {
        0
    }
}
