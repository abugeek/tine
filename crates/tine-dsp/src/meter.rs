//! Stage 7: RMS/peak metering, telemetry publishing (~60 Hz), and post-DSP audio tap.

use tine_protocol::telemetry::{AudioTelemetryFrame, flags};

use crate::primitives::db::lin_to_db;
use crate::primitives::math::sqrt_f64;

/// Sink receiving ~60 Hz telemetry frames from [`crate::DspPipeline::process`].
pub trait MeterSink {
    /// Publishes the latest [`AudioTelemetryFrame`].
    fn publish(&mut self, f: &AudioTelemetryFrame);
}

/// Sink receiving post-limiter mono `f32` audio samples.
pub trait TapSink {
    /// Pushes a block of post-limiter samples.
    fn push(&mut self, post: &[f32]);
}

/// Zero-cost [`MeterSink`] that discards telemetry frames.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoMeters;

impl MeterSink for NoMeters {
    #[inline]
    fn publish(&mut self, _f: &AudioTelemetryFrame) {}
}

/// Zero-cost [`TapSink`] that discards post-DSP samples.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoTap;

impl TapSink for NoTap {
    #[inline]
    fn push(&mut self, _post: &[f32]) {}
}

/// Per-block gain reduction values collected from Stages 2, 4, and 5.
#[derive(Clone, Copy, Debug, Default)]
#[allow(clippy::struct_field_names)]
pub(crate) struct StageMeters {
    pub gate_attenuation_db: f32,
    pub comp_gain_reduction_db: f32,
    pub deesser_reduction_db: f32,
}

/// Stage 7 metering accumulator and ~60 Hz telemetry publisher.
#[derive(Clone, Debug)]
pub(crate) struct MeterStage {
    publish_interval: usize,
    sample_count: usize,
    in_sum_sq: f64,
    in_peak: f32,
    in_clipped: bool,
    out_sum_sq: f64,
    out_peak: f32,
    gate_gr_db: f32,
    comp_gr_db: f32,
    deesser_gr_db: f32,
    sequence: u64,
}

impl MeterStage {
    pub fn new(sample_rate: f32) -> Self {
        let interval = if sample_rate.is_finite() && sample_rate > 0.0 {
            ((sample_rate / 60.0) + 0.5) as usize
        } else {
            800
        }
        .max(1);
        Self {
            publish_interval: interval,
            sample_count: 0,
            in_sum_sq: 0.0,
            in_peak: 0.0,
            in_clipped: false,
            out_sum_sq: 0.0,
            out_peak: 0.0,
            gate_gr_db: 0.0,
            comp_gr_db: 0.0,
            deesser_gr_db: 0.0,
            sequence: 0,
        }
    }

    pub fn reset(&mut self) {
        self.sample_count = 0;
        self.in_sum_sq = 0.0;
        self.in_peak = 0.0;
        self.in_clipped = false;
        self.out_sum_sq = 0.0;
        self.out_peak = 0.0;
        self.gate_gr_db = 0.0;
        self.comp_gr_db = 0.0;
        self.deesser_gr_db = 0.0;
    }

    pub fn accumulate_and_publish(
        &mut self,
        input: &[f32],
        output: &[f32],
        stage_meters: &StageMeters,
        base_flags: u32,
        meters: &mut impl MeterSink,
    ) {
        if stage_meters.gate_attenuation_db.abs() > self.gate_gr_db.abs() {
            self.gate_gr_db = stage_meters.gate_attenuation_db;
        }
        if stage_meters.comp_gain_reduction_db.abs() > self.comp_gr_db.abs() {
            self.comp_gr_db = stage_meters.comp_gain_reduction_db;
        }
        if stage_meters.deesser_reduction_db.abs() > self.deesser_gr_db.abs() {
            self.deesser_gr_db = stage_meters.deesser_reduction_db;
        }

        let len = input.len().min(output.len());
        let mut offset = 0usize;

        while offset < len {
            let remaining_in_window = self.publish_interval.saturating_sub(self.sample_count).max(1);
            let take = remaining_in_window.min(len - offset);
            let end = offset + take;

            if let (Some(in_slice), Some(out_slice)) = (input.get(offset..end), output.get(offset..end)) {
                for &x in in_slice {
                    let ax = if x.is_finite() { x.abs() } else { 0.0 };
                    if ax >= 1.0 {
                        self.in_clipped = true;
                    }
                    if ax > self.in_peak {
                        self.in_peak = ax;
                    }
                    let xd = f64::from(ax);
                    self.in_sum_sq += xd * xd;
                }

                for &y in out_slice {
                    let ay = if y.is_finite() { y.abs() } else { 0.0 };
                    if ay > self.out_peak {
                        self.out_peak = ay;
                    }
                    let yd = f64::from(ay);
                    self.out_sum_sq += yd * yd;
                }
            }

            self.sample_count += take;
            offset = end;

            if self.sample_count >= self.publish_interval {
                let count = self.sample_count.max(1) as f64;
                let in_rms = sqrt_f64(self.in_sum_sq / count) as f32;
                let out_rms = sqrt_f64(self.out_sum_sq / count) as f32;
                let frame_flags = base_flags | if self.in_clipped { flags::INPUT_CLIPPED } else { 0 };

                let frame = AudioTelemetryFrame {
                    sequence: self.sequence,
                    flags: frame_flags,
                    input_rms_db: lin_to_db(in_rms),
                    input_peak_db: lin_to_db(self.in_peak),
                    output_rms_db: lin_to_db(out_rms),
                    output_peak_db: lin_to_db(self.out_peak),
                    gate_attenuation_db: self.gate_gr_db,
                    comp_gain_reduction_db: self.comp_gr_db,
                    deesser_reduction_db: self.deesser_gr_db,
                };
                meters.publish(&frame);
                self.sequence = self.sequence.wrapping_add(1);
                self.reset();
            }
        }
    }
}
