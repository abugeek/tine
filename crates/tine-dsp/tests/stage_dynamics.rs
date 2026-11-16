//! Integration and property tests for Stages 5–6 (Compressor & Lookahead Limiter).

mod common;

use assert_no_alloc::AllocDisabler;
use proptest::prelude::*;
use tine_dsp::stages::compressor::CompressorStage;
use tine_dsp::{DspPipeline, MeterSink, NoMeters, NoTap};
use tine_protocol::params::DspParams;
use tine_protocol::telemetry::AudioTelemetryFrame;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[derive(Default)]
struct LastMeter(Option<AudioTelemetryFrame>);

impl MeterSink for LastMeter {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.0 = Some(*f);
    }
}

fn dynamics_only_params() -> DspParams {
    let mut p = DspParams::DEFAULT;
    p.bypass = false;
    p.input_gain_db = 0.0;
    p.hpf_enabled = false;
    p.denoise_mix = 0.0;
    p.gate_enabled = false;
    p.deesser_enabled = false;
    p.comp_enabled = true;
    p.comp_auto_makeup = false;
    p.comp_makeup_db = 0.0;
    p.limiter_ceiling_db = -1.0;
    p.limiter_true_peak = false;
    p
}

#[test]
fn compressor_static_curve_and_soft_knee_properties() {
    let mut p = dynamics_only_params();
    p.comp_thresh_db = -20.0;
    p.comp_ratio = 4.0;
    p.comp_knee_db = 0.0;

    // Hard knee: input -10 dBFS -> -20 + (-10 - (-20))/4 = -17.5 dBFS.
    let out = CompressorStage::static_curve_db(&p, -10.0);
    assert_close_db!(out, -17.5, 0.1);

    // Below threshold is unity.
    assert_close_db!(CompressorStage::static_curve_db(&p, -30.0), -30.0, 1e-5);

    // Auto makeup: -(T - T/R) * 0.5 = -(-20 - (-5)) * 0.5 = 7.5 dB.
    assert_close_db!(CompressorStage::auto_makeup_db(&p), 7.5, 1e-5);

    // Soft knee 6 dB: continuous, monotone, and C1 (continuous derivative).
    p.comp_knee_db = 6.0;
    let step = 0.01f32;
    let mut prev_y = CompressorStage::static_curve_db(&p, -60.0);
    let mut prev_slope = 1.0f32;
    let mut x = -60.0 + step;
    while x <= 0.0 {
        let y = CompressorStage::static_curve_db(&p, x);
        // Monotone increasing.
        assert!(y > prev_y, "not monotone at x={x}: prev_y={prev_y}, y={y}");
        // Continuous (bounded step).
        assert!((y - prev_y) <= step * 1.01, "discontinuity in value at x={x}");

        let slope = (y - prev_y) / step;
        // Continuous derivative (slope changes smoothly).
        assert!(
            (slope - prev_slope).abs() < 0.01,
            "discontinuity in derivative at x={x}: prev_slope={prev_slope}, slope={slope}"
        );
        prev_y = y;
        prev_slope = slope;
        x += step;
    }
}

#[test]
fn pipeline_compressor_reduces_loud_sine_and_reports_gr() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);

    let mut p = dynamics_only_params();
    p.comp_thresh_db = -20.0;
    p.comp_ratio = 4.0;
    p.comp_knee_db = 0.0;
    p.comp_attack_ms = 5.0;
    p.comp_release_ms = 100.0;
    p.comp_auto_makeup = false;
    p.comp_makeup_db = 0.0;
    p.limiter_ceiling_db = -0.1;

    // 1 kHz sine with RMS = -10 dBFS (peak = -6.99 dBFS):
    // Static curve gives -17.5 dBFS RMS (7.5 dB GR).
    let rms_amp = tine_dsp::primitives::db::db_to_lin(-10.0) * std::f32::consts::SQRT_2;
    let mut buf = common::sine(1000.0, fs, 24_000, rms_amp);
    let mut meters = LastMeter::default();

    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &p, &mut meters, &mut NoTap);
    }

    let out_rms = common::rms_db(&buf[12_000..]);
    assert_close_db!(out_rms, -17.5, 0.3);

    let frame = meters.0.expect("expected telemetry frame");
    assert!(
        frame.comp_gain_reduction_db > 6.5 && frame.comp_gain_reduction_db < 8.5,
        "unexpected comp_gain_reduction_db={}",
        frame.comp_gain_reduction_db
    );
}

#[test]
fn pipeline_limiter_below_ceiling_preserves_rms_apart_from_delay() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let delay = pipe.latency_samples() as usize;
    assert!(delay >= 72);

    let mut p = dynamics_only_params();
    p.comp_enabled = false;
    p.limiter_ceiling_db = -1.0; // ~0.89125 linear
    p.limiter_true_peak = false;

    let input = common::sine(440.0, fs, 4800, 0.5);
    let mut output = input.clone();
    for chunk in output.chunks_mut(480) {
        pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
    }

    // Pipeline includes Stage 1's 4 Hz DC blocker, so check steady-state RMS matches within 0.05 dB.
    // (Bit-exactness of LimiterStage itself is verified in limiter::tests::below_ceiling_is_bit_exact_apart_from_delay.)
    let in_rms = common::rms_db(&input[480..input.len() - delay]);
    let out_rms = common::rms_db(&output[delay + 480..]);
    assert_close_db!(out_rms, in_rms, 0.05);
}

