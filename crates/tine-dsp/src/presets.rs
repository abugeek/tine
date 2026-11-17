//! Built-in microphone archetype base profiles (Layer A).
//!
//! The four [`PresetDef`]s below are `const` tables and the only preset source
//! the DSP code ever consults: no parsing, no allocation, `no_std`, APO-safe.
//! `assets/presets/*.json` hold the same data in user- and documentation-facing
//! JSON form (schema 1 of [`tine_protocol::presets`]); a dev test in
//! `tests/macros.rs` asserts byte-level equality between the two so they cannot
//! drift.
//!
//! Non-overridden fields inherit [`DspParams::DEFAULT`]. Every value here sits
//! inside the [`tine_protocol::params::SPECS`] clamps (also proven by test).

use tine_protocol::params::DspParams;
use tine_protocol::presets::{Archetype, MacroPositions};

/// One built-in archetype profile: identity, Layer A base curve, and the
/// default Golden Line macro positions this archetype opens with.
///
/// Const-constructible, no allocation, no parsing. The base curve matches the
/// archetype table in `docs/project/04-mic-adaptation.md` (Layer A) exactly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PresetDef {
    /// Hardware archetype this profile describes.
    pub archetype: Archetype,
    /// Stable machine identifier (matches `assets/presets/<id>.json`).
    pub id: &'static str,
    /// Human-readable display name.
    pub name: &'static str,
    /// Layer A baseline DSP curve (sparse overrides over [`DspParams::DEFAULT`]).
    pub base: DspParams,
    /// Default Golden Line macro positions (`0.0..=1.0` each).
    pub macro_defaults: MacroPositions,
}

/// Dynamic USB / XLR broadcast mic (Fifine AM8, SM7B, `PodMic`).
///
/// HPF 80 Hz · low-shelf +2.0 dB @ 110 Hz · peaking −3.0 dB @ 340 Hz (Q 1.4)
/// · high-shelf +3.5 dB @ 8.0 kHz.
pub const DYNAMIC: PresetDef = PresetDef {
    archetype: Archetype::Dynamic,
    id: "dynamic",
    name: "Dynamic USB / XLR",
    base: DspParams {
        hpf_freq_hz: 80.0,
        eq_freq_hz: [110.0, 340.0, 950.0, 4200.0, 8000.0],
        eq_gain_db: [2.0, -3.0, 0.0, 0.0, 3.5],
        eq_q: [0.707, 1.4, 1.0, 1.0, 0.707],
        ..DspParams::DEFAULT
    },
    macro_defaults: MacroPositions {
        purity: 0.4,
        body: 0.5,
        presence: 0.5,
        leveler: 0.5,
    },
};

/// Studio condenser (`HyperX` `QuadCast`, Blue Yeti, AT2020).
///
/// HPF 95 Hz · low-shelf +0.5 dB @ 120 Hz · peaking −2.0 dB @ 280 Hz
/// · de-esser on @ 6.8 kHz (3 dB amount) · high-shelf +1.0 dB @ 10 kHz.
pub const CONDENSER: PresetDef = PresetDef {
    archetype: Archetype::Condenser,
    id: "condenser",
    name: "Studio Condenser",
    base: DspParams {
        hpf_freq_hz: 95.0,
        deesser_enabled: true,
        deesser_freq_hz: 6800.0,
        deesser_amount_db: 3.0,
        eq_freq_hz: [120.0, 280.0, 950.0, 4200.0, 10000.0],
        eq_gain_db: [0.5, -2.0, 0.0, 0.0, 1.0],
        ..DspParams::DEFAULT
    },
    macro_defaults: MacroPositions {
        purity: 0.5,
        body: 0.5,
        presence: 0.5,
        leveler: 0.5,
    },
};

/// Gaming headset / boom mic.
///
/// HPF 100 Hz · low-shelf +4.0 dB @ 130 Hz (synthetic body)
/// · peaking −3.5 dB @ 950 Hz (Q 1.2) · high-shelf +2.0 dB @ 8.0 kHz.
pub const HEADSET: PresetDef = PresetDef {
    archetype: Archetype::Headset,
    id: "headset",
    name: "Gaming Headset / Boom",
    base: DspParams {
        hpf_freq_hz: 100.0,
        eq_freq_hz: [130.0, 320.0, 950.0, 4200.0, 8000.0],
        eq_gain_db: [4.0, 0.0, -3.5, 0.0, 2.0],
        eq_q: [0.707, 1.0, 1.2, 1.0, 0.707],
        ..DspParams::DEFAULT
    },
    macro_defaults: MacroPositions {
        purity: 0.5,
        body: 0.5,
        presence: 0.5,
        leveler: 0.5,
    },
};

/// Laptop / webcam / lavalier mic.
///
/// HPF 110 Hz · `RNNoise` wet mix 0.90 · expander 4:1 · peaking +3.0 dB @ 2.5 kHz
/// (speech intelligibility).
pub const LAPTOP: PresetDef = PresetDef {
    archetype: Archetype::Laptop,
    id: "laptop",
    name: "Laptop / Webcam / Lav",
    base: DspParams {
        hpf_freq_hz: 110.0,
        denoise_mix: 0.9,
        gate_ratio: 4.0,
        eq_freq_hz: [110.0, 320.0, 2500.0, 4200.0, 8500.0],
        eq_gain_db: [0.0, 0.0, 3.0, 0.0, 0.0],
        ..DspParams::DEFAULT
    },
    macro_defaults: MacroPositions {
        purity: 0.5,
        body: 0.5,
        presence: 0.5,
        leveler: 0.5,
    },
};

/// Returns the built-in [`PresetDef`] for `archetype`.
///
/// `const`: resolved at compile time into the static tables above — no parsing,
/// no allocation, `no_std`, safe on the APO side. The four built-ins are the only
/// presets the DSP path ever consumes; user JSON profiles are validated and
/// applied by the control plane (`tine-engine`) instead.
#[must_use]
pub const fn builtin(a: Archetype) -> &'static PresetDef {
    match a {
        Archetype::Dynamic => &DYNAMIC,
        Archetype::Condenser => &CONDENSER,
        Archetype::Headset => &HEADSET,
        Archetype::Laptop => &LAPTOP,
    }
}
