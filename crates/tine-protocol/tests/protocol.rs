//! Integration, property, allocation-freedom, and concurrency stress tests for `tine-protocol`.

use std::alloc::System;
use std::mem::{align_of, size_of};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use assert_no_alloc::{AllocDisabler, assert_no_alloc};
use proptest::prelude::*;
use tine_protocol::params::{Band, DspParams, PARAM_COUNT, ParamId, SPECS};
use tine_protocol::shm::{
    SHM_LAYOUT_VERSION, SHM_MAGIC, ShmError, ShmLayout, init_layout_in_place, layout_from_ptr, shm_name,
};
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::AudioTelemetryFrame;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[test]
fn param_id_index_round_trip() {
    for (i, spec) in SPECS.iter().enumerate() {
        let id = ParamId::from_index(i).expect("valid index");
        assert_eq!(id.index(), i);
        assert_eq!(spec.id, id);
    }
    assert_eq!(ParamId::from_index(PARAM_COUNT), None);
    assert_eq!(ParamId::from_index(usize::MAX), None);
}

#[test]
fn dsp_params_get_set_round_trip() {
    let mut p = DspParams::DEFAULT;
    for spec in &SPECS {
        let mid = match spec.id {
            ParamId::EqType(_) => 3.0, // Notch
            _ if spec.max - spec.min <= 1.0 && spec.step == 1.0 => 1.0,
            _ => f32::midpoint(spec.min, spec.max),
        };
        p.set(spec.id, mid);
        let got = p.get(spec.id);
        assert!(
            (got - mid).abs() < 1e-4,
            "mismatch for {}: expected {mid}, got {got}",
            spec.key
        );
    }
}

#[test]
fn dsp_params_clamped_handles_nan_inf_and_oob() {
    let mut p = DspParams::DEFAULT;
    p.input_gain_db = f32::NAN;
    p.hpf_freq_hz = f32::INFINITY;
    p.denoise_mix = -5.0;
    p.gate_threshold_db = 10.0;
    p.limiter_ceiling_db = f32::NEG_INFINITY;
    p.eq_gain_db[2] = 99.0;

    let c = p.clamped();
    assert_eq!(c.input_gain_db, ParamId::InputGainDb.spec().default);
    assert_eq!(c.hpf_freq_hz, ParamId::HpfFreqHz.spec().default);
    assert_eq!(c.denoise_mix, 0.0);
    assert_eq!(c.gate_threshold_db, -20.0);
    assert_eq!(c.limiter_ceiling_db, ParamId::LimiterCeilingDb.spec().default);
    assert_eq!(c.eq_gain_db[2], 18.0);
}

proptest! {
    #[test]
    fn proptest_set_always_stays_in_range(idx in 0usize..PARAM_COUNT, val in proptest::num::f32::ANY) {
        let id = ParamId::from_index(idx).expect("valid param index");
        let spec = id.spec();
        let mut p = DspParams::DEFAULT;
        p.set(id, val);
        let got = p.get(id);
        prop_assert!(got.is_finite());
        prop_assert!(got >= spec.min && got <= spec.max);
    }
}

#[test]
fn rt_paths_perform_zero_allocations() {
    let block = ParamBlock::new();
    let mut params = DspParams::DEFAULT;
    params.input_gain_db = 6.0;
    let mut last_gen = 0u64;
    let mut out_params = DspParams::DEFAULT;

    let triple = Triple::new(AudioTelemetryFrame::default());
    let frame = AudioTelemetryFrame {
        sequence: 42,
        input_rms_db: -20.0,
        ..AudioTelemetryFrame::default()
    };
    let mut out_frame = AudioTelemetryFrame::default();

    let ring: Box<SpscRing<1024>> = Box::new(SpscRing::new());
    let samples = [0.5f32; 480];
    let mut popped = [0.0f32; 480];

    let _ = System; // keep System imported if needed, assert_no_alloc wraps the RT block
    assert_no_alloc(|| {
        block.store(&params);
        assert!(block.load_if_changed(&mut last_gen, &mut out_params));
        assert!(!block.load_if_changed(&mut last_gen, &mut out_params));

        let _ = out_params.get(ParamId::InputGainDb);
        out_params.set(ParamId::EqGainDb(Band::B1), -3.0);
        let _ = out_params.clamped();

        triple.publish(&frame);
        assert!(triple.latest(&mut out_frame));
        assert!(!triple.latest(&mut out_frame));

        assert_eq!(ring.push_slice(&samples), 480);
        assert_eq!(ring.pop_slice(&mut popped), 480);

        let name = shm_name("{0.0.1.00000000}.{1234-5678}");
        assert!(name.as_str().starts_with("Global\\Tine.v1."));
    });
}

