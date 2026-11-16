//! Parameter definitions, ranges, defaults, and [`DspParams`].
//!
//! This module is the single source of truth for every DSP parameter across
//! `tine-dsp`, `tine-apo`, `tine-engine`, and `tine-ui`.

/// Number of parametric EQ bands in Stage 3.
pub const EQ_BAND_COUNT: usize = 5;

/// Total number of scalar parameters (with EQ bands expanded).
pub const PARAM_COUNT: usize = 45;

/// Validated EQ band index (`0..5`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Band(u8);

impl Band {
    /// Total number of EQ bands.
    pub const COUNT: usize = EQ_BAND_COUNT;

    /// Band 0: Low-shelf (Warmth / body, default 110 Hz).
    pub const B0: Self = Self(0);
    /// Band 1: Peaking (Mud cut, default 320 Hz).
    pub const B1: Self = Self(1);
    /// Band 2: Peaking (Nasal / mid, default 950 Hz).
    pub const B2: Self = Self(2);
    /// Band 3: Peaking (Presence, default 4.2 kHz).
    pub const B3: Self = Self(3);
    /// Band 4: High-shelf (Air, default 8.5 kHz).
    pub const B4: Self = Self(4);

    /// Constructs a [`Band`] if `index < 5`.
    #[must_use]
    pub const fn new(index: u8) -> Option<Self> {
        if (index as usize) < Self::COUNT {
            Some(Self(index))
        } else {
            None
        }
    }

    /// Returns the zero-based band index (`0..5`).
    #[must_use]
    pub const fn index(self) -> usize {
        self.0 as usize
    }

    /// Returns the raw `u8` band index (`0..5`).
    #[must_use]
    pub const fn as_u8(self) -> u8 {
        self.0
    }
}

/// Filter topology for each parametric EQ band.
#[repr(u32)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
pub enum FilterType {
    /// Low-shelving filter.
    LowShelf = 0,
    /// High-shelving filter.
    HighShelf = 1,
    /// Parametric peaking EQ filter.
    #[default]
    Peaking = 2,
    /// Notch (band-reject) filter.
    Notch = 3,
    /// 2nd-order low-pass filter.
    LowPass = 4,
    /// 2nd-order high-pass filter.
    HighPass = 5,
    /// 2nd-order band-pass filter.
    BandPass = 6,
}

impl FilterType {
    /// Converts a raw `u32` discriminant to [`FilterType`].
    #[must_use]
    pub const fn from_u32(v: u32) -> Option<Self> {
        match v {
            0 => Some(Self::LowShelf),
            1 => Some(Self::HighShelf),
            2 => Some(Self::Peaking),
            3 => Some(Self::Notch),
            4 => Some(Self::LowPass),
            5 => Some(Self::HighPass),
            6 => Some(Self::BandPass),
            _ => None,
        }
    }

    /// Converts a float representation (`0.0..=6.0`) to [`FilterType`], clamping and
    /// falling back to [`FilterType::Peaking`] for non-finite values.
    #[must_use]
    pub fn from_f32(v: f32) -> Self {
        if !v.is_finite() {
            return Self::Peaking;
        }
        let clamped = v.clamp(0.0, 6.0);
        let idx = (clamped + 0.5) as u32;
        Self::from_u32(idx).unwrap_or(Self::Peaking)
    }

    /// Returns the numeric `f32` representation (`0.0..=6.0`).
    #[must_use]
    pub const fn as_f32(self) -> f32 {
        (self as u32) as f32
    }
}

/// Measurement unit for a parameter.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Unit {
    /// Dimensionless / boolean / enum.
    None,
    /// Relative decibels (dB).
    Db,
    /// Decibels relative to full scale (dBFS).
    Dbfs,
    /// Hertz (Hz).
    Hz,
    /// Milliseconds (ms).
    Ms,
    /// Compression / expansion ratio (`:1`).
    Ratio,
}

/// UI slider taper / control mapping.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Scale {
    /// Linear mapping between `min` and `max`.
    Linear,
    /// Logarithmic mapping (frequencies, time constants).
    Log,
    /// Boolean toggle (`0.0` or `1.0`).
    Bool,
    /// Discrete enum (`0.0..=6.0`).
    Enum,
}

