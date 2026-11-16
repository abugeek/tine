//! Microphone archetype profiles, macro positions, pin bitsets, calibration results,
//! and persisted endpoint profiles.

use crate::params::{PARAM_COUNT, ParamId};

const _: () = assert!(PARAM_COUNT <= 64, "PinSet(u64) requires PARAM_COUNT <= 64");

/// Hardware microphone archetype used to select the Layer A baseline DSP curve.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum Archetype {
    /// Dynamic USB / XLR broadcast mic (e.g., Fifine AM8, SM7B, `PodMic`).
    #[default]
    Dynamic,
    /// Studio condenser mic (e.g., `QuadCast`, Yeti, AT2020).
    Condenser,
    /// Gaming headset or boom mic.
    Headset,
    /// Laptop, webcam, or lavalier mic.
    Laptop,
}

impl Archetype {
    /// Stable string identifier matching `assets/presets/<id>.json`.
    #[must_use]
    pub const fn id(self) -> &'static str {
        match self {
            Self::Dynamic => "dynamic",
            Self::Condenser => "condenser",
            Self::Headset => "headset",
            Self::Laptop => "laptop",
        }
    }

    /// Human-readable display name.
    #[must_use]
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Dynamic => "Dynamic USB / XLR",
            Self::Condenser => "Studio Condenser",
            Self::Headset => "Gaming Headset / Boom",
            Self::Laptop => "Laptop / Webcam / Lav",
        }
    }
}

/// Normalized `[0.0, 1.0]` positions of the 4 "Golden Line" macro nodes.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(default))]
pub struct MacroPositions {
    /// Noise & Gate macro (`0.0..=1.0`).
    pub purity: f32,
    /// Warmth & Mud Control macro (`0.0..=1.0`).
    pub body: f32,
    /// Clarity & De-Ess macro (`0.0..=1.0`).
    pub presence: f32,
    /// Compression & Voice Focus macro (`0.0..=1.0`).
    pub leveler: f32,
}

impl Default for MacroPositions {
    fn default() -> Self {
        Self {
            purity: 0.5,
            body: 0.5,
            presence: 0.5,
            leveler: 0.5,
        }
    }
}

impl MacroPositions {
    /// Clamps all macro positions to `[0.0, 1.0]`, replacing non-finite values with `0.5`.
    #[must_use]
    pub fn clamped(self) -> Self {
        let clamp = |v: f32| if v.is_finite() { v.clamp(0.0, 1.0) } else { 0.5 };
        Self {
            purity: clamp(self.purity),
            body: clamp(self.body),
            presence: clamp(self.presence),
            leveler: clamp(self.leveler),
        }
    }
}

/// Bitset of parameters pinned by Pro-mode edits (D-007).
///
/// Pinned parameters are not overwritten when moving Golden Line macros.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[repr(transparent)]
pub struct PinSet(u64);

impl PinSet {
    /// Empty pin set (macros control all parameters).
    pub const EMPTY: Self = Self(0);

    /// Returns `true` if `id` is pinned.
    #[must_use]
    pub const fn is_pinned(self, id: ParamId) -> bool {
        (self.0 & (1u64 << id.index())) != 0
    }

    /// Marks `id` as pinned.
    pub const fn pin(&mut self, id: ParamId) {
        self.0 |= 1u64 << id.index();
    }

    /// Unpins `id`.
    pub const fn unpin(&mut self, id: ParamId) {
        self.0 &= !(1u64 << id.index());
    }

    /// Unpins all parameters.
    pub const fn clear(&mut self) {
        self.0 = 0;
    }

    /// Returns `true` if no parameters are pinned.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Returns the raw `u64` bitmask.
    #[must_use]
    pub const fn bits(self) -> u64 {
        self.0
    }

    /// Constructs a [`PinSet`] from a raw `u64` bitmask (masking bits `>= PARAM_COUNT`).
    #[must_use]
    pub const fn from_bits(bits: u64) -> Self {
        let mask = if PARAM_COUNT == 64 {
            u64::MAX
        } else {
            (1u64 << PARAM_COUNT) - 1
        };
        Self(bits & mask)
    }
}

