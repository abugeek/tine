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
fn skeleton_is_bit_exact_passthrough() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    assert_eq!(pipe.latency_samples(), 0);
    assert!(!pipe.is_passthrough_latched());

    let input = common::sine(440.0, fs, 2048, 0.5);
    let mut output = input.clone();
    let params = DspParams::DEFAULT;
    let mut tap = RecordingTap::default();

    for chunk in output.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut tap);
    }

    assert_eq!(output, input);
    assert_eq!(tap.samples, input);
}

#[test]
fn bypass_toggle_has_no_step_above_minus_60_dbfs() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let input = common::sine(1000.0, fs, 9600, 0.5);
    let mut output = input.clone();
    let mut params = DspParams::DEFAULT;

    for (i, chunk) in output.chunks_mut(480).enumerate() {
        params.bypass = i % 2 == 1;
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    // Compare output against input: step between adjacent samples of (output - input)
    // must never exceed -60 dBFS (0.001 linear).
    let mut prev_diff = 0.0f32;
    for (&o, &x) in output.iter().zip(input.iter()) {
        let diff = o - x;
        let step = (diff - prev_diff).abs();
        assert!(step < 0.001, "bypass step too large: {step}");
        prev_diff = diff;
    }
}

#[test]
fn nan_injection_latches_passthrough_and_reset_clears() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let params = DspParams::DEFAULT;
    let mut buf = [0.25f32, 0.5, f32::NAN, -0.25, f32::INFINITY, 0.1];

    pipe.process(&mut buf, &params, &mut NoMeters, &mut NoTap);
    assert!(pipe.is_passthrough_latched());
    assert!(buf.iter().all(|s| s.is_finite()));

    // Subsequent calls remain latched and finite.
    let mut buf2 = [0.1f32; 64];
    pipe.process(&mut buf2, &params, &mut NoMeters, &mut NoTap);
    assert!(pipe.is_passthrough_latched());
    assert_eq!(buf2, [0.1f32; 64]);

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
