//! Application state model and parameter resolution.
//!
//! [`AppModel`] is the single source of truth for what the user configured:
//! selected endpoint, its [`EndpointProfile`], bypass, and "Hear Myself"
//! monitor state. [`resolve`] folds it into the [`DspParams`] snapshot the DSP
//! consumes.
//!
//! Resolution order (per `docs/project/04-mic-adaptation.md`):
//!
//! ```text
//! DspParams = MacroMapper( archetype_base ⊕ calibration , macros ) ⊕ pro_overrides
//! ```
//!
//! **T10 not merged yet (early start):** the archetype base curve and
//! `MacroMapper` live in `tine-dsp` (`tine_dsp::presets`, `tine_dsp::macros`)
//! and are not on `main` yet. Until they land, [`resolve`] starts from the
//! neutral [`DspParams::DEFAULT`] and applies a *temporary, local* macro
//! mapping derived from `docs/project/04-mic-adaptation.md` so that Golden
//! Line semantics (including D-007 pinning) are exercisable and testable.
//! Every such site is marked `TODO(T10)`; folding `MacroMapper` in is a
//! drop-in replacement of [`resolve`].

use std::fmt;

use tine_protocol::params::{Band, DspParams, FilterType, ParamId};
use tine_protocol::presets::{Archetype, EndpointProfile, MacroPositions, PresetParams};

use crate::persist::PersistError;
use crate::shm_client::ShmOpenError;

/// Windows `MMDevice` endpoint identifier string.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EndpointId(String);

impl EndpointId {
    /// Creates an endpoint ID from its raw `MMDevice` string.
    #[must_use]
    pub fn new(raw: impl Into<String>) -> Self {
        Self(raw.into())
    }

    /// Returns the raw endpoint ID string.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for EndpointId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl From<EndpointId> for String {
    fn from(id: EndpointId) -> Self {
        id.0
    }
}

/// One of the four "Golden Line" macro nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Macro {
    /// Noise & gate (`RNNoise` wet mix, expander threshold).
    Purity,
    /// Warmth & mud control (low-shelf body, mud cut).
    Body,
    /// Clarity & de-ess (presence peak, air shelf, de-esser).
    Presence,
    /// Compression & voice focus (ratio, threshold, auto-makeup).
    Leveler,
}

impl Macro {
    /// Returns all macros in UI order.
    pub const ALL: [Self; 4] = [Self::Purity, Self::Body, Self::Presence, Self::Leveler];

    /// Stable machine name (persistence keys, CLI).
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Purity => "purity",
            Self::Body => "body",
            Self::Presence => "presence",
            Self::Leveler => "leveler",
        }
    }

    /// Reads this macro's position from `macros`.
    #[must_use]
    pub const fn get(self, macros: &MacroPositions) -> f32 {
        match self {
            Self::Purity => macros.purity,
            Self::Body => macros.body,
            Self::Presence => macros.presence,
            Self::Leveler => macros.leveler,
        }
    }

    /// Writes this macro's position into `macros`.
    pub fn set(self, macros: &mut MacroPositions, v: f32) {
        let clamped = if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 };
        match self {
            Self::Purity => macros.purity = clamped,
            Self::Body => macros.body = clamped,
            Self::Presence => macros.presence = clamped,
            Self::Leveler => macros.leveler = clamped,
        }
    }

    /// Parses a macro by machine name.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|m| m.name() == name)
    }
}

/// "Hear Myself" monitor state (controller-owned, not a DSP parameter; D-008).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MonitorSettings {
    /// Selected playback device ID (`None` = system default).
    pub output_device: Option<String>,
    /// Whether monitoring is enabled.
    pub enabled: bool,
}

/// The complete user-side state owned by the controller thread.
#[derive(Clone, Debug, PartialEq)]
pub struct AppModel {
    /// Selected capture endpoint (`None` until the user picks one).
    pub endpoint: Option<EndpointId>,
    /// Per-endpoint profile: archetype, macros, pins, calibration, overrides.
    pub profile: EndpointProfile,
    /// Global A/B bypass.
    pub bypass: bool,
    /// "Hear Myself" monitor state.
    pub monitor: MonitorSettings,
}