#[test]
fn shm_name_sanitizes_and_nul_terminates() {
    let name = shm_name("mic\\with/bad:chars*and spaces");
    assert_eq!(name.as_str(), "Global\\Tine.v1.mic_with_bad_chars_and_spaces");
    let bytes = name.as_bytes_with_nul();
    assert_eq!(bytes.last(), Some(&0));
}

#[repr(C, align(64))]
struct AlignedShmBuf([u8; size_of::<ShmLayout>()]);

#[test]
#[allow(clippy::large_stack_arrays, clippy::cast_ptr_alignment)]
fn shm_layout_validation_rejects_invalid_and_accepts_valid() {
    let mut buf = vec![0u8; size_of::<ShmLayout>() + 64];
    let raw_ptr = buf.as_mut_ptr();
    let align_offset = raw_ptr.align_offset(align_of::<AlignedShmBuf>());
    // SAFETY: `buf` has 64 bytes of slack, so `raw_ptr.add(align_offset)` is within bounds and 64-byte aligned.
    let ptr = unsafe { raw_ptr.add(align_offset) };
    let len = size_of::<ShmLayout>();

    // Null pointer
    assert_eq!(
        // SAFETY: testing null pointer rejection in `layout_from_ptr`.
        unsafe { layout_from_ptr(std::ptr::null_mut(), len) }.err(),
        Some(ShmError::NullPointer)
    );

    // Unaligned pointer
    assert_eq!(
        // SAFETY: testing unaligned pointer rejection in `layout_from_ptr`.
        unsafe { layout_from_ptr(ptr.add(1), len - 1) }.err(),
        Some(ShmError::Unaligned)
    );

    // Too small
    assert_eq!(
        // SAFETY: testing undersized buffer rejection in `layout_from_ptr`.
        unsafe { layout_from_ptr(ptr, len - 64) }.err(),
        Some(ShmError::TooSmall {
            actual: len - 64,
            expected: size_of::<ShmLayout>()
        })
    );

    // Bad magic (zeroed buffer)
    // SAFETY: `ptr` is valid and aligned for `ShmLayout` over `len` bytes.
    assert_eq!(unsafe { layout_from_ptr(ptr, len) }.err(), Some(ShmError::BadMagic(0)));

    // Initialize properly
    let mut init_params = DspParams::DEFAULT;
    init_params.hpf_freq_hz = 110.0;
    // SAFETY: `ptr` is valid, 64-byte aligned, and spans `len` bytes.
    let layout =
        unsafe { init_layout_in_place(ptr, len, *b"tine-test-0.1.0\0", 48_000, 2, &init_params).expect("valid init") };
    assert_eq!(layout.header.magic, SHM_MAGIC);
    assert_eq!(layout.header.layout_version, SHM_LAYOUT_VERSION);
    assert_eq!(layout.header.sample_rate.load(Ordering::Relaxed), 48_000);
    assert_eq!(layout.header.channels.load(Ordering::Relaxed), 2);

    // SAFETY: `ptr` was just initialized by `init_layout_in_place`.
    let validated = unsafe { layout_from_ptr(ptr, len) }.expect("valid layout");
    let mut last_gen = 0;
    let mut read_params = DspParams::DEFAULT;
    assert!(validated.params.load_if_changed(&mut last_gen, &mut read_params));
    assert_eq!(read_params.hpf_freq_hz, 110.0);

    // Wrong version
    // SAFETY: `ptr` is 64-byte aligned and valid; we overwrite `layout_version` at offset 8 for negative test.
    unsafe {
        let ver_ptr = ptr.add(8).cast::<u32>();
        *ver_ptr = 999;
    }
    assert_eq!(
        // SAFETY: `ptr` is valid and aligned for `ShmLayout` over `len` bytes.
        unsafe { layout_from_ptr(ptr, len) }.err(),
        Some(ShmError::VersionMismatch {
            actual: 999,
            expected: SHM_LAYOUT_VERSION
        })
    );
    let _ = align_of::<ShmLayout>();
}

