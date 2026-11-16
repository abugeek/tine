//! 7-stage real-time `DspPipeline`.

use alloc::boxed::Box;
use alloc::vec;

use tine_protocol::params::DspParams;
use tine_protocol::telemetry::flags;

use crate::meter::{MeterSink, MeterStage, StageMeters, TapSink};
use crate::primitives::delay::DelayLine;
use crate::primitives::smoother::Smoother;
use crate::rt::DenormalGuard;
use crate::stages::{CleanupStage, CompressorStage, DeesserStage, EqStage, InputStage, LimiterStage, Stage};

/// Real-time 7-stage microphone DSP pipeline.
///
/// All heap memory is allocated once in [`DspPipeline::new`]; [`DspPipeline::process`]
/// is lock-free, allocation-free, and panic-free.
pub struct DspPipeline {
    max_block: usize,
    latency: u32,
    passthrough_latched: bool,
    input: InputStage,
    cleanup: CleanupStage,
    eq: EqStage,
    deesser: DeesserStage,
    compressor: CompressorStage,
    limiter: LimiterStage,
    meter: MeterStage,
    dry_delay: DelayLine,
    dry_buf: Box<[f32]>,
    in_buf: Box<[f32]>,
    bypass_smoother: Smoother,
}

impl DspPipeline {
    /// Constructs and prepares a new [`DspPipeline`] for `sample_rate` and chunks up to `max_block`.
    ///
    /// All heap allocations happen here.
    #[must_use]
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        let max_block = max_block.max(1);
        let fs = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate
        } else {
            48_000.0
        };

        let mut input = InputStage::new();
        let mut cleanup = CleanupStage::new();
        let mut eq = EqStage::new();
        let mut deesser = DeesserStage::new();
        let mut compressor = CompressorStage::new();
        let mut limiter = LimiterStage::new();

        input.prepare(fs, max_block);
        cleanup.prepare(fs, max_block);
        eq.prepare(fs, max_block);
        deesser.prepare(fs, max_block);
        compressor.prepare(fs, max_block);
        limiter.prepare(fs, max_block);

        let latency = input.latency()
            + cleanup.latency()
            + eq.latency()
            + deesser.latency()
            + compressor.latency()
            + limiter.latency();

        Self {
            max_block,
            latency,
            passthrough_latched: false,
            input,
            cleanup,
            eq,
            deesser,
            compressor,
            limiter,
            meter: MeterStage::new(fs),
            dry_delay: DelayLine::new(latency as usize),
            dry_buf: vec![0.0; max_block].into_boxed_slice(),
            in_buf: vec![0.0; max_block].into_boxed_slice(),
            bypass_smoother: Smoother::new(10.0, fs, 0.0),
        }
    }

    /// Processes `buf` in-place through Stages 1–7.
    ///
    /// Accepts any slice length by chunking internally to `max_block`.
    pub fn process(
        &mut self,
        buf: &mut [f32],
        params: &DspParams,
        meters: &mut impl MeterSink,
        tap: &mut impl TapSink,
    ) {
        crate::debug_assert_rt!();
        if buf.is_empty() {
            return;
        }
        let _denormal_guard = DenormalGuard::new();

        let delay = self.latency as usize;
        self.bypass_smoother.set_target(if params.bypass { 1.0 } else { 0.0 });

        for chunk in buf.chunks_mut(self.max_block) {
            let n = chunk.len();

            // Snapshot input for metering and populate latency-aligned dry path.
            let mut input_non_finite = false;
            for ((&x, in_slot), dry_slot) in chunk
                .iter()
                .zip(self.in_buf.iter_mut().take(n))
                .zip(self.dry_buf.iter_mut().take(n))
            {
                *in_slot = x;
                let safe_x = if x.is_finite() {
                    x
                } else {
                    input_non_finite = true;
                    0.0
                };
                *dry_slot = self.dry_delay.push_read(safe_x, delay);
            }

            if input_non_finite && !self.passthrough_latched {
                self.reset_stages();
                self.passthrough_latched = true;
            }

            let mut stage_meters = StageMeters::default();

            if self.passthrough_latched {
                if let Some(dry) = self.dry_buf.get(..n) {
                    chunk.copy_from_slice(dry);
                }
            } else {
                self.input.process(chunk, params, &mut stage_meters);
                self.cleanup.process(chunk, params, &mut stage_meters);
                self.eq.process(chunk, params, &mut stage_meters);
                self.deesser.process(chunk, params, &mut stage_meters);
                self.compressor.process(chunk, params, &mut stage_meters);
                self.limiter.process(chunk, params, &mut stage_meters);

                let has_non_finite = chunk.iter().any(|s| !s.is_finite());
                if has_non_finite {
                    self.reset_stages();
                    self.passthrough_latched = true;
                    if let Some(dry) = self.dry_buf.get(..n) {
                        chunk.copy_from_slice(dry);
                    }
                } else {
                    for (wet, &dry) in chunk.iter_mut().zip(self.dry_buf.iter().take(n)) {
                        let g = self.bypass_smoother.next();
                        *wet = *wet * (1.0 - g) + dry * g;
                    }
                }
            }

            let mut base_flags = 0u32;
            if params.bypass {
                base_flags |= flags::BYPASS;
            }
            if self.passthrough_latched {
                base_flags |= flags::PASSTHROUGH_LATCHED;
            }

            let in_slice = self.in_buf.get(..n).unwrap_or(&[]);
            self.meter
                .accumulate_and_publish(in_slice, chunk, &stage_meters, base_flags, meters);
            tap.push(chunk);
        }
    }

    /// Resets all stage states, meters, dry delay, and clears the pass-through latch.
    pub fn reset(&mut self) {
        self.reset_stages();
        self.dry_delay.reset();
        self.meter.reset();
        self.bypass_smoother.snap_to(0.0);
        self.passthrough_latched = false;
    }

    /// Returns the total pipeline latency in samples (constant for the pipeline's lifetime).
    #[inline]
    #[must_use]
    pub const fn latency_samples(&self) -> u32 {
        self.latency
    }

    /// Returns `true` if a NaN/Inf fault latched the pipeline into pass-through mode.
    #[inline]
    #[must_use]
    pub const fn is_passthrough_latched(&self) -> bool {
        self.passthrough_latched
    }

    fn reset_stages(&mut self) {
        self.input.reset();
        self.cleanup.reset();
        self.eq.reset();
        self.deesser.reset();
        self.compressor.reset();
        self.limiter.reset();
    }
}