/// Parameters derived by the 8-second Auto-Tune wizard.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct CalibrationOverrides {
    /// Calibrated pre-gain to bring speech RMS to `-18 dBFS`.
    pub input_gain_db: f32,
    /// Calibrated expander threshold (`noise_rms_db + 6 dB`).
    pub gate_threshold_db: f32,
    /// Calibrated `RNNoise` mix (`0.85` if noisy, else `0.50`).
    pub denoise_mix: f32,
    /// Calibrated compressor threshold (`vocal_rms_after_gain + 2 dB`).
    pub comp_thresh_db: f32,
    /// Calibrated compressor makeup gain (verified by offline simulation).
    pub comp_makeup_db: f32,
}

/// Output of the 8-second Auto-Tune wizard stored per endpoint.
///
/// Plain data: always available (including `no_std` consumers such as
/// `tine-dsp::macros`), independent of the optional `serde` feature.
/// Serialization support is added by `serde`-aware consumers via the
/// `cfg_attr` derives below; validation (which allocates) stays gated.
#[derive(Clone, Copy, Debug, PartialEq)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct Calibration {
    /// Ambient room noise RMS (dBFS).
    pub noise_rms_db: f32,
    /// Ambient room noise peak (dBFS).
    pub noise_peak_db: f32,
    /// Integrated voiced speech RMS (dBFS).
    pub speech_rms_db: f32,
    /// Measured vocal crest factor (dB).
    pub crest_db: f32,
    /// Spectral tilt (`E(<350 Hz) / E(1-6 kHz)` in dB).
    pub tilt_db: f32,
    /// Derived DSP overrides.
    pub derived: CalibrationOverrides,
}

#[cfg(feature = "serde")]
impl Calibration {
    /// Validates that all measured and derived calibration values are finite and within range.
    pub fn validate(&self) -> Result<(), serde_types::ValidationError> {
        use serde_types::{ValidationError, check_param, check_range};

        let mut errors = Vec::new();
        check_range(&mut errors, "calibration.noise_rms_db", self.noise_rms_db, -120.0, 0.0);
        check_range(
            &mut errors,
            "calibration.noise_peak_db",
            self.noise_peak_db,
            -120.0,
            6.0,
        );
        check_range(
            &mut errors,
            "calibration.speech_rms_db",
            self.speech_rms_db,
            -120.0,
            0.0,
        );
        check_range(&mut errors, "calibration.crest_db", self.crest_db, 0.0, 40.0);
        check_range(&mut errors, "calibration.tilt_db", self.tilt_db, -40.0, 40.0);

        check_param(&mut errors, ParamId::InputGainDb, Some(self.derived.input_gain_db));
        check_param(
            &mut errors,
            ParamId::GateThresholdDb,
            Some(self.derived.gate_threshold_db),
        );
        check_param(&mut errors, ParamId::DenoiseMix, Some(self.derived.denoise_mix));
        check_param(&mut errors, ParamId::CompThreshDb, Some(self.derived.comp_thresh_db));
        check_param(&mut errors, ParamId::CompMakeupDb, Some(self.derived.comp_makeup_db));

        if errors.is_empty() {
            Ok(())
        } else {
            Err(ValidationError::OutOfRange(errors))
        }
    }
}

#[cfg(feature = "serde")]
mod serde_types {
    use super::{Archetype, Calibration, MacroPositions, PinSet};
    use crate::ids::SCHEMA_VERSION;
    use crate::params::{Band, DspParams, EQ_BAND_COUNT, FilterType, ParamId};

    /// Single out-of-range numeric field error.
    #[derive(Clone, Debug, PartialEq)]
    pub struct FieldError {
        /// Machine field key.
        pub field: &'static str,
        /// Invalid value observed.
        pub value: f32,
        /// Minimum allowed value.
        pub min: f32,
        /// Maximum allowed value.
        pub max: f32,
    }

