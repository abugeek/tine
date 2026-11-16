//! Individual DSP pipeline stages.

use tine_protocol::params::DspParams;

use crate::meter::StageMeters;

pub mod cleanup;
pub mod compressor;
pub mod deesser;
pub mod eq;
pub mod input;
pub mod limiter;

pub(crate) use cleanup::CleanupStage;
pub(crate) use compressor::CompressorStage;
pub(crate) use deesser::DeesserStage;
pub(crate) use eq::EqStage;
pub(crate) use input::InputStage;
pub(crate) use limiter::LimiterStage;

/// Internal per-stage contract implemented by Stages 1–6.
pub(crate) trait Stage {
    fn prepare(&mut self, fs: f32, max_block: usize);
    fn reset(&mut self);
    fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters);
    fn latency(&self) -> u32;
}
