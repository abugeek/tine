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

fn measure_tone_rms(proc: &mut VirtualCableProcessor<65_536>, freq_hz: f32, amp: f32, blocks: usize) -> (f32, f32) {
    let mut block = [0.0_f32; 480];
    let mut phase = 0.0_f32;
    let phase_inc = TAU * freq_hz / 48_000.0;
    let mut sq_sum = 0.0_f64;
    let mut peak = 0.0_f32;
    let mut count = 0usize;
    let warmup = blocks / 2;

    for b in 0..blocks {
        for s in &mut block {
            *s = amp * phase.sin();
            phase = (phase + phase_inc) % TAU;
        }
        proc.process_block(&mut block);
        if b >= warmup {
            for &s in &block {
                let a = s.abs();
                if a > peak {
                    peak = a;
                }
                sq_sum += f64::from(s) * f64::from(s);
                count += 1;
            }
        }
    }
    let rms = (sq_sum / count.max(1) as f64).sqrt() as f32;
    (rms, peak)
}

#[test]
fn virtual_cable_sliders_eq_dynamics_measurably_shape_audio() {
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

    // 1. EQ low-shelf boost (+9 dB @ 250 Hz) vs cut (-9 dB @ 250 Hz) on 160 Hz tone.
    let mut eq_boost = DspParams {
        denoise_mix: 0.0,
        gate_enabled: false,
        comp_enabled: false,
        deesser_enabled: false,
        hpf_enabled: false,
        ..DspParams::DEFAULT
    };
    eq_boost.eq_freq_hz[0] = 250.0;
    eq_boost.eq_gain_db[0] = 9.0;
    params.store(&eq_boost);
    let (rms_boost, _) = measure_tone_rms(&mut proc, 160.0, 0.1, 40);

    let mut eq_cut = eq_boost;
    eq_cut.eq_gain_db[0] = -9.0;
    params.store(&eq_cut);
    let (rms_cut, _) = measure_tone_rms(&mut proc, 160.0, 0.1, 40);
    assert!(
        rms_boost > rms_cut * 3.0,
        "EQ low-shelf boost ({rms_boost}) must significantly exceed cut ({rms_cut})"
    );

    // 2. Compressor (thresh -30 dBFS, ratio 6:1, no auto-makeup) reduces -6 dBFS tone.
    let comp_off = DspParams {
        denoise_mix: 0.0,
        gate_enabled: false,
        deesser_enabled: false,
        hpf_enabled: false,
        comp_enabled: false,
        ..DspParams::DEFAULT
    };
    params.store(&comp_off);
    let (rms_uncomp, _) = measure_tone_rms(&mut proc, 1000.0, 0.5, 40);

    let comp_on = DspParams {
        comp_enabled: true,
        comp_thresh_db: -30.0,
        comp_ratio: 6.0,
        comp_auto_makeup: false,
        comp_makeup_db: 0.0,
        ..comp_off
    };
    params.store(&comp_on);
    let (rms_comp, _) = measure_tone_rms(&mut proc, 1000.0, 0.5, 40);
    assert!(
        rms_comp < rms_uncomp * 0.5,
        "compressor must reduce loud signal ({rms_comp} < 0.5 * {rms_uncomp})"
    );

    // 3. Gate (thresh -20 dBFS, ratio 6:1) attenuates a -40 dBFS floor.
    let gate_on = DspParams {
        gate_enabled: true,
        gate_threshold_db: -20.0,
        gate_ratio: 6.0,
        ..comp_off
    };
    params.store(&gate_on);
    let (rms_gated, _) = measure_tone_rms(&mut proc, 1000.0, 0.01, 60);
    assert!(
        rms_gated < 0.004,
        "gate must attenuate -40 dBFS floor (got rms={rms_gated})"
    );

    // 4. Brickwall limiter caps output below ceiling (-6 dBFS ≈ 0.501).
    let lim = DspParams {
        limiter_ceiling_db: -6.0,
        ..comp_off
    };
    params.store(&lim);
    let (_, peak_limited) = measure_tone_rms(&mut proc, 1000.0, 0.9, 40);
    assert!(
        peak_limited <= 0.52,
        "limiter ceiling -6 dBFS must cap peak (got {peak_limited})"
    );
}