/// Identifier for every scalar parameter in [`DspParams`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ParamId {
    /// Global A/B bypass.
    Bypass,
    /// Stage 1: Input gain (dB).
    InputGainDb,
    /// Stage 1: High-pass filter enable.
    HpfEnabled,
    /// Stage 1: High-pass filter cutoff frequency (Hz).
    HpfFreqHz,
    /// Stage 2: `RNNoise` AI denoiser wet/dry mix (`0.0..=1.0`).
    DenoiseMix,
    /// Stage 2: Downward expander / gate enable.
    GateEnabled,
    /// Stage 2: Gate open threshold (dBFS).
    GateThresholdDb,
    /// Stage 2: Gate expansion ratio (`:1`).
    GateRatio,
    /// Stage 2: Gate close hysteresis (dB).
    GateHysteresisDb,
    /// Stage 2: Gate attack time (ms).
    GateAttackMs,
    /// Stage 2: Gate release time (ms).
    GateReleaseMs,
    /// Stage 3: EQ band filter type.
    EqType(Band),
    /// Stage 3: EQ band center/corner frequency (Hz).
    EqFreqHz(Band),
    /// Stage 3: EQ band gain (dB).
    EqGainDb(Band),
    /// Stage 3: EQ band Q factor.
    EqQ(Band),
    /// Stage 4: Split-band de-esser enable.
    DeesserEnabled,
    /// Stage 4: De-esser crossover / sidechain frequency (Hz).
    DeesserFreqHz,
    /// Stage 4: De-esser threshold (dBFS).
    DeesserThreshDb,
    /// Stage 4: De-esser maximum gain reduction (dB).
    DeesserAmountDb,
    /// Stage 5: Compressor enable.
    CompEnabled,
    /// Stage 5: Compressor threshold (dBFS).
    CompThreshDb,
    /// Stage 5: Compressor ratio (`:1`).
    CompRatio,
    /// Stage 5: Compressor attack time (ms).
    CompAttackMs,
    /// Stage 5: Compressor release time (ms).
    CompReleaseMs,
    /// Stage 5: Compressor soft-knee width (dB).
    CompKneeDb,
    /// Stage 5: Compressor manual makeup gain (dB).
    CompMakeupDb,
    /// Stage 5: Compressor automatic makeup gain enable.
    CompAutoMakeup,
    /// Stage 6: Lookahead limiter ceiling (dBFS).
    LimiterCeilingDb,
    /// Stage 6: 4x oversampled true-peak detection enable.
    LimiterTruePeak,
}

impl ParamId {
    /// Returns the flat `0..PARAM_COUNT` index of this parameter.
    #[must_use]
    pub const fn index(self) -> usize {
        match self {
            Self::Bypass => 0,
            Self::InputGainDb => 1,
            Self::HpfEnabled => 2,
            Self::HpfFreqHz => 3,
            Self::DenoiseMix => 4,
            Self::GateEnabled => 5,
            Self::GateThresholdDb => 6,
            Self::GateRatio => 7,
            Self::GateHysteresisDb => 8,
            Self::GateAttackMs => 9,
            Self::GateReleaseMs => 10,
            Self::EqType(b) => 11 + b.index(),
            Self::EqFreqHz(b) => 16 + b.index(),
            Self::EqGainDb(b) => 21 + b.index(),
            Self::EqQ(b) => 26 + b.index(),
            Self::DeesserEnabled => 31,
            Self::DeesserFreqHz => 32,
            Self::DeesserThreshDb => 33,
            Self::DeesserAmountDb => 34,
            Self::CompEnabled => 35,
            Self::CompThreshDb => 36,
            Self::CompRatio => 37,
            Self::CompAttackMs => 38,
            Self::CompReleaseMs => 39,
            Self::CompKneeDb => 40,
            Self::CompMakeupDb => 41,
            Self::CompAutoMakeup => 42,
            Self::LimiterCeilingDb => 43,
            Self::LimiterTruePeak => 44,
        }
    }

