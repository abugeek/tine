//! T10 — archetype presets + `MacroMapper` integration tests.
//!
//! Covers: JSON ↔ const-table equality, golden archetype values at
//! `macro_defaults`, property tests (in-range outputs, monotonicity, pins,
//! idempotence), boundary values, calibration replacement, the de-esser curve,
//! `explain`, allocation-freedom, and the `< 2 µs` apply budget.

use std::time::Instant;

use proptest::prelude::*;
use tine_dsp::macros::{ExplainBuf, MAX_EXPLAIN_ROWS, Macro, MacroMapper};
use tine_dsp::presets::{CONDENSER, DYNAMIC, HEADSET, LAPTOP, PresetDef, builtin};
use tine_protocol::params::{Band, DspParams, EQ_BAND_COUNT, FilterType, PARAM_COUNT, ParamId};
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, MacroPositions, PinSet};

const ARCHETYPES: [Archetype; 4] = [
    Archetype::Dynamic,
    Archetype::Condenser,
    Archetype::Headset,
    Archetype::Laptop,
];

/// Every parameter macro/calibration layers drive (union of the drives tables
/// and the calibration-replacement set).
const DRIVEN: [ParamId; 16] = [
    // Calibration layer
    ParamId::InputGainDb,
    ParamId::GateThresholdDb,
    ParamId::DenoiseMix,
    ParamId::CompThreshDb,
    ParamId::CompMakeupDb,
    // Purity
    ParamId::DenoiseMix,
    ParamId::GateThresholdDb,
    // Body
    ParamId::EqGainDb(Band::B0),
    ParamId::EqGainDb(Band::B1),
    // Presence
    ParamId::EqGainDb(Band::B3),
    ParamId::EqGainDb(Band::B4),
    ParamId::DeesserEnabled,
    ParamId::DeesserAmountDb,
    // Leveler
    ParamId::CompThreshDb,
    ParamId::CompRatio,
    ParamId::CompAutoMakeup,
];

const NEUTRAL: MacroPositions = MacroPositions {
    purity: 0.5,
    body: 0.5,
    presence: 0.5,
    leveler: 0.5,
};

fn all_params_in_spec(out: &DspParams) {
    for i in 0..PARAM_COUNT {
        if let Some(id) = ParamId::from_index(i) {
            let spec = id.spec();
            let v = out.get(id);
            assert!(
                v.is_finite() && v >= spec.min && v <= spec.max,
                "{id:?} = {v} out of [{}, {}] for SPECS entry {i}",
                spec.min,
                spec.max
            );
        }
    }
    // Bool and enum round-trips must hold too.
    for b in 0..EQ_BAND_COUNT {
        let band = Band::new(b as u8).expect("band index < 5");
        let raw = out.get(ParamId::EqType(band));
        let ft = FilterType::from_f32(raw);
        assert_eq!(ft.as_f32(), raw);
        assert!(ft == out.eq_type[b], "round-trip mismatch on band {b}");
    }
}

fn preset_json_path(a: Archetype) -> String {
    format!(
        "{}\\..\\..\\assets\\presets\\{}.json",
        env!("CARGO_MANIFEST_DIR"),
        a.id()
    )
}

// ---------------------------------------------------------------- JSON parity

#[test]
fn json_matches_const_table() {
    for a in ARCHETYPES {
        let path = preset_json_path(a);
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {path}: {e}"));
        let json: tine_protocol::presets::Preset =
            serde_json::from_str(&text).unwrap_or_else(|e| panic!("parse {path}: {e}"));
        json.validate().unwrap_or_else(|e| panic!("validate {path}: {e}"));

        let def = builtin(a);
        assert_eq!(json.id, def.id, "{a:?}: id");
        assert_eq!(json.name, def.name, "{a:?}: name");
        assert_eq!(json.macro_defaults, def.macro_defaults, "{a:?}: macro_defaults");

        let mut base = DspParams::DEFAULT;
        json.params.apply_to(&mut base);
        assert_eq!(base, def.base, "{a:?}: JSON-decoded base != const table");
    }
}

#[test]
fn builtin_matches_archetype_identity() {
    // The const fn must be usable in const context.
    const D: &PresetDef = builtin(Archetype::Dynamic);
    assert_eq!(D.id, "dynamic");
    for a in ARCHETYPES {
        let def = builtin(a);
        assert_eq!(def.archetype, a);
        assert_eq!(def.id, a.id());
        assert_eq!(def.name, a.display_name());
        assert_eq!(def, builtin(a), "builtin must be deterministic");
    }
}

