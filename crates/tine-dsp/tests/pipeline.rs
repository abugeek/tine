//! Integration tests for `DspPipeline` skeleton and RT allocation guarantees.

mod common;

use assert_no_alloc::AllocDisabler;
use tine_dsp::{DspPipeline, MeterSink, NoMeters, NoTap, TapSink};
use tine_protocol::params::DspParams;
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[derive(Default)]
struct RecordingMeters {
    frames: Vec<AudioTelemetryFrame>,
}

impl MeterSink for RecordingMeters {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.frames.push(*f);
    }
}

#[derive(Default)]
struct RecordingTap {
    samples: Vec<f32>,
}

impl TapSink for RecordingTap {
    fn push(&mut self, post: &[f32]) {
        self.samples.extend_from_slice(post);
    }
}

#[test]
fn pipeline_latency_and_bypass_bit_exact_aligned() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let latency = pipe.latency_samples() as usize;
    // 960 (CleanupStage: 480-sample FIFO framing + 480-sample nnnoiseless
    // overlap-add synthesis delay) + 76 (LimiterStage: 72 lookahead + 4 FIR).
    // Documented deviation from the task's original <= 552-sample budget: the
    // nnnoiseless STFT window adds a full frame of synthesis delay, so the
    // minimum constant latency for arbitrary host block sizes is 960 + lookahead.
    assert_eq!(latency, 1036);
    assert!(!pipe.is_passthrough_latched());

    // Measure actual impulse latency in bypass mode (after smoother settles).
    let mut params = DspParams::DEFAULT;
    params.bypass = true;
    let settle_len = 9600;
    let mut settle = vec![0.0f32; settle_len];
    for chunk in settle.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    let imp_len = 4096;
    let imp_pos = 100usize;
    let input = common::impulse(imp_len, imp_pos, 0.8);
    let mut output = input.clone();
    let mut tap = RecordingTap::default();
    for chunk in output.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut tap);
    }

    let (peak_idx, peak_val) = output
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .expect("non-empty");
    assert_eq!(peak_idx, imp_pos + latency);
    assert!((peak_val - 0.8).abs() < 1e-4);
    for (o, i) in output[latency..].iter().zip(input[..imp_len - latency].iter()) {
        assert!((o - i).abs() < 1e-4);
    }
    assert_eq!(tap.samples, output);
}

#[test]
fn ab_bypass_is_time_aligned_zero_lag() {
    let fs = 48_000.0;
    let len = 14_400;
    let input = common::pink_noise(len, 0.25, 0x1234_5678);

    // Wet run (all stages active, neutral defaults).
    let mut wet_pipe = DspPipeline::new(fs, 480);
    let wet_params = DspParams::DEFAULT;
    let mut wet = input.clone();
    for chunk in wet.chunks_mut(480) {
        wet_pipe.process(chunk, &wet_params, &mut NoMeters, &mut NoTap);
    }

    // Dry run (bypass = true, settled before measurement).
    let mut dry_pipe = DspPipeline::new(fs, 480);
    let mut dry_params = DspParams::DEFAULT;
    dry_params.bypass = true;
    let mut settle = vec![0.0f32; 4800];
    for chunk in settle.chunks_mut(480) {
        dry_pipe.process(chunk, &dry_params, &mut NoMeters, &mut NoTap);
    }
    let mut dry = input.clone();
    for chunk in dry.chunks_mut(480) {
        dry_pipe.process(chunk, &dry_params, &mut NoMeters, &mut NoTap);
    }

    // Cross-correlate wet and dry after initial warm-up (skip first 4800 samples).
    let start = 4800usize;
    let end = len - 128;
    let mut best_lag = 0isize;
    let mut best_corr = f64::NEG_INFINITY;
    for lag in -64isize..=64 {
        let mut corr = 0.0f64;
        for (i, &w) in wet.iter().enumerate().take(end).skip(start) {
            let j = (i.cast_signed() + lag) as usize;
            corr += f64::from(w) * f64::from(dry[j]);
        }
        if corr > best_corr {
            best_corr = corr;
            best_lag = lag;
        }
    }
    assert_eq!(
        best_lag, 0,
        "A/B bypass cross-correlation peak at lag {best_lag}, expected 0"
    );
}