    /// Reconstructs a [`ParamId`] from its flat `0..PARAM_COUNT` index.
    #[must_use]
    pub const fn from_index(i: usize) -> Option<Self> {
        match i {
            0 => Some(Self::Bypass),
            1 => Some(Self::InputGainDb),
            2 => Some(Self::HpfEnabled),
            3 => Some(Self::HpfFreqHz),
            4 => Some(Self::DenoiseMix),
            5 => Some(Self::GateEnabled),
            6 => Some(Self::GateThresholdDb),
            7 => Some(Self::GateRatio),
            8 => Some(Self::GateHysteresisDb),
            9 => Some(Self::GateAttackMs),
            10 => Some(Self::GateReleaseMs),
            11..=15 => match Band::new((i - 11) as u8) {
                Some(b) => Some(Self::EqType(b)),
                None => None,
            },
            16..=20 => match Band::new((i - 16) as u8) {
                Some(b) => Some(Self::EqFreqHz(b)),
                None => None,
            },
            21..=25 => match Band::new((i - 21) as u8) {
                Some(b) => Some(Self::EqGainDb(b)),
                None => None,
            },
            26..=30 => match Band::new((i - 26) as u8) {
                Some(b) => Some(Self::EqQ(b)),
                None => None,
            },
            31 => Some(Self::DeesserEnabled),
            32 => Some(Self::DeesserFreqHz),
            33 => Some(Self::DeesserThreshDb),
            34 => Some(Self::DeesserAmountDb),
            35 => Some(Self::CompEnabled),
            36 => Some(Self::CompThreshDb),
            37 => Some(Self::CompRatio),
            38 => Some(Self::CompAttackMs),
            39 => Some(Self::CompReleaseMs),
            40 => Some(Self::CompKneeDb),
            41 => Some(Self::CompMakeupDb),
            42 => Some(Self::CompAutoMakeup),
            43 => Some(Self::LimiterCeilingDb),
            44 => Some(Self::LimiterTruePeak),
            _ => None,
        }
    }

    /// Returns the static metadata specification for this parameter.
    #[must_use]
    pub const fn spec(self) -> &'static ParamSpec {
        // `index()` is statically proven `< PARAM_COUNT` by the const assertions below.
        #[allow(clippy::indexing_slicing)]
        &SPECS[self.index()]
    }
}

/// Static metadata for a single parameter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ParamSpec {
    /// Parameter identifier.
    pub id: ParamId,
    /// Stable machine key (used in JSON/CLI/automation).
    pub key: &'static str,
    /// Human-readable label for UI.
    pub label: &'static str,
    /// Display unit.
    pub unit: Unit,
    /// Minimum allowed value.
    pub min: f32,
    /// Maximum allowed value.
    pub max: f32,
    /// Neutral default value.
    pub default: f32,
    /// UI step increment.
    pub step: f32,
    /// Slider scale / control type.
    pub scale: Scale,
}

const fn bool_spec(id: ParamId, key: &'static str, label: &'static str, default: bool) -> ParamSpec {
    ParamSpec {
        id,
        key,
        label,
        unit: Unit::None,
        min: 0.0,
        max: 1.0,
        default: if default { 1.0 } else { 0.0 },
        step: 1.0,
        scale: Scale::Bool,
    }
}

const fn eq_type_spec(band: Band, key: &'static str, label: &'static str, default: FilterType) -> ParamSpec {
    ParamSpec {
        id: ParamId::EqType(band),
        key,
        label,
        unit: Unit::None,
        min: 0.0,
        max: 6.0,
        default: default.as_f32(),
        step: 1.0,
        scale: Scale::Enum,
    }
}

const fn eq_freq_spec(band: Band, key: &'static str, label: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        id: ParamId::EqFreqHz(band),
        key,
        label,
        unit: Unit::Hz,
        min: 20.0,
        max: 20_000.0,
        default,
        step: 1.0,
        scale: Scale::Log,
    }
}

const fn eq_gain_spec(band: Band, key: &'static str, label: &'static str) -> ParamSpec {
    ParamSpec {
        id: ParamId::EqGainDb(band),
        key,
        label,
        unit: Unit::Db,
        min: -18.0,
        max: 18.0,
        default: 0.0,
        step: 0.1,
        scale: Scale::Linear,
    }
}

const fn eq_q_spec(band: Band, key: &'static str, label: &'static str, default: f32) -> ParamSpec {
    ParamSpec {
        id: ParamId::EqQ(band),
        key,
        label,
        unit: Unit::None,
        min: 0.1,
        max: 10.0,
        default,
        step: 0.05,
        scale: Scale::Linear,
    }
}

