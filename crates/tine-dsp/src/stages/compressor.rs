//! Stage 5: Program-dependent vocal compressor.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;

/// Stage 5 pass-through stub (implemented in T08).
#[derive(Debug, Default)]
pub(crate) struct CompressorStage;

impl CompressorStage {
    pub fn new() -> Self {
        Self
    }
}

impl Stage for CompressorStage {
    fn prepare(&mut self, _fs: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _buf: &mut [f32], _p: &DspParams, _m: &mut StageMeters) {}
    fn latency(&self) -> u32 {
        0
    }
}