// ---------------------------------------------------------------- golden archetypes

#[test]
fn golden_archetypes_at_macro_defaults() {
    // Dynamic: purity 0.4 shifts mix 0.5 → 0.4 and gate −50 → −53 dBFS;
    // the archetype table values must come out exactly.
    let out = MacroMapper::apply(
        &DYNAMIC,
        None,
        DYNAMIC.macro_defaults,
        PinSet::EMPTY,
        &DspParams::DEFAULT,
    );
    assert_eq!(out.hpf_freq_hz, 80.0);
    assert_eq!(out.eq_type[0], FilterType::LowShelf);
    assert_eq!(out.eq_freq_hz[0], 110.0);
    assert_eq!(out.eq_gain_db[0], 2.0);
    assert_eq!(out.eq_type[1], FilterType::Peaking);
    assert_eq!(out.eq_freq_hz[1], 340.0);
    assert_eq!(out.eq_gain_db[1], -3.0);
    assert_eq!(out.eq_q[1], 1.4);
    assert_eq!(out.eq_freq_hz[4], 8000.0);
    assert_eq!(out.eq_gain_db[4], 3.5);
    // Purity-driven moves at 0.4: centered offsets.
    assert!((out.denoise_mix - 0.4).abs() < f32::EPSILON);
    assert!((out.gate_threshold_db - -53.0).abs() < 1e-4);

    // Condenser: de-esser stays active at default (0.5) presence because the
    // archetype base enables it.
    let out = MacroMapper::apply(
        &CONDENSER,
        None,
        CONDENSER.macro_defaults,
        PinSet::EMPTY,
        &DspParams::DEFAULT,
    );
    assert_eq!(out, CONDENSER.base);
    assert_eq!(out.hpf_freq_hz, 95.0);
    assert!(out.deesser_enabled, "condenser de-esser must be active");
    assert_eq!(out.deesser_freq_hz, 6800.0);
    assert_eq!(out.eq_freq_hz[0], 120.0);
    assert_eq!(out.eq_gain_db[0], 0.5);
    assert_eq!(out.eq_freq_hz[1], 280.0);
    assert_eq!(out.eq_gain_db[1], -2.0);
    assert_eq!(out.eq_freq_hz[4], 10000.0);
    assert_eq!(out.eq_gain_db[4], 1.0);

    // Headset: out == base at centre macros.
    let out = MacroMapper::apply(
        &HEADSET,
        None,
        HEADSET.macro_defaults,
        PinSet::EMPTY,
        &DspParams::DEFAULT,
    );
    assert_eq!(out, HEADSET.base);
    assert_eq!(out.hpf_freq_hz, 100.0);
    assert_eq!(out.eq_freq_hz[0], 130.0);
    assert_eq!(out.eq_gain_db[0], 4.0);
    assert_eq!(out.eq_freq_hz[2], 950.0);
    assert_eq!(out.eq_gain_db[2], -3.5);
    assert_eq!(out.eq_q[2], 1.2);
    assert_eq!(out.eq_freq_hz[4], 8000.0);
    assert_eq!(out.eq_gain_db[4], 2.0);

    // Laptop: out == base at centre macros (mix 0.90 preserved through the
    // Purity anchor).
    let out = MacroMapper::apply(&LAPTOP, None, LAPTOP.macro_defaults, PinSet::EMPTY, &DspParams::DEFAULT);
    assert_eq!(out, LAPTOP.base);
    assert_eq!(out.hpf_freq_hz, 110.0);
    assert!((out.denoise_mix - 0.9).abs() < f32::EPSILON);
    assert_eq!(out.gate_ratio, 4.0);
    assert_eq!(out.eq_freq_hz[2], 2500.0);
    assert_eq!(out.eq_gain_db[2], 3.0);
}

#[test]
fn half_position_reproduces_preset_base() {
    for a in ARCHETYPES {
        let def = builtin(a);
        let out = MacroMapper::apply(def, None, NEUTRAL, PinSet::EMPTY, &DspParams::DEFAULT);
        assert_eq!(out, def.base, "{a:?}: macro 0.5 must give the preset base");
        all_params_in_spec(&out);
    }
}

