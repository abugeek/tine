//! Stage 4: Split-band de-esser.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;

/// Stage 4 pass-through stub (implemented in T07).
#[derive(Debug, Default)]
pub(crate) struct DeesserStage;

impl DeesserStage {
    pub fn new() -> Self {
        Self
    }
}

impl Stage for DeesserStage {
    fn prepare(&mut self, _fs: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _buf: &mut [f32], _p: &DspParams, _m: &mut StageMeters) {}
    fn latency(&self) -> u32 {
        0
    }
}
