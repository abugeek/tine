//! Engine-agnostic UI view model types.

use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::{Archetype, MacroPositions, PinSet};

/// A physical microphone endpoint entry shown in the header selector.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EndpointRow {
    /// Windows endpoint identifier (or synthetic ID in scenes/demo).
    pub id: String,
    /// Human-readable device name (e.g. `"Shure SM7B"`).
    pub name: String,
}

/// Status of the Windows COM APO for the selected capture endpoint.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum ApoBadge {
    /// APO is installed, enabled, and publishing live telemetry.
    Active,
    /// APO is not registered on this endpoint.
    #[default]
    NotInstalled,
    /// APO is registered but Windows Audio Enhancements are turned off.
    Inactive,
    /// APO reported an error or faulted into pass-through.
    Error(String),
}

impl ApoBadge {
    /// Short badge label for the header status pill.
    #[must_use]
    pub fn label(&self) -> &str {
        match self {
            Self::Active => "Active",
            Self::NotInstalled => "Not installed",
            Self::Inactive => "Inactive",
            Self::Error(_) => "Error",
        }
    }
}

/// "Hear Myself" local monitoring state.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MonitorUi {
    /// Whether local loopback monitoring is active.
    pub enabled: bool,
    /// Currently selected playback device name, if any.
    pub device_name: Option<String>,
    /// Available playback devices for the settings popover.
    pub available_devices: Vec<String>,
    /// Estimated round-trip monitoring latency in milliseconds.
    pub estimated_latency_ms: f32,
}

/// A single parameter change proposed by the 8-second Auto-Tune wizard.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationParamDiff {
    /// Target DSP parameter.
    pub param: ParamId,
    /// Human-readable parameter label.
    pub label: &'static str,
    /// Value before Auto-Tune.
    pub before: f32,
    /// Proposed value after Auto-Tune.
    pub after: f32,
    /// Unit suffix (e.g. `"dB"`, `"dBFS"`, `"Hz"`, `""`).
    pub unit: &'static str,
}

/// Results of a completed Auto-Tune calibration run.
#[derive(Clone, Debug, PartialEq)]
pub struct CalibrationUi {
    /// Measured ambient room noise RMS in dBFS.
    pub noise_rms_db: f32,
    /// Measured vocal speech RMS in dBFS.
    pub speech_rms_db: f32,
    /// Parameters modified by the calibration.
    pub changes: Vec<CalibrationParamDiff>,
}

/// Current step of the 8-second Auto-Tune wizard modal.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AutoTuneStep {
    /// Wizard is closed.
    #[default]
    Idle,
    /// Preparation screen before silence capture begins.
    Prepare,
    /// Phase 1: 3 seconds of room silence capture.
    Silence {
        /// Remaining seconds in this phase.
        remaining_secs: f32,
    },
    /// Phase 2: 5 seconds of normal speaking capture.
    Speak {
        /// Remaining seconds in this phase.
        remaining_secs: f32,
    },
    /// Computing optimal parameters from captured audio.
    Analyzing,
    /// Displaying before/after parameter diffs for confirmation.
    Results,
    /// Calibration failed or was rejected (e.g. clipping or silence).
    Failed(String),
}

/// Auto-Tune wizard state mirrored from the engine.
#[derive(Clone, Debug, PartialEq)]
pub struct AutoTuneUi {
    /// Active wizard step.
    pub step: AutoTuneStep,
    /// Overall normalized progress (`0.0..=1.0`).
    pub progress: f32,
    /// Live input level in dBFS during capture.
    pub level_db: f32,
    /// Prompt sentence shown during the Speak phase.
    pub prompt_text: String,
}

impl Default for AutoTuneUi {
    fn default() -> Self {
        Self {
            step: AutoTuneStep::Idle,
            progress: 0.0,
            level_db: -60.0,
            prompt_text: String::from("The quick brown fox jumps over the lazy dog."),
        }
    }
}

/// One of the four Golden Line macro nodes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Macro {
    /// Noise & Gate macro (`0.0..=1.0`).
    Purity,
    /// Warmth & Mud Control macro (`0.0..=1.0`).
    Body,
    /// Clarity & De-Ess macro (`0.0..=1.0`).
    Presence,
    /// Compression & Voice Focus macro (`0.0..=1.0`).
    Leveler,
}

impl Macro {
    /// All four macros in left-to-right canvas / Tab order.
    pub const ALL: [Self; 4] = [Self::Purity, Self::Body, Self::Presence, Self::Leveler];

    /// Display name for the macro node.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Purity => "Purity",
            Self::Body => "Body",
            Self::Presence => "Presence",
            Self::Leveler => "Leveler",
        }
    }

    /// Stable Rune element key for this macro node.
    #[must_use]
    pub const fn key(self) -> &'static str {
        match self {
            Self::Purity => "macro/purity",
            Self::Body => "macro/body",
            Self::Presence => "macro/presence",
            Self::Leveler => "macro/leveler",
        }
    }

    /// Extracts this macro's value from [`MacroPositions`].
    #[must_use]
    pub const fn get(self, pos: MacroPositions) -> f32 {
        match self {
            Self::Purity => pos.purity,
            Self::Body => pos.body,
            Self::Presence => pos.presence,
            Self::Leveler => pos.leveler,
        }
    }

    /// Updates this macro's value in [`MacroPositions`] (clamped to `0.0..=1.0`).
    pub fn set(self, pos: &mut MacroPositions, value: f32) {
        let clamped = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.5 };
        match self {
            Self::Purity => pos.purity = clamped,
            Self::Body => pos.body = clamped,
            Self::Presence => pos.presence = clamped,
            Self::Leveler => pos.leveler = clamped,
        }
    }
}

/// Authoritative UI model pushed from the engine control plane.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct UiModel {
    /// Available capture endpoints.
    pub endpoints: Vec<EndpointRow>,
    /// Index of the selected endpoint in `endpoints`.
    pub selected: Option<usize>,
    /// APO status for the selected endpoint.
    pub apo: ApoBadge,
    /// Active microphone hardware archetype.
    pub archetype: Archetype,
    /// Current positions of the 4 Golden Line macro nodes.
    pub macros: MacroPositions,
    /// Parameters pinned by Pro-mode edits.
    pub pins: PinSet,
    /// Current DSP parameter values.
    pub params: DspParams,
    /// Global A/B bypass state (`true` = raw mic).
    pub bypass: bool,
    /// "Hear Myself" monitor state.
    pub monitor: MonitorUi,
    /// Completed Auto-Tune calibration results awaiting Apply/Discard.
    pub calibration: Option<CalibrationUi>,
    /// Auto-Tune wizard progress state.
    pub autotune: AutoTuneUi,
    /// Whether to show the first-run onboarding overlay.
    pub first_run: bool,
}
