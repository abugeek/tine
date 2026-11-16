//! Real-time allocation and functional tests for `tine_engine::virtual_cable`.

use std::f32::consts::TAU;
use std::sync::Arc;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use tine_engine::virtual_cable::VirtualCableProcessor;
use tine_protocol::params::DspParams;
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn virtual_cable_processor_zero_alloc_and_telemetry_tap() {
    let params = Arc::new(ParamBlock::new());
    let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
    let tap = Arc::new(SpscRing::<65_536>::new());

    let mut proc = VirtualCableProcessor::new(
        48_000.0,
        4096,
        Arc::clone(&params),
        Arc::clone(&telemetry),
        Arc::clone(&tap),
    );

    let mut block = [0.0_f32; 480];
    let mut phase = 0.0_f32;
    let phase_inc = TAU * 1000.0 / 48_000.0;

    // Warm up / run 100 blocks (1.0 s @ 48 kHz) under assert_no_alloc.
    for _ in 0..100 {
        for s in &mut block {
            *s = 0.25 * phase.sin();
            phase = (phase + phase_inc) % TAU;
        }
        assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
        assert!(block.iter().all(|s| s.is_finite()), "non-finite DSP output");
    }

    let mut frame = AudioTelemetryFrame::default();
    assert!(telemetry.latest(&mut frame), "expected ~60 Hz telemetry frame");
    assert!(
        frame.sequence >= 40,
        "expected >= 40 frames in 1s, got {}",
        frame.sequence
    );
    assert!(
        frame.input_rms_db > -30.0,
        "unexpected low input RMS: {}",
        frame.input_rms_db
    );
    assert!(
        frame.output_rms_db > -60.0,
        "unexpected low output RMS: {}",
        frame.output_rms_db
    );

    let mut drained = [0.0_f32; 4096];
    let n = tap.pop_slice(&mut drained);
    assert!(n > 0, "expected post-DSP samples in tap ring");
}

#[test]
fn virtual_cable_param_updates_and_bypass_zero_alloc() {
    let params = Arc::new(ParamBlock::new());
    let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
    let tap = Arc::new(SpscRing::<16_384>::new());

    let mut proc = VirtualCableProcessor::new(
        48_000.0,
        4096,
        Arc::clone(&params),
        Arc::clone(&telemetry),
        Arc::clone(&tap),
    );

    let mut block = [0.0_f32; 480];
    let mut phase = 0.0_f32;
    let phase_inc = TAU * 440.0 / 48_000.0;

    // Run 3 blocks with bypass enabled so smoother settles and telemetry publishes.
    let mut p = DspParams {
        bypass: true,
        ..DspParams::DEFAULT
    };
    params.store(&p);

    for _ in 0..3 {
        for s in &mut block {
            *s = 0.2 * phase.sin();
            phase = (phase + phase_inc) % TAU;
        }
        assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
    }

    let mut frame = AudioTelemetryFrame::default();
    assert!(telemetry.latest(&mut frame));
    assert_ne!(frame.flags & flags::BYPASS, 0, "expected BYPASS flag in telemetry");

    // Disable bypass and measure baseline output RMS.
    p.bypass = false;
    params.store(&p);
    for _ in 0..5 {
        for s in &mut block {
            *s = 0.05 * phase.sin();
            phase = (phase + phase_inc) % TAU;
        }
        assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
    }
    assert!(telemetry.latest(&mut frame));
    assert_eq!(frame.flags & flags::BYPASS, 0, "expected BYPASS flag cleared");
    let baseline_rms_db = frame.output_rms_db;

    // Boost input gain by +12 dB and verify audible increase within < 50 ms (4 blocks = 40 ms).
    p.input_gain_db = 12.0;
    params.store(&p);
    for _ in 0..4 {
        for s in &mut block {
            *s = 0.05 * phase.sin();
            phase = (phase + phase_inc) % TAU;
        }
        assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
    }
    assert!(telemetry.latest(&mut frame));
    assert!(
        frame.output_rms_db > baseline_rms_db + 3.0,
        "expected audible gain increase within 40 ms: baseline={baseline_rms_db}, boosted={}",
        frame.output_rms_db
    );
}
