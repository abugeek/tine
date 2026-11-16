//! Real-time safety (`assert_no_alloc`), clock-drift, and resampling tests for `tine_engine::audio_io`.

use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use cpal::{Data, SampleFormat};
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_engine::audio_io::{DeviceRef, list_inputs, list_outputs, test_capture_processor, test_playback_processor};
use tine_protocol::params::DspParams;
use tine_protocol::sync::SpscRing;

#[global_allocator]
static A: AllocDisabler = AllocDisabler;

#[test]
fn capture_and_playback_callbacks_are_zero_alloc() {
    let ring = Arc::new(SpscRing::<32_768>::new());
    let mut pipeline = DspPipeline::new(48_000.0, 1024);
    let params = DspParams::DEFAULT;

    let mut capture = test_capture_processor(2, 512, Arc::clone(&ring), move |block| {
        pipeline.process(block, &params, &mut NoMeters, &mut NoTap);
    });

    let mut playback = test_playback_processor(2, 44_100, 48_000, 1024, Arc::clone(&ring));

    // Deliberately pass a larger buffer (1200 stereo frames > 512 max_frames) to test chunking!
    let mut in_samples: Vec<i16> = (0..2400).map(|i| ((i % 200) as i16) * 50).collect();
    // SAFETY: `in_samples` is valid for `in_samples.len()` `i16` elements and only read via `Data::as_slice::<i16>()`.
    let in_data = unsafe {
        Data::from_parts(
            in_samples.as_mut_ptr().cast::<()>(),
            in_samples.len(),
            SampleFormat::I16,
        )
    };

    let mut out_samples = vec![0.0f32; 960 * 2];
    // SAFETY: `out_samples` is uniquely borrowed and valid for `out_samples.len()` `f32` elements.
    let mut out_data = unsafe {
        Data::from_parts(
            out_samples.as_mut_ptr().cast::<()>(),
            out_samples.len(),
            SampleFormat::F32,
        )
    };

    assert_no_alloc(|| {
        for _ in 0..20 {
            capture.process_data(&in_data);
            playback.process_data(&mut out_data);
        }
    });
}

#[test]
fn ten_minute_44100_to_48000_with_clock_drift_has_zero_glitches() {
    let ring = Arc::new(SpscRing::<32_768>::new());

    // Simulate 10 minutes = 600s of 44.1 kHz capture -> 48 kHz playback
    // with +300 ppm (+0.03%) capture clock drift relative to playback.
    let mut capture = test_capture_processor(1, 1024, Arc::clone(&ring), |_block| {});
    let mut playback = test_playback_processor(2, 44_100, 48_000, 1024, Arc::clone(&ring));

    let in_block_frames = 441usize;
    let out_block_frames = 480usize;

    let mut in_buf = vec![0.0f32; in_block_frames];
    let mut out_buf = vec![0.0f32; out_block_frames * 2];

    let mut phase = 0.0f64;
    let phase_inc = 2.0 * std::f64::consts::PI * 440.0 / 44_100.0;

    // Simulate +300 ppm clock drift: capture runs slightly faster (every ~3333 callbacks, an extra 10ms).
    let mut drift_acc = 0.0f64;
    let mut underruns = 0u64;
    let mut max_step_diff = 0.0f32;
    let mut prev_out = 0.0f32;
    let mut primed_seen = false;

    for cb in 0..60_000u64 {
        for s in &mut in_buf {
            *s = (phase.sin() * 0.5) as f32;
            phase += phase_inc;
        }
        // SAFETY: `in_buf` is valid for `in_buf.len()` `f32` elements and only read via `Data::as_slice::<f32>()`.
        let in_data = unsafe { Data::from_parts(in_buf.as_mut_ptr().cast::<()>(), in_buf.len(), SampleFormat::F32) };
        capture.process_data(&in_data);

        // Occasional extra capture block to model +300 ppm clock drift.
        drift_acc += 0.0003;
        if drift_acc >= 1.0 {
            drift_acc -= 1.0;
            for s in &mut in_buf {
                *s = (phase.sin() * 0.5) as f32;
                phase += phase_inc;
            }
            // SAFETY: `in_buf` is valid for `in_buf.len()` `f32` elements and only read via `Data::as_slice::<f32>()`.
            let extra = unsafe { Data::from_parts(in_buf.as_mut_ptr().cast::<()>(), in_buf.len(), SampleFormat::F32) };
            capture.process_data(&extra);
        }

        // SAFETY: `out_buf` is uniquely borrowed and valid for `out_buf.len()` `f32` elements.
        let mut out_data =
            unsafe { Data::from_parts(out_buf.as_mut_ptr().cast::<()>(), out_buf.len(), SampleFormat::F32) };
        playback.process_data(&mut out_data);

        // Once signal appears, check continuity of 440 Hz sine wave (no zero-dropouts/glitches).
        let energy: f32 = out_buf.iter().step_by(2).map(|x| x.abs()).sum();
        if energy > 1.0 {
            primed_seen = true;
        }
        if primed_seen && cb > 5 {
            if energy < 0.01 {
                underruns += 1;
            }
            for &s in out_buf.iter().step_by(2) {
                let diff = (s - prev_out).abs();
                if diff > max_step_diff {
                    max_step_diff = diff;
                }
                prev_out = s;
            }
        }
    }

    assert!(primed_seen, "playback never produced audio");
    assert_eq!(
        underruns, 0,
        "unexpected underrun callbacks in 10-minute drift simulation"
    );
    assert_eq!(ring.overruns(), 0, "unexpected capture ring overruns");
    // A 440 Hz sine at 48 kHz with amplitude 0.5 has max sample-to-sample delta ~ 0.029.
    assert!(
        max_step_diff < 0.08,
        "sine wave discontinuity detected: max_step_diff={max_step_diff}"
    );
}

#[test]
fn device_enumeration_and_parse_smoke() {
    let _inputs = list_inputs().expect("list_inputs should succeed");
    let _outputs = list_outputs().expect("list_outputs should succeed");

    assert_eq!(DeviceRef::parse("default"), DeviceRef::Default);
    assert_eq!(
        DeviceRef::parse("wasapi:{0.0.1.00000000}.{1234}"),
        DeviceRef::Id("wasapi:{0.0.1.00000000}.{1234}".to_owned())
    );
    assert_eq!(DeviceRef::parse("Microphone"), DeviceRef::Name("Microphone".to_owned()));
}