#[test]
fn triple_10m_stress_test_never_tears() {
    const ITERS: u64 = 10_000_000;
    let triple = Arc::new(Triple::new(AudioTelemetryFrame::default()));
    let done = Arc::new(AtomicBool::new(false));

    let t_writer = Arc::clone(&triple);
    let d_writer = Arc::clone(&done);
    let writer = thread::spawn(move || {
        for seq in 1..=ITERS {
            let f = AudioTelemetryFrame {
                sequence: seq,
                flags: (seq as u32) ^ 0xDEAD_BEEF,
                input_rms_db: (seq & 0xFFFF) as f32,
                input_peak_db: ((seq >> 16) & 0xFFFF) as f32,
                output_rms_db: -(seq as f32),
                output_peak_db: (seq ^ 0x5555_5555) as u32 as f32,
                gate_attenuation_db: 0.0,
                comp_gain_reduction_db: 0.0,
                deesser_reduction_db: 0.0,
            };
            t_writer.publish(&f);
        }
        d_writer.store(true, Ordering::Release);
    });

    let mut last_seq = 0u64;
    let mut out = AudioTelemetryFrame::default();
    let mut observed = 0u64;
    loop {
        if triple.latest(&mut out) {
            assert!(
                out.sequence > last_seq,
                "non-monotonic sequence: {} <= {last_seq}",
                out.sequence
            );
            assert_eq!(out.flags, (out.sequence as u32) ^ 0xDEAD_BEEF, "torn flags");
            assert_eq!(out.input_rms_db, (out.sequence & 0xFFFF) as f32, "torn input_rms_db");
            assert_eq!(
                out.input_peak_db,
                ((out.sequence >> 16) & 0xFFFF) as f32,
                "torn input_peak_db"
            );
            assert_eq!(out.output_rms_db, -(out.sequence as f32), "torn output_rms_db");
            assert_eq!(
                out.output_peak_db,
                (out.sequence ^ 0x5555_5555) as u32 as f32,
                "torn output_peak_db"
            );
            last_seq = out.sequence;
            observed += 1;
        } else if done.load(Ordering::Acquire) {
            // One final check after writer finished
            if triple.latest(&mut out) {
                assert_eq!(out.flags, (out.sequence as u32) ^ 0xDEAD_BEEF);
                last_seq = out.sequence;
                observed += 1;
            }
            break;
        }
    }

    writer.join().expect("writer thread panicked");
    assert_eq!(last_seq, ITERS);
    assert!(observed > 0);
}

#[test]
fn spsc_ring_two_thread_streaming() {
    const TOTAL: usize = 200_000;
    let ring: Arc<SpscRing<1024>> = Arc::new(SpscRing::new());
    let r_prod = Arc::clone(&ring);

    let producer = thread::spawn(move || {
        let mut sent = 0usize;
        let mut chunk = [0.0f32; 128];
        while sent < TOTAL {
            let n = chunk.len().min(TOTAL - sent);
            for (i, slot) in chunk[..n].iter_mut().enumerate() {
                *slot = (sent + i) as f32;
            }
            let pushed = r_prod.push_slice(&chunk[..n]);
            sent += pushed;
            if pushed == 0 {
                thread::yield_now();
            }
        }
    });

    let mut received = 0usize;
    let mut buf = [0.0f32; 97];
    while received < TOTAL {
        let n = ring.pop_slice(&mut buf);
        for &v in &buf[..n] {
            assert_eq!(v, received as f32);
            received += 1;
        }
        if n == 0 {
            thread::yield_now();
        }
    }

    producer.join().expect("producer panicked");
    assert_eq!(received, TOTAL);
}

#[test]
fn spsc_ring_overrun_counts_dropped_samples() {
    let ring: SpscRing<4> = SpscRing::new();
    assert_eq!(ring.push_slice(&[1.0, 2.0, 3.0]), 3);
    assert_eq!(ring.overruns(), 0);
    // Only 1 slot free; pushing 3 drops 2 samples.
    assert_eq!(ring.push_slice(&[4.0, 5.0, 6.0]), 1);
    assert_eq!(ring.overruns(), 2);

    let mut out = [0.0f32; 4];
    assert_eq!(ring.pop_slice(&mut out), 4);
    assert_eq!(out, [1.0, 2.0, 3.0, 4.0]);
}

#[cfg(feature = "serde")]
mod serde_tests {
    use tine_protocol::ids::SCHEMA_VERSION;
    use tine_protocol::params::{DspParams, EQ_BAND_COUNT, FilterType, ParamId};
    use tine_protocol::presets::{
        Archetype, Calibration, CalibrationOverrides, EndpointProfile, EqBandOverride, MacroPositions, PinSet, Preset,
        PresetParams, ValidationError,
    };
    use tine_protocol::settings::AppSettings;