// ---------------------------------------------------------------- property tests

fn cal_strategy() -> impl Strategy<Value = Calibration> {
    (
        -80.0f32..-20.0, // noise_rms_db
        -80.0f32..0.0,   // noise_peak_db
        -60.0f32..-6.0,  // speech_rms_db
        1.0f32..20.0,    // crest_db
        -20.0f32..20.0,  // tilt_db
        -24.0f32..24.0,  // derived input gain
        -80.0f32..-20.0, // derived gate threshold
        0.0f32..=1.0,    // derived denoise mix
        -50.0f32..-6.0,  // derived comp threshold
        0.0f32..24.0,    // derived comp makeup
    )
        .prop_map(
            |(nrms, npeak, srms, crest, tilt, gain, gate, mix, cthresh, makeup)| Calibration {
                noise_rms_db: nrms,
                noise_peak_db: npeak,
                speech_rms_db: srms,
                crest_db: crest,
                tilt_db: tilt,
                derived: CalibrationOverrides {
                    input_gain_db: gain,
                    gate_threshold_db: gate,
                    denoise_mix: mix,
                    comp_thresh_db: cthresh,
                    comp_makeup_db: makeup,
                },
            },
        )
}

fn arch_strategy() -> impl Strategy<Value = Archetype> {
    (0usize..4).prop_map(|i| ARCHETYPES[i])
}