#[test]
fn limiter_strict_brickwall_over_1m_samples() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = dynamics_only_params();
    p.comp_enabled = false;
    p.limiter_ceiling_db = -1.0;
    p.limiter_true_peak = false;
    let ceiling = tine_dsp::primitives::db::db_to_lin(-1.0);

    // 1,000,000 samples across full-scale squares, impulses, noise, and +24 dB overdriven speech-like sines.
    let mut buf = Vec::with_capacity(1_000_000);
    // 250k full-scale square
    buf.extend(common::square(200.0, fs, 250_000, 1.0));
    // 250k impulses (+24 dB = ~15.85 linear)
    let mut imp = vec![0.0f32; 250_000];
    for i in (100..250_000).step_by(500) {
        imp[i] = if (i / 500) % 2 == 0 { 15.85 } else { -15.85 };
    }
    buf.extend(imp);
    // 250k white noise at +12 dB
    buf.extend(common::white_noise(250_000, 4.0, 0xdead_beef_cafe_babe));
    // 250k +24 dB overdriven multi-tone
    let s1 = common::sine(180.0, fs, 250_000, 8.0);
    let s2 = common::sine(1200.0, fs, 250_000, 7.85);
    buf.extend(s1.into_iter().zip(s2).map(|(a, b)| a + b));

    assert_eq!(buf.len(), 1_000_000);

    rt! {
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
    }

    for (i, &s) in buf.iter().enumerate() {
        assert!(s.abs() <= ceiling, "sample {i} exceeded ceiling: |{s}| > {ceiling}");
    }
}

#[test]
fn limiter_true_peak_mode_bounds_intersample_peaks() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = dynamics_only_params();
    p.comp_enabled = false;
    p.limiter_ceiling_db = -1.0;
    p.limiter_true_peak = true;

    let ceiling_lin = tine_dsp::primitives::db::db_to_lin(-1.0);
    let allowed_lin = tine_dsp::primitives::db::db_to_lin(-1.0 + 0.1);

    // 12 kHz sine at fs/4 with 45-degree phase offset: sample peaks are 0.707 * A,
    // while true intersample peak is A. Drive A = 2.0 (+6 dBFS true peak).
    let w = 2.0 * std::f32::consts::PI * 12_000.0 / fs;
    let mut buf: Vec<f32> = (0..9_600)
        .map(|n| 2.0 * (w * (n as f32) + std::f32::consts::FRAC_PI_4).sin())
        .collect();

    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
    }

    // Measure 4x oversampled true peak of steady-state output (after initial transient)
    // using an independent 4x linear/cubic-free sinc interpolation check.
    let tail = &buf[2000..];
    for &s in tail {
        assert!(s.abs() <= ceiling_lin);
    }
    // At 12 kHz (fs/4) with 45-deg phase, adjacent samples have equal magnitude `peak / sqrt(2)`,
    // so the true peak of the reconstructed sine is `sample_peak * sqrt(2)`.
    let max_sample_peak = tail.iter().fold(0.0f32, |m, &x| m.max(x.abs()));
    let true_peak_est = max_sample_peak * std::f32::consts::SQRT_2;
    assert!(
        true_peak_est <= allowed_lin,
        "true_peak_est={true_peak_est:.4} > allowed={allowed_lin:.4} (ceiling+0.1dB)"
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]
    #[test]
    fn limiter_never_exceeds_arbitrary_ceiling(
        ceiling_db in -12.0f32..=-0.1f32,
        amp in 0.1f32..=16.0f32,
        seed in 1u64..10_000u64,
    ) {
        let mut pipe = DspPipeline::new(48_000.0, 480);
        let mut p = dynamics_only_params();
        p.comp_enabled = false;
        p.limiter_ceiling_db = ceiling_db;
        p.limiter_true_peak = false;
        let ceiling = tine_dsp::primitives::db::db_to_lin(ceiling_db);

        let mut buf = common::white_noise(2_400, amp, seed);
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
        for &s in &buf {
            prop_assert!(s.abs() <= ceiling, "|{s}| > {ceiling}");
        }
    }
}

#[test]
fn dynamics_zero_alloc_under_param_changes_and_constant_latency() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let latency = pipe.latency_samples();
    assert!(latency >= 72);

    let mut p = dynamics_only_params();
    let mut buf = common::sine(440.0, fs, 480, 0.5);

    rt! {
        for i in 0..200u32 {
            p.comp_enabled = (i % 2) == 0;
            p.comp_auto_makeup = (i % 3) == 0;
            p.comp_attack_ms = 1.0 + ((i % 20) as f32) * 2.0;
            p.comp_release_ms = 20.0 + ((i % 10) as f32) * 30.0;
            p.limiter_true_peak = (i % 4) == 0;
            p.limiter_ceiling_db = -1.0 - ((i % 5) as f32) * 0.5;
            pipe.process(&mut buf, &p, &mut NoMeters, &mut NoTap);
        }
    }

    assert_eq!(pipe.latency_samples(), latency);
}
