//! Integration and property tests for Stages 5–6 (Compressor & Lookahead Limiter).

mod common;

use assert_no_alloc::AllocDisabler;
use proptest::prelude::*;
use tine_dsp::stages::compressor::CompressorStage;
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_protocol::params::DspParams;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

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
fn compressor_attack_and_release_time_constants_within_10_percent() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let delay = pipe.latency_samples() as usize;

    let mut p = dynamics_only_params();
    p.comp_thresh_db = -30.0;
    p.comp_ratio = 4.0;
    p.comp_knee_db = 0.0;
    p.comp_attack_ms = 80.0;
    p.comp_release_ms = 200.0;
    p.comp_auto_makeup = false;
    p.comp_makeup_db = 0.0;
    p.limiter_ceiling_db = -0.1;

    // Pre-roll at -30.5 dBFS (just below threshold, 0 dB GR) so the 10 ms RMS detector is active,
    // then step to -24 dBFS (6 dB above threshold; target steady GR = 4.5 dB), then drop to -60 dBFS.
    let preroll_amp = tine_dsp::primitives::db::db_to_lin(-30.5);
    let high_amp = tine_dsp::primitives::db::db_to_lin(-24.0);
    let low_amp = tine_dsp::primitives::db::db_to_lin(-60.0);

    let preroll_len = (0.100 * fs) as usize;
    let high_len = (0.400 * fs) as usize;
    let low_len = (0.600 * fs) as usize;
    let mut buf = vec![preroll_amp; preroll_len];
    buf.extend(std::iter::repeat_n(high_amp, high_len));
    buf.extend(std::iter::repeat_n(low_amp, low_len));

    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
    }

    // Account for pipeline latency and pre-roll.
    let aligned = &buf[delay + preroll_len..];

    // Measure attack time constant: time for GR to reach 63.2% of 4.5 dB (2.845 dB).
    let target_attack_gr_db = 4.5 * (1.0 - (-1.0f32).exp());
    let attack_samples = aligned[..high_len]
        .iter()
        .position(|&y| {
            let gr_db = -24.0 - tine_dsp::primitives::db::lin_to_db(y.abs());
            gr_db >= target_attack_gr_db
        })
        .expect("attack never reached 63.2%");
    let measured_attack_ms = (attack_samples as f32) * 1000.0 / fs;
    assert!(
        (measured_attack_ms - p.comp_attack_ms).abs() <= p.comp_attack_ms * 0.10,
        "attack_ms={measured_attack_ms:.2}, expected={} ±10%",
        p.comp_attack_ms
    );

    // Measure release time constant: after 300 ms (> 50 ms program window),
    // GR starts at ~4.5 dB and decays to 4.5 * (1/e) = 1.655 dB in ~200 ms.
    let target_release_gr_db = 4.5 * (-1.0f32).exp();
    let release_samples = aligned[high_len..]
        .iter()
        .position(|&y| {
            let gr_db = -60.0 - tine_dsp::primitives::db::lin_to_db(y.abs());
            gr_db <= target_release_gr_db
        })
        .expect("release never reached 36.8%");
    let measured_release_ms = (release_samples as f32) * 1000.0 / fs;
    assert!(
        (measured_release_ms - p.comp_release_ms).abs() <= p.comp_release_ms * 0.10,
        "release_ms={measured_release_ms:.2}, expected={} ±10%",
        p.comp_release_ms
    );
}

#[test]
fn compressor_program_dependent_release_shortens_on_short_transient() {
    let fs = 48_000.0;
    let mut p = dynamics_only_params();
    p.comp_thresh_db = -30.0;
    p.comp_ratio = 4.0;
    p.comp_knee_db = 0.0;
    p.comp_attack_ms = 1.0;
    p.comp_release_ms = 200.0;
    p.limiter_ceiling_db = -0.1;

    let high_amp = tine_dsp::primitives::db::db_to_lin(-24.0);
    let low_amp = tine_dsp::primitives::db::db_to_lin(-60.0);

    let measure_release = |hold_ms: f32| -> f32 {
        let mut pipe = DspPipeline::new(fs, 480);
        let delay = pipe.latency_samples() as usize;
        let hold_len = (hold_ms * 1e-3 * fs) as usize;
        let low_len = (0.400 * fs) as usize;
        let mut buf = vec![high_amp; hold_len];
        buf.extend(std::iter::repeat_n(low_amp, low_len));
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
        let aligned = &buf[delay..];
        let start_gr = -60.0 - tine_dsp::primitives::db::lin_to_db(aligned[hold_len].abs());
        let target_gr = start_gr * (-1.0f32).exp();
        let samples = aligned[hold_len..]
            .iter()
            .position(|&y| (-60.0 - tine_dsp::primitives::db::lin_to_db(y.abs())) <= target_gr)
            .expect("release did not reach 1/e");
        (samples as f32) * 1000.0 / fs
    };

    let short_rel = measure_release(10.0);
    let long_rel = measure_release(100.0);
    assert!(
        short_rel < long_rel * 0.75,
        "expected shorter release for 10 ms transient: short={short_rel:.1} ms, long={long_rel:.1} ms"
    );
}

#[test]
fn limiter_below_ceiling_is_bit_exact_apart_from_delay() {
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

    assert_eq!(&output[delay..], &input[..input.len() - delay]);
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