fn macros_strategy() -> impl Strategy<Value = MacroPositions> {
    (0.0f32..=1.0, 0.0f32..=1.0, 0.0f32..=1.0, 0.0f32..=1.0).prop_map(|(purity, body, presence, leveler)| {
        MacroPositions {
            purity,
            body,
            presence,
            leveler,
        }
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// Every output stays inside the tine-protocol SPECS clamps for any macro
    /// positions in [0, 1] and any valid calibration — no matter the archetype.
    #[test]
    fn prop_outputs_in_specs_range(arch in arch_strategy(), macros in macros_strategy(),
                                   cal in proptest::option::of(cal_strategy())) {
        let def = builtin(arch);
        let out = MacroMapper::apply(def, cal.as_ref(), macros, PinSet::EMPTY, &DspParams::DEFAULT);
        all_params_in_spec(&out);
    }

    /// Monotonicity: each macro moves every parameter it drives in the
    /// documented direction (with or without a calibration).
    #[test]
    fn prop_macros_are_monotone(arch in arch_strategy(), cal in proptest::option::of(cal_strategy()),
                                m0 in 0.0f32..=1.0, m1 in 0.0f32..=1.0,
                                others in macros_strategy()) {
        let def = builtin(arch);
        let (lo, hi) = if m0 <= m1 { (m0, m1) } else { (m1, m0) };

        for which in Macro::ALL {
            let mut a = others;
            let mut b = others;
            match which {
                Macro::Purity => { a.purity = lo; b.purity = hi; }
                Macro::Body => { a.body = lo; b.body = hi; }
                Macro::Presence => { a.presence = lo; b.presence = hi; }
                Macro::Leveler => { a.leveler = lo; b.leveler = hi; }
            }
            let out_a = MacroMapper::apply(def, cal.as_ref(), a, PinSet::EMPTY, &DspParams::DEFAULT);
            let out_b = MacroMapper::apply(def, cal.as_ref(), b, PinSet::EMPTY, &DspParams::DEFAULT);

            for id in MacroMapper::drives(which) {
                let va = out_a.get(*id);
                let vb = out_b.get(*id);
                // CompAutoMakeup is a constant side of Leveler (non-strict).
                if matches!(id, ParamId::CompAutoMakeup) {
                    continue;
                }
                if matches!(id, ParamId::DeesserEnabled) {
                    // Enabled (0/1) may only turn on as the macro rises.
                    prop_assert!(vb >= va, "{which:?} {id:?}: enabled {va} -> {vb} must not turn off");
                } else {
                    let increasing = !matches!(
                        (which, id),
                        (Macro::Body, ParamId::EqGainDb(Band::B1))
                            | (Macro::Leveler, ParamId::CompThreshDb)
                    );
                    if increasing {
                        prop_assert!(vb >= va - 1e-5, "{which:?} {id:?}: {va} -> {vb} must not decrease");
                    } else {
                        prop_assert!(vb <= va + 1e-5, "{which:?} {id:?}: {va} -> {vb} must not increase");
                    }
                }
            }
        }
    }

    /// D-007: pinned parameters never change — they are copied from `current`.
    #[test]
    fn prop_pinned_params_never_change(
        arch in arch_strategy(), macros in macros_strategy(),
        cal in proptest::option::of(cal_strategy()),
        pin_bits in proptest::num::u64::ANY, t in 0.0f32..=1.0) {
        // Only pin driven params.
        let mut mask: u64 = 0;
        for id in DRIVEN {
            mask |= 1u64 << id.index();
        }
        let pins = PinSet::from_bits(pin_bits & mask);

        // `current`: driven params at distinctive value, others neutral.
        let mut current = DspParams::DEFAULT;
        for id in DRIVEN {
            let spec = id.spec();
            current.set(id, spec.min + t * (spec.max - spec.min));
        }

        let def = builtin(arch);
        let out = MacroMapper::apply(def, cal.as_ref(), macros, pins, &current);

        for id in DRIVEN {
            if pins.is_pinned(id) {
                let want = current.get(id);
                let got = out.get(id);
                prop_assert!((got - want).abs() < 1e-6,
                    "pinned {id:?} changed: current={want} out={got}");
            }
        }
        all_params_in_spec(&out);
    }

    /// Re-applying with the previous output as `current` is a fixed point.
    #[test]
    fn prop_apply_is_idempotent(arch in arch_strategy(), macros in macros_strategy(),
                                cal in proptest::option::of(cal_strategy()),
                                pin_bits in proptest::num::u64::ANY) {
        let mut mask: u64 = 0;
        for id in DRIVEN { mask |= 1u64 << id.index(); }
        let pins = PinSet::from_bits(pin_bits & mask);

        let def = builtin(arch);
        let out1 = MacroMapper::apply(def, cal.as_ref(), macros, pins, &DspParams::DEFAULT);
        let out2 = MacroMapper::apply(def, cal.as_ref(), macros, pins, &out1);
        prop_assert_eq!(out1, out2);
        let out3 = MacroMapper::apply(def, cal.as_ref(), macros, pins, &DspParams::DEFAULT);
        prop_assert_eq!(out1, out3);
    }
}

// ---------------------------------------------------------------- boundaries & curves

#[test]
fn boundaries_zero_and_one() {
    let zero = MacroPositions {
        purity: 0.0,
        body: 0.0,
        presence: 0.0,
        leveler: 0.0,
    };
    let one = MacroPositions {
        purity: 1.0,
        body: 1.0,
        presence: 1.0,
        leveler: 1.0,
    };

    for a in ARCHETYPES {
        let def = builtin(a);
        let out = MacroMapper::apply(def, None, zero, PinSet::EMPTY, &DspParams::DEFAULT);
        all_params_in_spec(&out);
        // Full symmetry for the shelf; nothing cut into the mud, no presence boost.
        assert!(
            (out.eq_gain_db[0] - (def.base.eq_gain_db[0] - 4.0)).abs() < 1e-5,
            "{a:?} shelf min"
        );
        assert_eq!(
            out.eq_gain_db[1], def.base.eq_gain_db[1],
            "{a:?} mud stays at base at body=0"
        );

        let out = MacroMapper::apply(def, None, one, PinSet::EMPTY, &DspParams::DEFAULT);
        all_params_in_spec(&out);
        assert!(
            (out.eq_gain_db[0] - (def.base.eq_gain_db[0] + 4.0)).abs() < 1e-5,
            "{a:?} shelf max"
        );
        assert!(
            (out.eq_gain_db[1] - (def.base.eq_gain_db[1] - 5.0)).abs() < 1e-5,
            "{a:?} mud max"
        );
        assert!(
            (out.eq_gain_db[3] - (def.base.eq_gain_db[3] + 5.0)).abs() < 1e-5,
            "{a:?} peak max"
        );
        assert!(
            (out.eq_gain_db[4] - (def.base.eq_gain_db[4] + 6.0)).abs() < 1e-5,
            "{a:?} air max"
        );
        assert!(out.deesser_enabled, "{a:?} de-esser on at presence=1");
        assert!(
            (out.denoise_mix - (def.base.denoise_mix + 0.5).min(1.0)).abs() < 1e-5,
            "{a:?} mix max"
        );
        assert!(
            (out.gate_threshold_db - (def.base.gate_threshold_db + 15.0)).abs() < 1e-5,
            "{a:?} gate max"
        );
        assert!(
            (out.comp_ratio - (def.base.comp_ratio + 1.75)).abs() < 1e-5,
            "{a:?} ratio max"
        );
        assert!(
            (out.comp_thresh_db - (def.base.comp_thresh_db - 10.0)).abs() < 1e-5,
            "{a:?} thresh min"
        );
    }
}

#[test]
fn deesser_curves_match_docs() {
    // Condenser base amount 3 dB: pure curve 3 → 8 dB over presence 0.6 → 1.0.
    let pr = |p: f32| MacroPositions { presence: p, ..NEUTRAL };
    let out = MacroMapper::apply(&CONDENSER, None, pr(0.6), PinSet::EMPTY, &DspParams::DEFAULT);
    assert!(out.deesser_enabled);
    assert!((out.deesser_amount_db - 3.0).abs() < 1e-5);
    let out = MacroMapper::apply(&CONDENSER, None, pr(1.0), PinSet::EMPTY, &DspParams::DEFAULT);
    assert!((out.deesser_amount_db - 8.0).abs() < 1e-5);

    // Default 6 dB base: max() keeps 6 until the curve passes it, then peaks at 8.
    let out = MacroMapper::apply(&DYNAMIC, None, pr(0.7), PinSet::EMPTY, &DspParams::DEFAULT);
    assert_eq!(out.deesser_amount_db, 6.0);
    assert!(out.deesser_enabled, "0.7 > 0.6 engages the de-esser");
    let out = MacroMapper::apply(&DYNAMIC, None, pr(0.4), PinSet::EMPTY, &DspParams::DEFAULT);
    assert!(!out.deesser_enabled, "dynamic base has de-esser off below 0.6");

    // Condenser: base-enabled de-esser stays on below 0.6.
    let out = MacroMapper::apply(&CONDENSER, None, pr(0.0), PinSet::EMPTY, &DspParams::DEFAULT);
    assert!(out.deesser_enabled);
}

#[test]
fn calibration_replaces_preset_values() {
    let cal = Calibration {
        noise_rms_db: -52.0,
        noise_peak_db: -40.0,
        speech_rms_db: -30.0,
        crest_db: 12.0,
        tilt_db: -3.0,
        derived: CalibrationOverrides {
            input_gain_db: 9.5,
            gate_threshold_db: -46.0,
            denoise_mix: 0.85,
            comp_thresh_db: -28.0,
            comp_makeup_db: 5.0,
        },
    };
    let out = MacroMapper::apply(&DYNAMIC, Some(&cal), NEUTRAL, PinSet::EMPTY, &DspParams::DEFAULT);
    assert!((out.input_gain_db - 9.5).abs() < f32::EPSILON);
    assert!((out.denoise_mix - 0.85).abs() < f32::EPSILON);
    assert!((out.gate_threshold_db - -46.0).abs() < f32::EPSILON);
    assert!((out.comp_thresh_db - -28.0).abs() < f32::EPSILON);
    assert!((out.comp_makeup_db - 5.0).abs() < f32::EPSILON);
    assert!(!out.comp_auto_makeup, "calibration supplies a verified manual makeup");

    // Purity/Leveler move relative to the calibrated anchors.
    let m = MacroPositions {
        purity: 0.75,
        leveler: 0.75,
        ..NEUTRAL
    };
    let out = MacroMapper::apply(&DYNAMIC, Some(&cal), m, PinSet::EMPTY, &DspParams::DEFAULT);
    assert!(
        (out.gate_threshold_db - (-46.0 + 0.5 * 15.0)).abs() < 1e-4,
        "gate anchored on calibration"
    );
    assert!(
        (out.comp_thresh_db - (-28.0 - 10.0 * 0.5)).abs() < 1e-4,
        "thresh anchored on calibration"
    );
    // 0.85 + 0.25 exceeds the SPECS max of 1.0 — clamped through SPECS.
    assert!(out.denoise_mix == 1.0, "mix clamped at SPECS max: {}", out.denoise_mix);

    // Without calibration the same macros anchor on the preset base.
    let out = MacroMapper::apply(&DYNAMIC, None, m, PinSet::EMPTY, &DspParams::DEFAULT);
    assert!((out.denoise_mix - (0.5 + 0.25)).abs() < 1e-5);
    assert!((out.gate_threshold_db - (-50.0 + 0.5 * 15.0)).abs() < 1e-4);
}

// ---------------------------------------------------------------- explain

#[test]
fn explain_rows_match_apply_and_pins() {
    let def = builtin(Archetype::Condenser);
    let macros = MacroPositions {
        purity: 0.3,
        body: 0.7,
        presence: 0.65,
        leveler: 0.8,
    };
    let mut pins = PinSet::EMPTY;
    pins.pin(ParamId::EqGainDb(Band::B3));

    let mut buf = ExplainBuf::new();
    MacroMapper::explain(def, None, macros, pins, Macro::Presence, &mut buf);
    assert!(buf.len() <= MAX_EXPLAIN_ROWS);
    assert_eq!(buf.len(), MacroMapper::drives(Macro::Presence).len());

    let mapped = MacroMapper::apply(def, None, macros, PinSet::EMPTY, &DspParams::DEFAULT);
    let base = def.base;
    for row in buf.rows() {
        assert!(
            (row.after - mapped.get(row.id)).abs() < f32::EPSILON,
            "after must equal the mapped value"
        );
        assert!(
            (row.before - base.get(row.id)).abs() < f32::EPSILON,
            "before must equal the effective base"
        );
        assert_eq!(row.pinned, pins.is_pinned(row.id));
    }
    // Band-3 peak is pinned and reported as such.
    let b3 = buf
        .rows()
        .iter()
        .find(|r| r.id == ParamId::EqGainDb(Band::B3))
        .expect("presence explains band 3");
    assert!(b3.pinned);

    // Row counts per macro.
    for which in Macro::ALL {
        let mut b = ExplainBuf::new();
        MacroMapper::explain(def, None, NEUTRAL, PinSet::EMPTY, which, &mut b);
        assert_eq!(b.len(), MacroMapper::drives(which).len());
        assert!(b.len() <= MAX_EXPLAIN_ROWS);
    }
}

// ---------------------------------------------------------------- RT safety

#[test]
fn apply_is_allocation_free() {
    let cal = cal_fixture();
    let out = assert_no_alloc::assert_no_alloc(|| {
        let mut buf = ExplainBuf::new();
        MacroMapper::explain(&LAPTOP, Some(&cal), NEUTRAL, PinSet::EMPTY, Macro::Presence, &mut buf);
        MacroMapper::apply(&LAPTOP, Some(&cal), NEUTRAL, PinSet::EMPTY, &DspParams::DEFAULT)
    });
    all_params_in_spec(&out);
}

#[test]
fn apply_meets_time_budget() {
    const BATCH: u32 = 500;
    const BUDGET_NS: f64 = if cfg!(debug_assertions) { 50_000.0 } else { 2_000.0 };

    let cal = cal_fixture();
    let worst = MacroPositions {
        purity: 0.37,
        body: 0.68,
        presence: 0.91,
        leveler: 0.44,
    };
    // Warm up caches.
    for _ in 0..100 {
        let _ = MacroMapper::apply(&CONDENSER, Some(&cal), worst, PinSet::EMPTY, &DspParams::DEFAULT);
    }
    // Best-of-batches to filter out scheduler noise.
    let mut best_ns = f64::MAX;
    for _ in 0..20 {
        let start = Instant::now();
        for _ in 0..BATCH {
            let out = MacroMapper::apply(&CONDENSER, Some(&cal), worst, PinSet::EMPTY, &DspParams::DEFAULT);
            std::hint::black_box(out);
        }
        let dt = start.elapsed().as_secs_f64();
        best_ns = best_ns.min(dt / f64::from(BATCH) * 1e9);
    }
    println!("apply best-of-batches: {best_ns:.1} ns/call (budget {BUDGET_NS:.0} ns)");
    assert!(best_ns < BUDGET_NS, "apply too slow: {best_ns:.1} ns/call");
}

fn cal_fixture() -> Calibration {
    Calibration {
        noise_rms_db: -52.0,
        noise_peak_db: -41.0,
        speech_rms_db: -30.0,
        crest_db: 12.0,
        tilt_db: -3.0,
        derived: CalibrationOverrides {
            input_gain_db: 9.5,
            gate_threshold_db: -46.0,
            denoise_mix: 0.85,
            comp_thresh_db: -28.0,
            comp_makeup_db: 5.0,
        },
    }
}