impl AppModel {
    /// Creates a model for `endpoint` with a fresh default profile.
    #[must_use]
    pub fn new(endpoint: EndpointId) -> Self {
        Self {
            endpoint: Some(endpoint.clone()),
            profile: EndpointProfile::new(endpoint, Archetype::default()),
            bypass: false,
            monitor: MonitorSettings::default(),
        }
    }
}

/// Whether an operation keeps the current Pro-mode pins (D-007).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeepPins {
    /// Keep existing pins untouched.
    Keep,
    /// Clear pins for parameters the operation now drives.
    Clear,
}

/// Errors surfaced through [`crate::controller::Event::Error`].
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    /// A persistence write or load failed.
    #[error(transparent)]
    Persist(#[from] PersistError),
    /// Opening or validating the APO shared memory failed.
    #[error(transparent)]
    Shm(#[from] ShmOpenError),
    /// A command carried a value outside its parameter's valid range and was
    /// clamped or rejected (see the message).
    #[error("invalid command: {0}")]
    InvalidCommand(String),
}

/// Clamps `v` into `id`'s [`tine_protocol::params::SPECS`] range, replacing
/// non-finite values with the parameter's default.
#[must_use]
pub fn clamp_param(id: ParamId, v: f32) -> f32 {
    let spec = id.spec();
    if v.is_finite() {
        v.clamp(spec.min, spec.max)
    } else {
        spec.default
    }
}

/// Linear interpolation helper for the temporary macro maps.
fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// Neutral `gate_threshold_db` the Purity macro swings around (`−60..=−30` dB
/// around its midpoint of `−45` dB, so macro 0.5 is neutral).
const NEUTRAL_GATE_THRESHOLD_DB: f32 = -45.0;

/// Neutral de-esser amount (dB) below which Presence does not engage it.
const DEESSER_ENGAGE: f32 = 0.6;

/// Resolves the [`AppModel`] into the [`DspParams`] snapshot for the DSP.
///
/// Layer order: neutral defaults → calibration → macros (skipping pinned
/// parameters) → Pro-mode overrides → bypass.
#[must_use]
pub fn resolve(model: &AppModel) -> DspParams {
    let profile = &model.profile;
    let macros = profile.macros.clamped();
    let pins = profile.pins;
    let cal = profile.calibration;

    let mut p = DspParams::DEFAULT;
    // TODO(T10): start from the archetype base curve:
    //   let mut p = tine_dsp::presets::base(profile.archetype).to_dsp_params();
    // instead of the neutral default, and route every macro-dependent value
    // through `tine_dsp::macros::MacroMapper::map` instead of the local
    // tables below.

    // Layer B — calibration (drives the params Auto-Tune measured).
    if let Some(cal) = cal {
        p.set(ParamId::InputGainDb, cal.derived.input_gain_db);
        p.set(ParamId::GateThresholdDb, cal.derived.gate_threshold_db);
        p.set(ParamId::DenoiseMix, cal.derived.denoise_mix);
        p.set(ParamId::CompThreshDb, cal.derived.comp_thresh_db);
        p.set(ParamId::CompMakeupDb, cal.derived.comp_makeup_db);
    }

    // Layer C — Golden Line macros (temporary local mapping; TODO(T10)).
    // Pinned parameters (D-007) keep their calibrated/overridden value.
    let cal_gate = cal.map_or(NEUTRAL_GATE_THRESHOLD_DB, |c| c.derived.gate_threshold_db);
    let cal_comp = cal.map_or(p.comp_thresh_db, |c| c.derived.comp_thresh_db);

    if !pins.is_pinned(ParamId::DenoiseMix) {
        // TODO(T10): route through MacroMapper::map — purity: mix 0.0 -> 1.0.
        p.set(ParamId::DenoiseMix, macros.purity);
    }
    if !pins.is_pinned(ParamId::GateThresholdDb) {
        // TODO(T10): MacroMapper — purity swings the calibrated threshold
        // within the documented −60 .. −30 dB envelope around its midpoint.
        p.set(
            ParamId::GateThresholdDb,
            cal_gate + lerp(NEUTRAL_GATE_THRESHOLD_DB - 15.0, NEUTRAL_GATE_THRESHOLD_DB + 15.0, macros.purity)
                - NEUTRAL_GATE_THRESHOLD_DB,
        );
    }
    if !pins.is_pinned(ParamId::EqGainDb(Band::B0)) {
        // TODO(T10): MacroMapper — body: low-shelf warmth −2 .. +6 dB.
        p.set(ParamId::EqGainDb(Band::B0), lerp(-2.0, 6.0, macros.body));
    }
    if !pins.is_pinned(ParamId::EqGainDb(Band::B1)) {
        // TODO(T10): MacroMapper — body: mud cut 0 .. −5 dB (deepens as body rises).
        p.set(ParamId::EqGainDb(Band::B1), lerp(0.0, -5.0, macros.body));
    }
    if !pins.is_pinned(ParamId::EqGainDb(Band::B3)) {
        // TODO(T10): MacroMapper — presence: 4.2 kHz peak 0 .. +5 dB.
        p.set(ParamId::EqGainDb(Band::B3), lerp(0.0, 5.0, macros.presence));
    }
    if !pins.is_pinned(ParamId::EqGainDb(Band::B4)) {
        // TODO(T10): MacroMapper — presence: 8.5 kHz air shelf 0 .. +6 dB.
        p.set(ParamId::EqGainDb(Band::B4), lerp(0.0, 6.0, macros.presence));
    }
    if !pins.is_pinned(ParamId::DeesserEnabled) {
        // TODO(T10): MacroMapper — presence engages the de-esser above 0.6.
        p.set(
            ParamId::DeesserEnabled,
            if macros.presence > DEESSER_ENGAGE { 1.0 } else { 0.0 },
        );
    }
    if !pins.is_pinned(ParamId::DeesserAmountDb) {
        // TODO(T10): MacroMapper — amount scales 3 .. 8 dB over 0.6 -> 1.0.
        let t = ((macros.presence - DEESSER_ENGAGE) / (1.0 - DEESSER_ENGAGE)).clamp(0.0, 1.0);
        p.set(ParamId::DeesserAmountDb, lerp(3.0, 8.0, t));
    }
    if !pins.is_pinned(ParamId::CompRatio) {
        // TODO(T10): MacroMapper — leveler: ratio 1.5:1 -> 5:1.
        p.set(ParamId::CompRatio, lerp(1.5, 5.0, macros.leveler));
    }
    if !pins.is_pinned(ParamId::CompThreshDb) {
        // TODO(T10): MacroMapper — leveler lowers the calibrated threshold
        // by 0 .. 10 dB and enables auto-makeup.
        p.set(ParamId::CompThreshDb, cal_comp - lerp(0.0, 10.0, macros.leveler));
    }
    if !pins.is_pinned(ParamId::CompAutoMakeup) {
        // TODO(T10): MacroMapper — leveler enables auto-makeup.
        p.set(ParamId::CompAutoMakeup, 1.0);
    }

    // Pro-mode direct overrides (pinned by definition; see D-007).
    profile.overrides.apply_to(&mut p);

    // Global A/B bypass is controller state applied last.
    p.bypass = model.bypass;

    p.clamped()
}

/// Gets (or creates) the sparse EQ override entry for `band` and applies `edit`
/// to it. Entries created here carry an explicit band index so they round-trip
/// through JSON unambiguously.
fn eq_edit(overrides: &mut PresetParams, band: Band, edit: impl FnOnce(&mut tine_protocol::presets::EqBandOverride)) {
    let idx = band.as_u8();
    if let Some(pos) = overrides.eq.iter().position(|e| e.band == Some(idx)) {
        if let Some(entry) = overrides.eq.get_mut(pos) {
            edit(entry);
        }
    } else {
        overrides.eq.push(tine_protocol::presets::EqBandOverride {
            band: Some(idx),
            ..tine_protocol::presets::EqBandOverride::default()
        });
        if let Some(entry) = overrides.eq.last_mut() {
            edit(entry);
        }
    }
}

/// Stores `v` into the matching sparse override slot of `overrides` (Pro-mode
/// edit; the parameter becomes pinned).
pub fn apply_override(overrides: &mut PresetParams, id: ParamId, v: f32) {
    let v = clamp_param(id, v);
    match id {
        ParamId::Bypass => overrides.bypass = Some(v >= 0.5),
        ParamId::InputGainDb => overrides.input_gain_db = Some(v),
        ParamId::HpfEnabled => overrides.hpf_enabled = Some(v >= 0.5),
        ParamId::HpfFreqHz => overrides.hpf_freq_hz = Some(v),
        ParamId::DenoiseMix => overrides.denoise_mix = Some(v),
        ParamId::GateEnabled => overrides.gate_enabled = Some(v >= 0.5),
        ParamId::GateThresholdDb => overrides.gate_threshold_db = Some(v),
        ParamId::GateRatio => overrides.gate_ratio = Some(v),
        ParamId::GateHysteresisDb => overrides.gate_hysteresis_db = Some(v),
        ParamId::GateAttackMs => overrides.gate_attack_ms = Some(v),
        ParamId::GateReleaseMs => overrides.gate_release_ms = Some(v),
        ParamId::EqType(b) => eq_edit(overrides, b, |e| {
            e.filter_type = Some(FilterType::from_f32(v));
        }),
        ParamId::EqFreqHz(b) => eq_edit(overrides, b, |e| {
            e.freq_hz = Some(v);
        }),
        ParamId::EqGainDb(b) => eq_edit(overrides, b, |e| {
            e.gain_db = Some(v);
        }),
        ParamId::EqQ(b) => eq_edit(overrides, b, |e| {
            e.q = Some(v);
        }),
        ParamId::DeesserEnabled => overrides.deesser_enabled = Some(v >= 0.5),
        ParamId::DeesserFreqHz => overrides.deesser_freq_hz = Some(v),
        ParamId::DeesserThreshDb => overrides.deesser_thresh_db = Some(v),
        ParamId::DeesserAmountDb => overrides.deesser_amount_db = Some(v),
        ParamId::CompEnabled => overrides.comp_enabled = Some(v >= 0.5),
        ParamId::CompThreshDb => overrides.comp_thresh_db = Some(v),
        ParamId::CompRatio => overrides.comp_ratio = Some(v),
        ParamId::CompAttackMs => overrides.comp_attack_ms = Some(v),
        ParamId::CompReleaseMs => overrides.comp_release_ms = Some(v),
        ParamId::CompKneeDb => overrides.comp_knee_db = Some(v),
        ParamId::CompMakeupDb => overrides.comp_makeup_db = Some(v),
        ParamId::CompAutoMakeup => overrides.comp_auto_makeup = Some(v >= 0.5),
        ParamId::LimiterCeilingDb => overrides.limiter_ceiling_db = Some(v),
        ParamId::LimiterTruePeak => overrides.limiter_true_peak = Some(v >= 0.5),
    }
}

/// Clears the matching sparse override slot of `overrides` (unpinning returns
/// the parameter to macro control).
pub fn clear_override(overrides: &mut PresetParams, id: ParamId) {
    match id {
        ParamId::Bypass => overrides.bypass = None,
        ParamId::InputGainDb => overrides.input_gain_db = None,
        ParamId::HpfEnabled => overrides.hpf_enabled = None,
        ParamId::HpfFreqHz => overrides.hpf_freq_hz = None,
        ParamId::DenoiseMix => overrides.denoise_mix = None,
        ParamId::GateEnabled => overrides.gate_enabled = None,
        ParamId::GateThresholdDb => overrides.gate_threshold_db = None,
        ParamId::GateRatio => overrides.gate_ratio = None,
        ParamId::GateHysteresisDb => overrides.gate_hysteresis_db = None,
        ParamId::GateAttackMs => overrides.gate_attack_ms = None,
        ParamId::GateReleaseMs => overrides.gate_release_ms = None,
        ParamId::EqType(b) => eq_edit(overrides, b, |e| e.filter_type = None),
        ParamId::EqFreqHz(b) => eq_edit(overrides, b, |e| e.freq_hz = None),
        ParamId::EqGainDb(b) => eq_edit(overrides, b, |e| e.gain_db = None),
        ParamId::EqQ(b) => eq_edit(overrides, b, |e| e.q = None),
        ParamId::DeesserEnabled => overrides.deesser_enabled = None,
        ParamId::DeesserFreqHz => overrides.deesser_freq_hz = None,
        ParamId::DeesserThreshDb => overrides.deesser_thresh_db = None,
        ParamId::DeesserAmountDb => overrides.deesser_amount_db = None,
        ParamId::CompEnabled => overrides.comp_enabled = None,
        ParamId::CompThreshDb => overrides.comp_thresh_db = None,
        ParamId::CompRatio => overrides.comp_ratio = None,
        ParamId::CompAttackMs => overrides.comp_attack_ms = None,
        ParamId::CompReleaseMs => overrides.comp_release_ms = None,
        ParamId::CompKneeDb => overrides.comp_knee_db = None,
        ParamId::CompMakeupDb => overrides.comp_makeup_db = None,
        ParamId::CompAutoMakeup => overrides.comp_auto_makeup = None,
        ParamId::LimiterCeilingDb => overrides.limiter_ceiling_db = None,
        ParamId::LimiterTruePeak => overrides.limiter_true_peak = None,
    }
}

#[cfg(test)]
mod tests {
    use super::{AppModel, KeepPins, Macro, apply_override, clamp_param, clear_override, resolve};
    use tine_protocol::params::{Band, DspParams, ParamId};
    use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, EndpointProfile, MacroPositions, PinSet};

    fn model() -> AppModel {
        AppModel::new(super::EndpointId::new("0.0.1.00000000.{test}"))
    }

    #[test]
    fn neutral_macros_keep_neutral_params() {
        let m = model();
        let p = resolve(&m);
        // Macro midpoint ≈ neutral curve (temporary mapping, TODO(T10)).
        assert_eq!(p.denoise_mix, 0.5, "purity 0.5 keeps default denoise mix");
        assert!(
            (p.gate_threshold_db - (-45.0)).abs() < f32::EPSILON,
            "purity 0.5 lands on the neutral gate threshold"
        );
        assert_eq!(p.comp_ratio, 3.25, "leveler 0.5 lands mid-range 1.5..5");
        assert!(p.comp_auto_makeup, "leveler enables auto makeup");
    }

    #[test]
    fn each_macro_moves_its_params_monotonically() {
        for mac in Macro::ALL {
            let mut m = model();
            Macro::set(mac, &mut m.profile.macros, 0.0);
            let low = resolve(&m);
            Macro::set(mac, &mut m.profile.macros, 1.0);
            let high = resolve(&m);
            // Pick one representative metric per macro.
            let (id, increasing) = match mac {
                Macro::Purity => (ParamId::DenoiseMix, true),
                Macro::Body => (ParamId::EqGainDb(Band::B0), true),
                Macro::Presence => (ParamId::EqGainDb(Band::B3), true),
                Macro::Leveler => (ParamId::CompRatio, true),
            };
            let a = low.get(id);
            let b = high.get(id);
            assert!(b >= a, "{mac:?} should not decrease {id:?}");
            assert!(b > a, "{mac:?} should move {id:?}");
            if increasing {
                assert!(b > a);
            }
        }
    }

    #[test]
    fn macros_drive_expected_params() {
        let mut m = model();
        Macro::set(Macro::Purity, &mut m.profile.macros, 1.0);
        let p = resolve(&m);
        assert_eq!(p.denoise_mix, 1.0);
        assert!((p.gate_threshold_db - (-30.0)).abs() < 1e-4, "purity 1.0 -> -30 dB");

        let mut m = model();
        Macro::set(Macro::Body, &mut m.profile.macros, 1.0);
        let p = resolve(&m);
        assert!((p.eq_gain_db[Band::B0.index()] - 6.0).abs() < f32::EPSILON);
        assert!((p.eq_gain_db[Band::B1.index()] - (-5.0)).abs() < f32::EPSILON);

        let mut m = model();
        Macro::set(Macro::Presence, &mut m.profile.macros, 0.8);
        let p = resolve(&m);
        assert!(p.deesser_enabled, "presence > 0.6 engages the de-esser");
        assert!((p.deesser_amount_db - 5.5).abs() < 0.01);
        assert!(!resolve(&{
            let mut m = model();
            Macro::set(Macro::Presence, &mut m.profile.macros, 0.5);
            m
        })
        .deesser_enabled);

        let mut m = model();
        Macro::set(Macro::Leveler, &mut m.profile.macros, 1.0);
        let p = resolve(&m);
        assert!((p.comp_ratio - 5.0).abs() < f32::EPSILON);
        assert!((p.comp_thresh_db - (-30.0)).abs() < 1e-3, "leveler 1.0 lowers threshold by 10 dB");
    }

    #[test]
    fn pins_stop_macros_from_driving_params() {
        let mut m = model();
        m.profile.pins.pin(ParamId::DenoiseMix);
        // Pinned parameters keep their calibrated/overridden value.
        apply_override(
            &mut m.profile.overrides,
            ParamId::DenoiseMix,
            0.25,
        );
        Macro::set(Macro::Purity, &mut m.profile.macros, 0.9);
        let p = resolve(&m);
        assert!((p.denoise_mix - 0.25).abs() < f32::EPSILON, "pinned param not driven by macro");

        // Unpinning returns control to macros.
        m.profile.pins.unpin(ParamId::DenoiseMix);
        clear_override(&mut m.profile.overrides, ParamId::DenoiseMix);
        let p = resolve(&m);
        assert!((p.denoise_mix - 0.9).abs() < f32::EPSILON);
    }

    #[test]
    fn calibration_drives_measured_params() {
        let m = model();
        let mut m = m;
        m.profile.calibration = Some(Calibration {
            noise_rms_db: -60.0,
            noise_peak_db: -55.0,
            speech_rms_db: -28.0,
            crest_db: 12.0,
            tilt_db: 3.0,
            derived: CalibrationOverrides {
                input_gain_db: 10.0,
                gate_threshold_db: -54.0,
                denoise_mix: 0.5,
                comp_thresh_db: -16.0,
                comp_makeup_db: 2.0,
            },
        });
        let p = resolve(&m);
        assert_eq!(p.input_gain_db, 10.0);
        // Leveler 0.5 offsets the calibrated threshold by -5 dB (temp mapping).
        assert!((p.comp_thresh_db - (-21.0)).abs() < 1e-3);
        assert_eq!(p.comp_makeup_db, 2.0);
        assert!(m.profile.calibration.is_some());
    }

    #[test]
    fn resolve_is_deterministic_and_clamped() {
        let mut m = model();
        m.profile.macros.purity = f32::NAN;
        m.profile.macros.presence = 7.0; // out of range; clamped to 1.0
        let p = resolve(&m);
        assert!((p.denoise_mix - 0.5).abs() < f32::EPSILON, "NaN macro -> neutral");
        assert!((p.eq_gain_db[Band::B3.index()] - 5.0).abs() < f32::EPSILON, "clamped to 1.0");
    }

    #[test]
    fn bypass_is_applied_last() {
        let mut m = model();
        m.bypass = true;
        let p = resolve(&m);
        assert!(p.bypass);
    }

    #[test]
    fn archetype_switch_keeps_pins_per_d007() {
        let mut m = model();
        m.profile.pins.pin(ParamId::InputGainDb);
        apply_override(&mut m.profile.overrides, ParamId::InputGainDb, 6.0);
        m.profile.archetype = Archetype::Condenser;
        assert!(m.profile.pins.is_pinned(ParamId::InputGainDb));
        let p = resolve(&m);
        assert_eq!(p.input_gain_db, 6.0, "override survives the archetype switch");
    }

    #[test]
    fn apply_and_clear_override_round_trip_eq_band_and_scalar() {
        let mut profile = EndpointProfile::new("e", Archetype::Headset);
        let mut overrides = std::mem::take(&mut profile.overrides);

        apply_override(&mut overrides, ParamId::EqGainDb(Band::B2), -4.5);
        apply_override(&mut overrides, ParamId::EqQ(Band::B2), 2.0);
        assert_eq!(overrides.eq.len(), 1, "same band reuses one entry");
        assert_eq!(overrides.eq[0].band, Some(2));
        assert_eq!(overrides.eq[0].gain_db, Some(-4.5));
        assert_eq!(overrides.eq[0].q, Some(2.0));

        clear_override(&mut overrides, ParamId::EqGainDb(Band::B2));
        assert_eq!(overrides.eq[0].gain_db, None, "other fields of the band stay");
        clear_override(&mut overrides, ParamId::EqQ(Band::B2));
        assert_eq!(overrides.eq[0].q, None);

        apply_override(&mut overrides, ParamId::HpfFreqHz, 95.0);
        assert_eq!(overrides.hpf_freq_hz, Some(95.0));
        clear_override(&mut overrides, ParamId::HpfFreqHz);
        assert_eq!(overrides.hpf_freq_hz, None);
    }

    #[test]
    fn clamp_param_clamps_and_defaults_nonfinite() {
        assert_eq!(clamp_param(ParamId::HpfFreqHz, 500.0), 200.0);
        assert_eq!(clamp_param(ParamId::HpfFreqHz, 10.0), 20.0);
        assert_eq!(clamp_param(ParamId::HpfFreqHz, f32::NAN), 80.0);
        assert_eq!(clamp_param(ParamId::CompRatio, f32::INFINITY), 3.0, "non-finite -> default");
    }

    #[test]
    fn keep_pins_is_flag_like() {
        assert_ne!(KeepPins::Keep, KeepPins::Clear);
    }

    #[test]
    fn macro_names_round_trip() {
        for m in Macro::ALL {
            assert_eq!(Macro::from_name(m.name()), Some(m));
        }
        assert_eq!(Macro::from_name("nope"), None);
    }

    #[test]
    fn pinned_comp_auto_makeup_keeps_the_sampler_wide_default() {
        let mut m = model();
        m.profile.pins.pin(ParamId::CompAutoMakeup);
        let p = resolve(&m);
        // SPECS default for auto-makeup is `true`; a pin freezes the
        // pre-macro value, and the default snapshot already has it on.
        assert!(p.comp_auto_makeup, "pins freeze the pre-macro (default) value");
    }

    #[test]
    fn default_model_marks_and_positions() {
        let macros = MacroPositions::default();
        assert_eq!(macros.purity, 0.5);
        let mut m = model();
        m.profile.pins.pin(ParamId::Bypass);
        assert!(m.profile.pins.is_pinned(ParamId::Bypass));
        let pins: PinSet = m.profile.pins;
        assert!(!pins.is_empty());
        let _ = DspParams::DEFAULT.eq_gain_db[Band::B4.index()];
    }
}
