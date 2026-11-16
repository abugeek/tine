//! Stage 3: 5-band parametric EQ.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;

/// Stage 3 pass-through stub (implemented in T07).
#[derive(Debug, Default)]
pub(crate) struct EqStage;

impl EqStage {
    pub fn new() -> Self {
        Self
    }
}

impl Stage for EqStage {
    fn prepare(&mut self, _fs: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _buf: &mut [f32], _p: &DspParams, _m: &mut StageMeters) {}
    fn latency(&self) -> u32 {
        0
    }
}
