//! 7-stage real-time `DspPipeline`.

use alloc::boxed::Box;
use alloc::vec;

use tine_protocol::params::{DenoiseEngine, DspParams, ParamId};
use tine_protocol::telemetry::flags;

#[cfg(feature = "deepfilter")]
use crate::denoise::DeepFilterDenoiser;
use crate::denoise::{BypassDenoiser, Denoiser, RnnoiseDenoiser};
use crate::meter::{MeterSink, MeterStage, StageMeters, TapSink};
use crate::primitives::db::db_to_lin;
use crate::primitives::delay::DelayLine;
use crate::primitives::smoother::Smoother;
use crate::rt::DenormalGuard;
use crate::stages::{CleanupStage, CompressorStage, DeesserStage, EqStage, InputStage, LimiterStage, Stage};

/// Consecutive finite input samples required to auto-recover from `PASSTHROUGH_LATCHED`.
const RECOVERY_CLEAN_SAMPLES: usize = 48_000;

/// Real-time 7-stage microphone DSP pipeline.
///
/// All heap memory is allocated once in [`DspPipeline::new`]; [`DspPipeline::process`]
/// is lock-free, allocation-free, and panic-free.
pub struct DspPipeline {
    max_block: usize,
    latency: u32,
    passthrough_latched: bool,
    recovery_clean_samples: usize,
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
    dry_gain_smoother: Smoother,
}

impl DspPipeline {
    /// Constructs and prepares a new [`DspPipeline`] with the default [`RnnoiseDenoiser`]
    /// for `sample_rate` and chunks up to `max_block`.
    ///
    /// All heap allocations happen here.
    #[must_use]
    pub fn new(sample_rate: f32, max_block: usize) -> Self {
        Self::for_engine(sample_rate, max_block, DenoiseEngine::Rnnoise)
    }

    /// Constructs and prepares a new [`DspPipeline`] using the selected [`DenoiseEngine`].
    ///
    /// Falls back to [`RnnoiseDenoiser`] if [`DenoiseEngine::DeepFilterNet3`] is requested
    /// without the `deepfilter` crate feature enabled.
    #[must_use]
    pub fn for_engine(sample_rate: f32, max_block: usize, engine: DenoiseEngine) -> Self {
        let denoiser: Box<dyn Denoiser> = match engine {
            DenoiseEngine::Rnnoise => Box::new(RnnoiseDenoiser::new()),
            DenoiseEngine::DeepFilterNet3 => {
                #[cfg(feature = "deepfilter")]
                {
                    Box::new(DeepFilterDenoiser::new())
                }
                #[cfg(not(feature = "deepfilter"))]
                {
                    Box::new(RnnoiseDenoiser::new())
                }
            }
            DenoiseEngine::Off => Box::new(BypassDenoiser),
        };
        Self::with_denoiser(sample_rate, max_block, denoiser)
    }

    /// Constructs and prepares a new [`DspPipeline`] with a custom [`Denoiser`]
    /// for `sample_rate` and chunks up to `max_block`.
    ///
    /// All heap allocations happen here.
    #[must_use]
    pub fn with_denoiser(sample_rate: f32, max_block: usize, denoiser: Box<dyn Denoiser>) -> Self {
        let max_block = max_block.max(1);
        let fs = if sample_rate.is_finite() && sample_rate > 0.0 {
            sample_rate.clamp(8_000.0, 192_000.0)
        } else {
            48_000.0
        };

        let mut input = InputStage::new();
        let mut cleanup = CleanupStage::with_denoiser(denoiser);
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
            recovery_clean_samples: 0,
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
            dry_gain_smoother: Smoother::new(10.0, fs, 1.0),
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
        if buf.is_empty() {
            return;
        }
        let _denormal_guard = DenormalGuard::new();

        let delay = self.latency as usize;
        self.bypass_smoother.set_target(if params.bypass { 1.0 } else { 0.0 });
        let gain_spec = ParamId::InputGainDb.spec();
        let gain_db = if params.input_gain_db.is_finite() {
            params.input_gain_db.clamp(gain_spec.min, gain_spec.max)
        } else {
            gain_spec.default
        };
        self.dry_gain_smoother.set_target(db_to_lin(gain_db));

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
                    if self.passthrough_latched {
                        self.recovery_clean_samples = self.recovery_clean_samples.saturating_add(1);
                    }
                    x
                } else {
                    input_non_finite = true;
                    self.recovery_clean_samples = 0;
                    0.0
                };
                *dry_slot = self.dry_delay.push_read(safe_x, delay);
            }

            if input_non_finite && !self.passthrough_latched {
                self.reset_stages();
                self.passthrough_latched = true;
                self.recovery_clean_samples = 0;
            } else if self.passthrough_latched && self.recovery_clean_samples >= RECOVERY_CLEAN_SAMPLES {
                self.reset_stages();
                self.passthrough_latched = false;
                self.recovery_clean_samples = 0;
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
                    self.recovery_clean_samples = 0;
                    if let Some(dry) = self.dry_buf.get(..n) {
                        chunk.copy_from_slice(dry);
                    }
                } else {
                    for (wet, &dry) in chunk.iter_mut().zip(self.dry_buf.iter().take(n)) {
                        let g = self.bypass_smoother.next();
                        let dry_gain = self.dry_gain_smoother.next();
                        *wet = *wet * (1.0 - g) + (dry * dry_gain) * g;
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
        self.dry_gain_smoother.snap_to(1.0);
        self.passthrough_latched = false;
        self.recovery_clean_samples = 0;
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
