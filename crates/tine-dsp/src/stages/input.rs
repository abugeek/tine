//! Stage 1: Input gain, DC blocker, and Butterworth HPF.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;

/// Stage 1 pass-through stub (implemented in T06).
#[derive(Debug, Default)]
pub(crate) struct InputStage;

impl InputStage {
    pub fn new() -> Self {
        Self
    }
}

impl Stage for InputStage {
    fn prepare(&mut self, _fs: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _buf: &mut [f32], _p: &DspParams, _m: &mut StageMeters) {}
    fn latency(&self) -> u32 {
        0
    }
}