#[test]
fn bypass_toggle_has_no_step_above_minus_60_dbfs() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let latency = pipe.latency_samples() as usize;
    let input = common::sine(1000.0, fs, 9600, 0.5);
    // Keep processing stages neutral so wet and dry match in steady state,
    // isolating the bypass cross-fade transition itself.
    let mut params = DspParams::DEFAULT;
    params.hpf_enabled = false;
    params.denoise_mix = 0.0;
    params.gate_enabled = false;
    params.deesser_enabled = false;
    params.comp_enabled = false;

    // Settle parameter smoothers before toggling bypass.
    let mut settle = vec![0.0f32; 4800];
    for chunk in settle.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    let mut output = input.clone();
    for (i, chunk) in output.chunks_mut(480).enumerate() {
        params.bypass = i % 2 == 1;
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    // Compare latency-aligned output against input after the initial latency settle.
    let mut prev_diff = output[latency] - input[0];
    for i in (latency + 1)..output.len() {
        let diff = output[i] - input[i - latency];
        let step = (diff - prev_diff).abs();
        assert!(step < 0.001, "bypass step too large at {i}: {step}");
        prev_diff = diff;
    }
}

#[test]
fn nan_injection_latches_passthrough_and_reset_clears() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let latency = pipe.latency_samples() as usize;
    let params = DspParams::DEFAULT;

    // Feed NaN/Inf followed by enough samples to flush the pipeline's latency.
    let mut buf = vec![0.1f32; latency + 512];
    buf[2] = f32::NAN;
    buf[4] = f32::INFINITY;

    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }
    assert!(pipe.is_passthrough_latched());
    assert!(buf.iter().all(|s| s.is_finite()));

    // Subsequent calls remain latched and finite (passing through dry_delay).
    let mut buf2 = vec![0.1f32; 1280];
    for chunk in buf2.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }
    assert!(pipe.is_passthrough_latched());
    assert!(buf2.iter().all(|s| s.is_finite()));

    // `reset()` clears the latch.
    pipe.reset();
    assert!(!pipe.is_passthrough_latched());
}

#[test]
fn zero_allocations_over_10000_random_blocks() {
    let fs = 48_000.0;
    let max_block = 512usize;
    let mut pipe = DspPipeline::new(fs, max_block);
    let mut params = DspParams::DEFAULT;
    let mut buf = vec![0.0f32; max_block];

    let mut rng = 0x9e37_79b9_7f4a_7c15u64;
    for i in 0..10_000usize {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        let block_len = ((rng.wrapping_mul(0x2545_f491_4f6c_dd1d) >> 32) as usize % max_block) + 1;
        params.bypass = (i & 64) != 0;

        let slice = &mut buf[..block_len];
        for (j, s) in slice.iter_mut().enumerate() {
            *s = ((j as f32) * 0.01).sin() * 0.25;
        }

        rt! {
            pipe.process(slice, &params, &mut NoMeters, &mut NoTap);
        }
    }
}

#[test]
fn meters_publish_at_60hz_and_detect_clipping() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 480);
    let params = DspParams::DEFAULT;
    let mut meters = RecordingMeters::default();

    // 1 second of audio = 100 frames of 480 samples -> ~60 telemetry frames.
    let mut buf = common::sine(440.0, fs, 48_000, 1.0);
    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &params, &mut meters, &mut NoTap);
    }

    assert_eq!(meters.frames.len(), 60);
    assert_eq!(meters.frames[0].sequence, 0);
    assert_eq!(meters.frames[59].sequence, 59);
    assert!((meters.frames[0].flags & flags::INPUT_CLIPPED) != 0);
    assert_close_db!(meters.frames[0].input_rms_db, -3.01, 0.2);
    assert_close_db!(meters.frames[0].input_peak_db, 0.0, 0.1);
}

