//! Stage 6: Brickwall lookahead limiter.

use tine_protocol::params::DspParams;

use super::Stage;
use crate::meter::StageMeters;

/// Stage 6 pass-through stub (implemented in T08).
#[derive(Debug, Default)]
pub(crate) struct LimiterStage;

impl LimiterStage {
    pub fn new() -> Self {
        Self
    }
}

impl Stage for LimiterStage {
    fn prepare(&mut self, _fs: f32, _max_block: usize) {}
    fn reset(&mut self) {}
    fn process(&mut self, _buf: &mut [f32], _p: &DspParams, _m: &mut StageMeters) {}
    fn latency(&self) -> u32 {
        0
    }
}