    /// Validation error returned when loading a [`Preset`], [`EndpointProfile`], or [`crate::settings::AppSettings`].
    #[derive(Clone, Debug, PartialEq)]
    pub enum ValidationError {
        /// File schema version is newer than [`SCHEMA_VERSION`] or 0.
        UnsupportedSchema {
            /// Schema version found in the file.
            actual: u32,
            /// Maximum schema version supported by this build.
            max_supported: u32,
        },
        /// Required string field was empty.
        EmptyField {
            /// Field name.
            field: &'static str,
        },
        /// More than [`EQ_BAND_COUNT`] EQ overrides were specified.
        TooManyEqBands {
            /// Number of EQ band entries present.
            count: usize,
            /// Maximum allowed (`5`).
            max: usize,
        },
        /// Explicit EQ band index was outside `0..5`.
        InvalidEqBandIndex {
            /// Invalid band index.
            band: u8,
        },
        /// One or more numeric fields were non-finite or outside their valid range.
        OutOfRange(Vec<FieldError>),
    }

    impl core::fmt::Display for ValidationError {
        fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
            match self {
                Self::UnsupportedSchema { actual, max_supported } => {
                    write!(f, "unsupported schema version {actual} (max supported {max_supported})")
                }
                Self::EmptyField { field } => write!(f, "required field '{field}' is empty"),
                Self::TooManyEqBands { count, max } => {
                    write!(f, "too many EQ bands: {count} (max {max})")
                }
                Self::InvalidEqBandIndex { band } => {
                    write!(f, "invalid EQ band index {band} (expected 0..{EQ_BAND_COUNT})")
                }
                Self::OutOfRange(errors) => {
                    write!(f, "{} field(s) out of range: ", errors.len())?;
                    for (i, e) in errors.iter().enumerate() {
                        if i > 0 {
                            f.write_str(", ")?;
                        }
                        write!(f, "{}={} (expected {}..={})", e.field, e.value, e.min, e.max)?;
                    }
                    Ok(())
                }
            }
        }
    }

    impl std::error::Error for ValidationError {}

    pub(crate) fn check_range(errors: &mut Vec<FieldError>, field: &'static str, value: f32, min: f32, max: f32) {
        if !value.is_finite() || value < min || value > max {
            errors.push(FieldError { field, value, min, max });
        }
    }

    pub(crate) fn check_param(errors: &mut Vec<FieldError>, id: ParamId, value: Option<f32>) {
        if let Some(v) = value {
            let spec = id.spec();
            check_range(errors, spec.key, v, spec.min, spec.max);
        }
    }

    /// Sparse EQ band override inside [`PresetParams`].
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct EqBandOverride {
        /// Optional explicit band index (`0..5`); if `None`, uses positional index in the `eq` array.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub band: Option<u8>,
        /// Filter topology override.
        #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
        pub filter_type: Option<FilterType>,
        /// Center/corner frequency in Hz.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub freq_hz: Option<f32>,
        /// Gain in dB.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub gain_db: Option<f32>,
        /// Quality factor (Q).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub q: Option<f32>,
    }

    /// Sparse parameter overrides for presets and Pro-mode user overrides.
    #[derive(Clone, Debug, Default, PartialEq, serde::Serialize, serde::Deserialize)]
    #[serde(default)]
    pub struct PresetParams {
        /// Bypass override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub bypass: Option<bool>,
        /// Input gain override (dB).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub input_gain_db: Option<f32>,
        /// HPF enable override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub hpf_enabled: Option<bool>,
        /// HPF cutoff override (Hz).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub hpf_freq_hz: Option<f32>,
        /// `RNNoise` mix override (`0.0..=1.0`).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub denoise_mix: Option<f32>,
        /// Gate enable override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_enabled: Option<bool>,
        /// Gate threshold override (dBFS).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_threshold_db: Option<f32>,
        /// Gate ratio override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_ratio: Option<f32>,
        /// Gate hysteresis override (dB).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_hysteresis_db: Option<f32>,
        /// Gate attack override (ms).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_attack_ms: Option<f32>,
        /// Gate release override (ms).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub gate_release_ms: Option<f32>,
        /// Sparse EQ band overrides.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        pub eq: Vec<EqBandOverride>,
        /// De-esser enable override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub deesser_enabled: Option<bool>,
        /// De-esser frequency override (Hz).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub deesser_freq_hz: Option<f32>,
        /// De-esser threshold override (dBFS).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub deesser_thresh_db: Option<f32>,
        /// De-esser amount override (dB).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub deesser_amount_db: Option<f32>,
        /// Compressor enable override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_enabled: Option<bool>,
        /// Compressor threshold override (dBFS).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_thresh_db: Option<f32>,
        /// Compressor ratio override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_ratio: Option<f32>,
        /// Compressor attack override (ms).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_attack_ms: Option<f32>,
        /// Compressor release override (ms).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_release_ms: Option<f32>,
        /// Compressor knee override (dB).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_knee_db: Option<f32>,
        /// Compressor makeup override (dB).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_makeup_db: Option<f32>,
        /// Compressor auto-makeup override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub comp_auto_makeup: Option<bool>,
        /// Limiter ceiling override (dBFS).
        #[serde(skip_serializing_if = "Option::is_none")]
        pub limiter_ceiling_db: Option<f32>,
        /// Limiter true-peak override.
        #[serde(skip_serializing_if = "Option::is_none")]
        pub limiter_true_peak: Option<bool>,
    }

    impl PresetParams {
        /// Validates all populated fields against [`crate::params::SPECS`].
        pub fn validate(&self) -> Result<(), ValidationError> {
            if self.eq.len() > EQ_BAND_COUNT {
                return Err(ValidationError::TooManyEqBands {
                    count: self.eq.len(),
                    max: EQ_BAND_COUNT,
                });
            }

            let mut errors = Vec::new();
            check_param(&mut errors, ParamId::InputGainDb, self.input_gain_db);
            check_param(&mut errors, ParamId::HpfFreqHz, self.hpf_freq_hz);
            check_param(&mut errors, ParamId::DenoiseMix, self.denoise_mix);
            check_param(&mut errors, ParamId::GateThresholdDb, self.gate_threshold_db);
            check_param(&mut errors, ParamId::GateRatio, self.gate_ratio);
            check_param(&mut errors, ParamId::GateHysteresisDb, self.gate_hysteresis_db);
            check_param(&mut errors, ParamId::GateAttackMs, self.gate_attack_ms);
            check_param(&mut errors, ParamId::GateReleaseMs, self.gate_release_ms);

            for (pos, entry) in self.eq.iter().enumerate() {
                let raw_idx = entry.band.unwrap_or(pos as u8);
                let Some(band) = Band::new(raw_idx) else {
                    return Err(ValidationError::InvalidEqBandIndex { band: raw_idx });
                };
                check_param(&mut errors, ParamId::EqFreqHz(band), entry.freq_hz);
                check_param(&mut errors, ParamId::EqGainDb(band), entry.gain_db);
                check_param(&mut errors, ParamId::EqQ(band), entry.q);
            }

            check_param(&mut errors, ParamId::DeesserFreqHz, self.deesser_freq_hz);
            check_param(&mut errors, ParamId::DeesserThreshDb, self.deesser_thresh_db);
            check_param(&mut errors, ParamId::DeesserAmountDb, self.deesser_amount_db);
            check_param(&mut errors, ParamId::CompThreshDb, self.comp_thresh_db);
            check_param(&mut errors, ParamId::CompRatio, self.comp_ratio);
            check_param(&mut errors, ParamId::CompAttackMs, self.comp_attack_ms);
            check_param(&mut errors, ParamId::CompReleaseMs, self.comp_release_ms);
            check_param(&mut errors, ParamId::CompKneeDb, self.comp_knee_db);
            check_param(&mut errors, ParamId::CompMakeupDb, self.comp_makeup_db);
            check_param(&mut errors, ParamId::LimiterCeilingDb, self.limiter_ceiling_db);

            if errors.is_empty() {
                Ok(())
            } else {
                Err(ValidationError::OutOfRange(errors))
            }
        }

        /// Applies all present overrides onto `dst`, clamping values to valid ranges.
        pub fn apply_to(&self, dst: &mut DspParams) {
            if let Some(v) = self.bypass {
                dst.bypass = v;
            }
            if let Some(v) = self.input_gain_db {
                dst.set(ParamId::InputGainDb, v);
            }
            if let Some(v) = self.hpf_enabled {
                dst.hpf_enabled = v;
            }
            if let Some(v) = self.hpf_freq_hz {
                dst.set(ParamId::HpfFreqHz, v);
            }
            if let Some(v) = self.denoise_mix {
                dst.set(ParamId::DenoiseMix, v);
            }
            if let Some(v) = self.gate_enabled {
                dst.gate_enabled = v;
            }
            if let Some(v) = self.gate_threshold_db {
                dst.set(ParamId::GateThresholdDb, v);
            }
            if let Some(v) = self.gate_ratio {
                dst.set(ParamId::GateRatio, v);
            }
            if let Some(v) = self.gate_hysteresis_db {
                dst.set(ParamId::GateHysteresisDb, v);
            }
            if let Some(v) = self.gate_attack_ms {
                dst.set(ParamId::GateAttackMs, v);
            }
            if let Some(v) = self.gate_release_ms {
                dst.set(ParamId::GateReleaseMs, v);
            }
            for (pos, entry) in self.eq.iter().enumerate() {
                let raw_idx = entry.band.unwrap_or(pos as u8);
                if let Some(band) = Band::new(raw_idx) {
                    if let Some(ft) = entry.filter_type
                        && let Some(slot) = dst.eq_type.get_mut(band.index())
                    {
                        *slot = ft;
                    }
                    if let Some(v) = entry.freq_hz {
                        dst.set(ParamId::EqFreqHz(band), v);
                    }
                    if let Some(v) = entry.gain_db {
                        dst.set(ParamId::EqGainDb(band), v);
                    }
                    if let Some(v) = entry.q {
                        dst.set(ParamId::EqQ(band), v);
                    }
                }
            }
            if let Some(v) = self.deesser_enabled {
                dst.deesser_enabled = v;
            }
            if let Some(v) = self.deesser_freq_hz {
                dst.set(ParamId::DeesserFreqHz, v);
            }
            if let Some(v) = self.deesser_thresh_db {
                dst.set(ParamId::DeesserThreshDb, v);
            }
            if let Some(v) = self.deesser_amount_db {
                dst.set(ParamId::DeesserAmountDb, v);
            }
            if let Some(v) = self.comp_enabled {
                dst.comp_enabled = v;
            }
            if let Some(v) = self.comp_thresh_db {
                dst.set(ParamId::CompThreshDb, v);
            }
            if let Some(v) = self.comp_ratio {
                dst.set(ParamId::CompRatio, v);
            }
            if let Some(v) = self.comp_attack_ms {
                dst.set(ParamId::CompAttackMs, v);
            }
            if let Some(v) = self.comp_release_ms {
                dst.set(ParamId::CompReleaseMs, v);
            }
            if let Some(v) = self.comp_knee_db {
                dst.set(ParamId::CompKneeDb, v);
            }
            if let Some(v) = self.comp_makeup_db {
                dst.set(ParamId::CompMakeupDb, v);
            }
            if let Some(v) = self.comp_auto_makeup {
                dst.comp_auto_makeup = v;
            }
            if let Some(v) = self.limiter_ceiling_db {
                dst.set(ParamId::LimiterCeilingDb, v);
            }
            if let Some(v) = self.limiter_true_peak {
                dst.limiter_true_peak = v;
            }
        }
    }

    /// Versioned microphone archetype preset JSON schema (`assets/presets/*.json`).
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct Preset {
        /// Schema version (must be `1..=SCHEMA_VERSION`).
        pub schema: u32,
        /// Stable preset identifier (e.g. `"dynamic"`).
        pub id: String,
        /// Human-readable preset name.
        pub name: String,
        /// Sparse baseline parameter overrides.
        #[serde(default)]
        pub params: PresetParams,
        /// Default macro positions for this archetype.
        #[serde(default)]
        pub macro_defaults: MacroPositions,
    }

    impl Preset {
        /// Validates schema version, required identifiers, macro bounds, and parameter ranges.
        pub fn validate(&self) -> Result<(), ValidationError> {
            if self.schema == 0 || self.schema > SCHEMA_VERSION {
                return Err(ValidationError::UnsupportedSchema {
                    actual: self.schema,
                    max_supported: SCHEMA_VERSION,
                });
            }
            if self.id.trim().is_empty() {
                return Err(ValidationError::EmptyField { field: "id" });
            }
            if self.name.trim().is_empty() {
                return Err(ValidationError::EmptyField { field: "name" });
            }
            self.params.validate()?;

            let mut errors = Vec::new();
            check_range(
                &mut errors,
                "macro_defaults.purity",
                self.macro_defaults.purity,
                0.0,
                1.0,
            );
            check_range(&mut errors, "macro_defaults.body", self.macro_defaults.body, 0.0, 1.0);
            check_range(
                &mut errors,
                "macro_defaults.presence",
                self.macro_defaults.presence,
                0.0,
                1.0,
            );
            check_range(
                &mut errors,
                "macro_defaults.leveler",
                self.macro_defaults.leveler,
                0.0,
                1.0,
            );
            if !errors.is_empty() {
                return Err(ValidationError::OutOfRange(errors));
            }
            Ok(())
        }
    }

    /// Persisted per-microphone profile (`%APPDATA%\Tine\profiles\<endpoint>.json`).
    #[derive(Clone, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
    pub struct EndpointProfile {
        /// Schema version (`1..=SCHEMA_VERSION`).
        pub schema: u32,
        /// Windows `MMDevice` endpoint ID.
        pub endpoint_id: String,
        /// Selected hardware archetype.
        #[serde(default)]
        pub archetype: Archetype,
        /// Golden Line macro positions.
        #[serde(default)]
        pub macros: MacroPositions,
        /// Pinned Pro-mode parameters.
        #[serde(default)]
        pub pins: PinSet,
        /// Optional Auto-Tune calibration result.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        pub calibration: Option<Calibration>,
        /// Sparse Pro-mode parameter overrides.
        #[serde(default)]
        pub overrides: PresetParams,
    }

    impl EndpointProfile {
        /// Creates a fresh profile for `endpoint_id` with neutral defaults.
        #[must_use]
        pub fn new(endpoint_id: impl Into<String>, archetype: Archetype) -> Self {
            Self {
                schema: SCHEMA_VERSION,
                endpoint_id: endpoint_id.into(),
                archetype,
                macros: MacroPositions::default(),
                pins: PinSet::EMPTY,
                calibration: None,
                overrides: PresetParams::default(),
            }
        }

        /// Validates schema, endpoint ID, macros, calibration, and overrides.
        pub fn validate(&self) -> Result<(), ValidationError> {
            if self.schema == 0 || self.schema > SCHEMA_VERSION {
                return Err(ValidationError::UnsupportedSchema {
                    actual: self.schema,
                    max_supported: SCHEMA_VERSION,
                });
            }
            if self.endpoint_id.trim().is_empty() {
                return Err(ValidationError::EmptyField { field: "endpoint_id" });
            }
            if let Some(cal) = &self.calibration {
                cal.validate()?;
            }
            self.overrides.validate()?;

            let mut errors = Vec::new();
            check_range(&mut errors, "macros.purity", self.macros.purity, 0.0, 1.0);
            check_range(&mut errors, "macros.body", self.macros.body, 0.0, 1.0);
            check_range(&mut errors, "macros.presence", self.macros.presence, 0.0, 1.0);
            check_range(&mut errors, "macros.leveler", self.macros.leveler, 0.0, 1.0);
            if !errors.is_empty() {
                return Err(ValidationError::OutOfRange(errors));
            }
            Ok(())
        }
    }
}

#[cfg(feature = "serde")]
pub use serde_types::{EndpointProfile, EqBandOverride, FieldError, Preset, PresetParams, ValidationError};