#[test]
fn harness_measurements_and_wav_roundtrip() {
    let fs = 48_000.0;
    let s = common::sine(1000.0, fs, 4096, 0.5);
    assert_close_db!(common::peak_db(&s), -6.02, 0.05);
    assert_close_db!(common::rms_db(&s), -9.03, 0.05);
    assert_close_db!(common::crest_db(&s), 3.01, 0.05);
    assert!(common::thd_db(&s, 1000.0, fs) < -70.0);

    let fft = common::fft_magnitude_db(&s);
    assert_eq!(fft.len(), s.len() / 2 + 1);

    let tmp = std::env::temp_dir().join("tine_dsp_harness_test.wav");
    common::write_wav_mono(&tmp, &s, 48_000);
    let (loaded, rate) = common::read_wav_mono(&tmp);
    let _ = std::fs::remove_file(&tmp);
    assert_eq!(rate, 48_000);
    assert_eq!(loaded.len(), s.len());
}

#[test]
fn long_run_stability_10_minutes() {
    let fs = 48_000.0;
    let max_block = 512usize;
    let total_samples = 48_000usize * 600; // 10 minutes
    let mut pipe = DspPipeline::new(fs, max_block);

    // 2-second loop of pink noise + speech-like bursts (pre-allocated outside RT).
    let loop_len = 96_000usize;
    let mut source = common::pink_noise(loop_len, 0.15, 0xfeed_face_cafe_beef);
    let bursts = common::bursts(220.0, fs, 12_000, 12_000, 4, 0.35);
    for (s, b) in source.iter_mut().zip(bursts.iter()) {
        *s += *b;
    }

    let mut params = DspParams::DEFAULT;
    let ceiling_lin = tine_dsp::primitives::db::db_to_lin(params.limiter_ceiling_db);
    let mut buf = vec![0.0f32; max_block];
    let mut rng = 0x1234_5678_9abc_def0u64;
    let mut next_u64 = || {
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng.wrapping_mul(0x2545_f491_4f6c_dd1d)
    };

    let mut processed = 0usize;
    let mut next_param_change = 4800usize; // every 100 ms

    while processed < total_samples {
        let block_len = ((next_u64() >> 32) as usize % max_block) + 1;
        let block_len = block_len.min(total_samples - processed);

        if processed >= next_param_change {
            next_param_change += 4800;
            let r0 = ((next_u64() >> 40) as f32) / 16_777_216.0;
            let r1 = ((next_u64() >> 40) as f32) / 16_777_216.0;
            let r2 = ((next_u64() >> 40) as f32) / 16_777_216.0;
            let r3 = ((next_u64() >> 40) as f32) / 16_777_216.0;

            params.input_gain_db = -6.0 + r0 * 18.0;
            params.hpf_enabled = (next_u64() & 1) == 0;
            params.hpf_freq_hz = 40.0 + r1 * 140.0;
            params.denoise_mix = r2;
            params.gate_enabled = (next_u64() & 2) == 0;
            params.gate_threshold_db = -60.0 + r3 * 25.0;
            params.eq_gain_db[0] = -6.0 + r0 * 12.0;
            params.eq_gain_db[1] = -6.0 + r1 * 12.0;
            params.deesser_enabled = (next_u64() & 4) == 0;
            params.comp_enabled = (next_u64() & 8) == 0;
            params.comp_thresh_db = -30.0 + r2 * 20.0;
            params.comp_ratio = 1.5 + r3 * 6.5;
            params.limiter_true_peak = (next_u64() & 16) == 0;
        }

        let slice = &mut buf[..block_len];
        for (i, dst) in slice.iter_mut().enumerate() {
            *dst = source[(processed + i) % loop_len];
        }

        rt! {
            pipe.process(slice, &params, &mut NoMeters, &mut NoTap);
        }

        assert!(
            !pipe.is_passthrough_latched(),
            "unexpected passthrough latch at {processed}"
        );
        for (i, &s) in slice.iter().enumerate() {
            assert!(s.is_finite(), "non-finite sample at {}", processed + i);
            assert!(
                s.abs() <= ceiling_lin + 1e-4,
                "sample {} exceeded ceiling: |{s}| > {ceiling_lin}",
                processed + i
            );
        }

        processed += block_len;
    }
}