    #[test]
    fn preset_json_round_trip_and_forward_compat() {
        let json = r#"{
            "schema": 1,
            "id": "dynamic",
            "name": "Dynamic USB / XLR",
            "future_unknown_field": { "hello": 123 },
            "params": {
                "hpf_freq_hz": 80.0,
                "eq": [
                    { "type": "low_shelf", "freq_hz": 110.0, "gain_db": 2.0, "q": 0.707 },
                    { "type": "peaking", "freq_hz": 340.0, "gain_db": -3.0, "q": 1.4 }
                ]
            },
            "macro_defaults": { "purity": 0.4, "body": 0.5, "presence": 0.5, "leveler": 0.5 }
        }"#;

        let preset: Preset = serde_json::from_str(json).expect("valid preset json");
        preset.validate().expect("valid preset");
        assert_eq!(preset.id, "dynamic");
        assert_eq!(preset.params.eq.len(), 2);

        let mut dsp = DspParams::DEFAULT;
        preset.params.apply_to(&mut dsp);
        assert_eq!(dsp.eq_type[0], FilterType::LowShelf);
        assert_eq!(dsp.eq_gain_db[1], -3.0);

        let reserialized = serde_json::to_string(&preset).expect("serialize");
        let reparsed: Preset = serde_json::from_str(&reserialized).expect("reparse");
        assert_eq!(preset, reparsed);
    }

    #[test]
    fn endpoint_profile_and_settings_round_trip_and_validation() {
        let mut pins = PinSet::EMPTY;
        pins.pin(ParamId::CompThreshDb);
        pins.pin(ParamId::LimiterCeilingDb);
        assert!(pins.is_pinned(ParamId::CompThreshDb));
        assert!(!pins.is_pinned(ParamId::Bypass));

        let profile = EndpointProfile {
            schema: SCHEMA_VERSION,
            endpoint_id: "{0.0.1.00000000}.{abcd}".to_owned(),
            archetype: Archetype::Condenser,
            macros: MacroPositions {
                purity: 0.7,
                body: 0.4,
                presence: 0.6,
                leveler: 0.5,
            },
            pins,
            calibration: Some(Calibration {
                noise_rms_db: -55.0,
                noise_peak_db: -42.0,
                speech_rms_db: -22.0,
                crest_db: 12.0,
                tilt_db: -3.5,
                derived: CalibrationOverrides {
                    input_gain_db: 4.0,
                    gate_threshold_db: -49.0,
                    denoise_mix: 0.5,
                    comp_thresh_db: -16.0,
                    comp_makeup_db: 2.0,
                },
            }),
            overrides: PresetParams::default(),
        };
        profile.validate().expect("valid profile");

        let json = serde_json::to_string(&profile).expect("serialize profile");
        let parsed: EndpointProfile = serde_json::from_str(&json).expect("deserialize profile");
        assert_eq!(profile, parsed);

        // Validation error cases
        let mut bad_preset = Preset {
            schema: 99,
            id: "x".to_owned(),
            name: "X".to_owned(),
            params: PresetParams::default(),
            macro_defaults: MacroPositions::default(),
        };
        assert!(matches!(
            bad_preset.validate(),
            Err(ValidationError::UnsupportedSchema { actual: 99, .. })
        ));

        bad_preset.schema = 1;
        bad_preset.params.input_gain_db = Some(50.0);
        bad_preset.params.hpf_freq_hz = Some(5.0);
        match bad_preset.validate() {
            Err(ValidationError::OutOfRange(fields)) => {
                assert_eq!(fields.len(), 2);
                assert_eq!(fields[0].field, "input_gain_db");
                assert_eq!(fields[1].field, "hpf_freq_hz");
            }
            other => panic!("unexpected validation result: {other:?}"),
        }

        bad_preset.params = PresetParams {
            eq: vec![EqBandOverride::default(); EQ_BAND_COUNT + 1],
            ..PresetParams::default()
        };
        assert!(matches!(
            bad_preset.validate(),
            Err(ValidationError::TooManyEqBands { .. })
        ));

        let settings = AppSettings::default();
        settings.validate().expect("valid default settings");
        let s_json = serde_json::to_string(&settings).expect("serialize settings");
        let s_parsed: AppSettings = serde_json::from_str(&s_json).expect("deserialize settings");
        assert_eq!(settings, s_parsed);
    }
}