/// Authoritative specification table for all [`PARAM_COUNT`] parameters.
pub const SPECS: [ParamSpec; PARAM_COUNT] = [
    bool_spec(ParamId::Bypass, "bypass", "Bypass", false),
    ParamSpec {
        id: ParamId::InputGainDb,
        key: "input_gain_db",
        label: "Input Gain",
        unit: Unit::Db,
        min: -24.0,
        max: 36.0,
        default: 0.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    bool_spec(ParamId::HpfEnabled, "hpf_enabled", "High-Pass", true),
    ParamSpec {
        id: ParamId::HpfFreqHz,
        key: "hpf_freq_hz",
        label: "HPF Cutoff",
        unit: Unit::Hz,
        min: 20.0,
        max: 200.0,
        default: 80.0,
        step: 1.0,
        scale: Scale::Log,
    },
    ParamSpec {
        id: ParamId::DenoiseMix,
        key: "denoise_mix",
        label: "AI Denoise",
        unit: Unit::None,
        min: 0.0,
        max: 1.0,
        default: 0.5,
        step: 0.01,
        scale: Scale::Linear,
    },
    bool_spec(ParamId::GateEnabled, "gate_enabled", "Expander / Gate", true),
    ParamSpec {
        id: ParamId::GateThresholdDb,
        key: "gate_threshold_db",
        label: "Gate Threshold",
        unit: Unit::Dbfs,
        min: -80.0,
        max: -20.0,
        default: -50.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::GateRatio,
        key: "gate_ratio",
        label: "Gate Ratio",
        unit: Unit::Ratio,
        min: 1.0,
        max: 10.0,
        default: 2.0,
        step: 0.1,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::GateHysteresisDb,
        key: "gate_hysteresis_db",
        label: "Gate Hysteresis",
        unit: Unit::Db,
        min: 0.0,
        max: 12.0,
        default: 4.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::GateAttackMs,
        key: "gate_attack_ms",
        label: "Gate Attack",
        unit: Unit::Ms,
        min: 0.1,
        max: 20.0,
        default: 1.0,
        step: 0.1,
        scale: Scale::Log,
    },
    ParamSpec {
        id: ParamId::GateReleaseMs,
        key: "gate_release_ms",
        label: "Gate Release",
        unit: Unit::Ms,
        min: 10.0,
        max: 1000.0,
        default: 150.0,
        step: 5.0,
        scale: Scale::Log,
    },
    // EQ Band Types (11..=15)
    eq_type_spec(Band::B0, "eq.0.type", "EQ 1 Type", FilterType::LowShelf),
    eq_type_spec(Band::B1, "eq.1.type", "EQ 2 Type", FilterType::Peaking),
    eq_type_spec(Band::B2, "eq.2.type", "EQ 3 Type", FilterType::Peaking),
    eq_type_spec(Band::B3, "eq.3.type", "EQ 4 Type", FilterType::Peaking),
    eq_type_spec(Band::B4, "eq.4.type", "EQ 5 Type", FilterType::HighShelf),
    // EQ Band Frequencies (16..=20)
    eq_freq_spec(Band::B0, "eq.0.freq_hz", "EQ 1 Freq", 110.0),
    eq_freq_spec(Band::B1, "eq.1.freq_hz", "EQ 2 Freq", 320.0),
    eq_freq_spec(Band::B2, "eq.2.freq_hz", "EQ 3 Freq", 950.0),
    eq_freq_spec(Band::B3, "eq.3.freq_hz", "EQ 4 Freq", 4200.0),
    eq_freq_spec(Band::B4, "eq.4.freq_hz", "EQ 5 Freq", 8500.0),
    // EQ Band Gains (21..=25)
    eq_gain_spec(Band::B0, "eq.0.gain_db", "EQ 1 Gain"),
    eq_gain_spec(Band::B1, "eq.1.gain_db", "EQ 2 Gain"),
    eq_gain_spec(Band::B2, "eq.2.gain_db", "EQ 3 Gain"),
    eq_gain_spec(Band::B3, "eq.3.gain_db", "EQ 4 Gain"),
    eq_gain_spec(Band::B4, "eq.4.gain_db", "EQ 5 Gain"),
    // EQ Band Qs (26..=30)
    eq_q_spec(Band::B0, "eq.0.q", "EQ 1 Q", 0.707),
    eq_q_spec(Band::B1, "eq.1.q", "EQ 2 Q", 1.0),
    eq_q_spec(Band::B2, "eq.2.q", "EQ 3 Q", 1.0),
    eq_q_spec(Band::B3, "eq.3.q", "EQ 4 Q", 1.0),
    eq_q_spec(Band::B4, "eq.4.q", "EQ 5 Q", 0.707),
    // De-Esser (31..=34)
    bool_spec(ParamId::DeesserEnabled, "deesser_enabled", "De-Esser", false),
    ParamSpec {
        id: ParamId::DeesserFreqHz,
        key: "deesser_freq_hz",
        label: "De-Esser Freq",
        unit: Unit::Hz,
        min: 5500.0,
        max: 9500.0,
        default: 6800.0,
        step: 50.0,
        scale: Scale::Log,
    },
    ParamSpec {
        id: ParamId::DeesserThreshDb,
        key: "deesser_thresh_db",
        label: "De-Esser Threshold",
        unit: Unit::Dbfs,
        min: -60.0,
        max: 0.0,
        default: -30.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::DeesserAmountDb,
        key: "deesser_amount_db",
        label: "De-Esser Amount",
        unit: Unit::Db,
        min: 0.0,
        max: 12.0,
        default: 6.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    // Compressor (35..=42)
    bool_spec(ParamId::CompEnabled, "comp_enabled", "Compressor", true),
    ParamSpec {
        id: ParamId::CompThreshDb,
        key: "comp_thresh_db",
        label: "Comp Threshold",
        unit: Unit::Dbfs,
        min: -60.0,
        max: 0.0,
        default: -20.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::CompRatio,
        key: "comp_ratio",
        label: "Comp Ratio",
        unit: Unit::Ratio,
        min: 1.0,
        max: 20.0,
        default: 3.0,
        step: 0.1,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::CompAttackMs,
        key: "comp_attack_ms",
        label: "Comp Attack",
        unit: Unit::Ms,
        min: 0.1,
        max: 100.0,
        default: 5.0,
        step: 0.5,
        scale: Scale::Log,
    },
    ParamSpec {
        id: ParamId::CompReleaseMs,
        key: "comp_release_ms",
        label: "Comp Release",
        unit: Unit::Ms,
        min: 10.0,
        max: 1000.0,
        default: 120.0,
        step: 5.0,
        scale: Scale::Log,
    },
    ParamSpec {
        id: ParamId::CompKneeDb,
        key: "comp_knee_db",
        label: "Comp Knee",
        unit: Unit::Db,
        min: 0.0,
        max: 12.0,
        default: 6.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    ParamSpec {
        id: ParamId::CompMakeupDb,
        key: "comp_makeup_db",
        label: "Comp Makeup",
        unit: Unit::Db,
        min: 0.0,
        max: 24.0,
        default: 0.0,
        step: 0.5,
        scale: Scale::Linear,
    },
    bool_spec(ParamId::CompAutoMakeup, "comp_auto_makeup", "Auto Makeup", true),
    // Limiter (43..=44)
    ParamSpec {
        id: ParamId::LimiterCeilingDb,
        key: "limiter_ceiling_db",
        label: "Limiter Ceiling",
        unit: Unit::Dbfs,
        min: -12.0,
        max: -0.1,
        default: -1.0,
        step: 0.1,
        scale: Scale::Linear,
    },
    bool_spec(ParamId::LimiterTruePeak, "limiter_true_peak", "True Peak", false),
];

// Compile-time invariants on `SPECS` and `ParamId::index`.
#[allow(clippy::indexing_slicing)]
const _: () = {
    let mut i = 0;
    while i < PARAM_COUNT {
        assert!(SPECS[i].id.index() == i);
        assert!(SPECS[i].min <= SPECS[i].default);
        assert!(SPECS[i].default <= SPECS[i].max);
        i += 1;
    }
};

/// Complete resolved parameter snapshot consumed by `tine-dsp`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DspParams {
    /// Global A/B bypass.
    pub bypass: bool,
    /// Stage 1: Input gain in dB (`-24..=+36`).
    pub input_gain_db: f32,
    /// Stage 1: High-pass filter enabled.
    pub hpf_enabled: bool,
    /// Stage 1: High-pass cutoff frequency in Hz (`20..=200`).
    pub hpf_freq_hz: f32,
    /// Stage 2: `RNNoise` wet/dry mix (`0.0..=1.0`).
    pub denoise_mix: f32,
    /// Stage 2: Downward expander enabled.
    pub gate_enabled: bool,
    /// Stage 2: Expander threshold in dBFS (`-80..=-20`).
    pub gate_threshold_db: f32,
    /// Stage 2: Expander ratio (`1..=10`).
    pub gate_ratio: f32,
    /// Stage 2: Expander hysteresis in dB (`0..=12`).
    pub gate_hysteresis_db: f32,
    /// Stage 2: Expander attack in ms (`0.1..=20`).
    pub gate_attack_ms: f32,
    /// Stage 2: Expander release in ms (`10..=1000`).
    pub gate_release_ms: f32,
    /// Stage 3: EQ band filter types.
    pub eq_type: [FilterType; EQ_BAND_COUNT],
    /// Stage 3: EQ band frequencies in Hz (`20..=20000`).
    pub eq_freq_hz: [f32; EQ_BAND_COUNT],
    /// Stage 3: EQ band gains in dB (`-18..=+18`).
    pub eq_gain_db: [f32; EQ_BAND_COUNT],
    /// Stage 3: EQ band Q factors (`0.1..=10`).
    pub eq_q: [f32; EQ_BAND_COUNT],
    /// Stage 4: Split-band de-esser enabled.
    pub deesser_enabled: bool,
    /// Stage 4: De-esser frequency in Hz (`5500..=9500`).
    pub deesser_freq_hz: f32,
    /// Stage 4: De-esser threshold in dBFS (`-60..=0`).
    pub deesser_thresh_db: f32,
    /// Stage 4: De-esser max gain reduction in dB (`0..=12`).
    pub deesser_amount_db: f32,
    /// Stage 5: Compressor enabled.
    pub comp_enabled: bool,
    /// Stage 5: Compressor threshold in dBFS (`-60..=0`).
    pub comp_thresh_db: f32,
    /// Stage 5: Compressor ratio (`1..=20`).
    pub comp_ratio: f32,
    /// Stage 5: Compressor attack in ms (`0.1..=100`).
    pub comp_attack_ms: f32,
    /// Stage 5: Compressor release in ms (`10..=1000`).
    pub comp_release_ms: f32,
    /// Stage 5: Compressor soft-knee width in dB (`0..=12`).
    pub comp_knee_db: f32,
    /// Stage 5: Compressor manual makeup gain in dB (`0..=24`).
    pub comp_makeup_db: f32,
    /// Stage 5: Compressor automatic makeup gain enabled.
    pub comp_auto_makeup: bool,
    /// Stage 6: Lookahead limiter ceiling in dBFS (`-12..=-0.1`).
    pub limiter_ceiling_db: f32,
    /// Stage 6: Limiter 4x oversampled true-peak detection enabled.
    pub limiter_true_peak: bool,
}

impl Default for DspParams {
    fn default() -> Self {
        Self::DEFAULT
    }
}

#[inline]
fn clamp_finite(val: f32, spec: &ParamSpec) -> f32 {
    if val.is_finite() {
        val.clamp(spec.min, spec.max)
    } else {
        spec.default
    }
}

impl DspParams {
    /// Neutral default parameters matching [`SPECS`].
    pub const DEFAULT: Self = Self {
        bypass: false,
        input_gain_db: 0.0,
        hpf_enabled: true,
        hpf_freq_hz: 80.0,
        denoise_mix: 0.5,
        gate_enabled: true,
        gate_threshold_db: -50.0,
        gate_ratio: 2.0,
        gate_hysteresis_db: 4.0,
        gate_attack_ms: 1.0,
        gate_release_ms: 150.0,
        eq_type: [
            FilterType::LowShelf,
            FilterType::Peaking,
            FilterType::Peaking,
            FilterType::Peaking,
            FilterType::HighShelf,
        ],
        eq_freq_hz: [110.0, 320.0, 950.0, 4200.0, 8500.0],
        eq_gain_db: [0.0; EQ_BAND_COUNT],
        eq_q: [0.707, 1.0, 1.0, 1.0, 0.707],
        deesser_enabled: false,
        deesser_freq_hz: 6800.0,
        deesser_thresh_db: -30.0,
        deesser_amount_db: 6.0,
        comp_enabled: true,
        comp_thresh_db: -20.0,
        comp_ratio: 3.0,
        comp_attack_ms: 5.0,
        comp_release_ms: 120.0,
        comp_knee_db: 6.0,
        comp_makeup_db: 0.0,
        comp_auto_makeup: true,
        limiter_ceiling_db: -1.0,
        limiter_true_peak: false,
    };

    /// Reads any parameter by [`ParamId`] as an `f32` (`0.0`/`1.0` for bools, `0.0..=6.0` for [`FilterType`]).
    #[must_use]
    pub fn get(&self, id: ParamId) -> f32 {
        match id {
            ParamId::Bypass => f32::from(u8::from(self.bypass)),
            ParamId::InputGainDb => self.input_gain_db,
            ParamId::HpfEnabled => f32::from(u8::from(self.hpf_enabled)),
            ParamId::HpfFreqHz => self.hpf_freq_hz,
            ParamId::DenoiseMix => self.denoise_mix,
            ParamId::GateEnabled => f32::from(u8::from(self.gate_enabled)),
            ParamId::GateThresholdDb => self.gate_threshold_db,
            ParamId::GateRatio => self.gate_ratio,
            ParamId::GateHysteresisDb => self.gate_hysteresis_db,
            ParamId::GateAttackMs => self.gate_attack_ms,
            ParamId::GateReleaseMs => self.gate_release_ms,
            ParamId::EqType(b) => self.eq_type.get(b.index()).copied().unwrap_or_default().as_f32(),
            ParamId::EqFreqHz(b) => self.eq_freq_hz.get(b.index()).copied().unwrap_or(1000.0),
            ParamId::EqGainDb(b) => self.eq_gain_db.get(b.index()).copied().unwrap_or(0.0),
            ParamId::EqQ(b) => self.eq_q.get(b.index()).copied().unwrap_or(1.0),
            ParamId::DeesserEnabled => f32::from(u8::from(self.deesser_enabled)),
            ParamId::DeesserFreqHz => self.deesser_freq_hz,
            ParamId::DeesserThreshDb => self.deesser_thresh_db,
            ParamId::DeesserAmountDb => self.deesser_amount_db,
            ParamId::CompEnabled => f32::from(u8::from(self.comp_enabled)),
            ParamId::CompThreshDb => self.comp_thresh_db,
            ParamId::CompRatio => self.comp_ratio,
            ParamId::CompAttackMs => self.comp_attack_ms,
            ParamId::CompReleaseMs => self.comp_release_ms,
            ParamId::CompKneeDb => self.comp_knee_db,
            ParamId::CompMakeupDb => self.comp_makeup_db,
            ParamId::CompAutoMakeup => f32::from(u8::from(self.comp_auto_makeup)),
            ParamId::LimiterCeilingDb => self.limiter_ceiling_db,
            ParamId::LimiterTruePeak => f32::from(u8::from(self.limiter_true_peak)),
        }
    }

    /// Sets a parameter by [`ParamId`], clamping to [`ParamSpec`] bounds and replacing
    /// non-finite floats with the parameter's default.
    pub fn set(&mut self, id: ParamId, v: f32) {
        let spec = id.spec();
        let val = clamp_finite(v, spec);
        match id {
            ParamId::Bypass => self.bypass = val >= 0.5,
            ParamId::InputGainDb => self.input_gain_db = val,
            ParamId::HpfEnabled => self.hpf_enabled = val >= 0.5,
            ParamId::HpfFreqHz => self.hpf_freq_hz = val,
            ParamId::DenoiseMix => self.denoise_mix = val,
            ParamId::GateEnabled => self.gate_enabled = val >= 0.5,
            ParamId::GateThresholdDb => self.gate_threshold_db = val,
            ParamId::GateRatio => self.gate_ratio = val,
            ParamId::GateHysteresisDb => self.gate_hysteresis_db = val,
            ParamId::GateAttackMs => self.gate_attack_ms = val,
            ParamId::GateReleaseMs => self.gate_release_ms = val,
            ParamId::EqType(b) => {
                if let Some(slot) = self.eq_type.get_mut(b.index()) {
                    *slot = FilterType::from_f32(val);
                }
            }
            ParamId::EqFreqHz(b) => {
                if let Some(slot) = self.eq_freq_hz.get_mut(b.index()) {
                    *slot = val;
                }
            }
            ParamId::EqGainDb(b) => {
                if let Some(slot) = self.eq_gain_db.get_mut(b.index()) {
                    *slot = val;
                }
            }
            ParamId::EqQ(b) => {
                if let Some(slot) = self.eq_q.get_mut(b.index()) {
                    *slot = val;
                }
            }
            ParamId::DeesserEnabled => self.deesser_enabled = val >= 0.5,
            ParamId::DeesserFreqHz => self.deesser_freq_hz = val,
            ParamId::DeesserThreshDb => self.deesser_thresh_db = val,
            ParamId::DeesserAmountDb => self.deesser_amount_db = val,
            ParamId::CompEnabled => self.comp_enabled = val >= 0.5,
            ParamId::CompThreshDb => self.comp_thresh_db = val,
            ParamId::CompRatio => self.comp_ratio = val,
            ParamId::CompAttackMs => self.comp_attack_ms = val,
            ParamId::CompReleaseMs => self.comp_release_ms = val,
            ParamId::CompKneeDb => self.comp_knee_db = val,
            ParamId::CompMakeupDb => self.comp_makeup_db = val,
            ParamId::CompAutoMakeup => self.comp_auto_makeup = val >= 0.5,
            ParamId::LimiterCeilingDb => self.limiter_ceiling_db = val,
            ParamId::LimiterTruePeak => self.limiter_true_peak = val >= 0.5,
        }
    }

    /// Returns a copy with every parameter clamped to its [`ParamSpec`] range
    /// and any non-finite float replaced by its default.
    #[must_use]
    pub fn clamped(mut self) -> Self {
        self.input_gain_db = clamp_finite(self.input_gain_db, ParamId::InputGainDb.spec());
        self.hpf_freq_hz = clamp_finite(self.hpf_freq_hz, ParamId::HpfFreqHz.spec());
        self.denoise_mix = clamp_finite(self.denoise_mix, ParamId::DenoiseMix.spec());
        self.gate_threshold_db = clamp_finite(self.gate_threshold_db, ParamId::GateThresholdDb.spec());
        self.gate_ratio = clamp_finite(self.gate_ratio, ParamId::GateRatio.spec());
        self.gate_hysteresis_db = clamp_finite(self.gate_hysteresis_db, ParamId::GateHysteresisDb.spec());
        self.gate_attack_ms = clamp_finite(self.gate_attack_ms, ParamId::GateAttackMs.spec());
        self.gate_release_ms = clamp_finite(self.gate_release_ms, ParamId::GateReleaseMs.spec());

        for i in 0..EQ_BAND_COUNT {
            if let Some(b) = Band::new(i as u8) {
                if let Some(v) = self.eq_freq_hz.get_mut(i) {
                    *v = clamp_finite(*v, ParamId::EqFreqHz(b).spec());
                }
                if let Some(v) = self.eq_gain_db.get_mut(i) {
                    *v = clamp_finite(*v, ParamId::EqGainDb(b).spec());
                }
                if let Some(v) = self.eq_q.get_mut(i) {
                    *v = clamp_finite(*v, ParamId::EqQ(b).spec());
                }
            }
        }

        self.deesser_freq_hz = clamp_finite(self.deesser_freq_hz, ParamId::DeesserFreqHz.spec());
        self.deesser_thresh_db = clamp_finite(self.deesser_thresh_db, ParamId::DeesserThreshDb.spec());
        self.deesser_amount_db = clamp_finite(self.deesser_amount_db, ParamId::DeesserAmountDb.spec());
        self.comp_thresh_db = clamp_finite(self.comp_thresh_db, ParamId::CompThreshDb.spec());
        self.comp_ratio = clamp_finite(self.comp_ratio, ParamId::CompRatio.spec());
        self.comp_attack_ms = clamp_finite(self.comp_attack_ms, ParamId::CompAttackMs.spec());
        self.comp_release_ms = clamp_finite(self.comp_release_ms, ParamId::CompReleaseMs.spec());
        self.comp_knee_db = clamp_finite(self.comp_knee_db, ParamId::CompKneeDb.spec());
        self.comp_makeup_db = clamp_finite(self.comp_makeup_db, ParamId::CompMakeupDb.spec());
        self.limiter_ceiling_db = clamp_finite(self.limiter_ceiling_db, ParamId::LimiterCeilingDb.spec());
        self
    }
}
