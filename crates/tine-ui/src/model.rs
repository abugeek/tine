//! Engine-agnostic UI view model types.

use tine_protocol::params::{DenoiseEngine, DspParams, ParamId};
use tine_protocol::presets::{Archetype, Calibration, MacroPositions, PinSet};

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
    /// Output peak of `finish()`'s offline full-pipeline verification (dBFS),
    /// proven to land in `-6..-3` (T11).
    pub simulated_peak_db: f32,
    /// Output RMS of the offline verification (dBFS).
    pub simulated_rms_db: f32,
    /// Parameters modified by the calibration.
    pub changes: Vec<CalibrationParamDiff>,
}

/// UI view-model of a calibration rejection: `tine_dsp::calibrator::CalFailure`
/// (T11, `docs/tasks/T11-auto-calibrator.md`) mapped into `tine-ui` in **one
/// place** (`views/autotune.rs` — the narrow adapter between the engine-side
/// state machine and this wizard's view model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum CalFailureUi {
    /// Signal (and room) stayed below every usable threshold.
    TooQuiet,
    /// Room speech-to-noise ratio fell under the 10 dB minimum (`snr_db` measured).
    TooNoisy {
        /// Measured speech-to-noise ratio in dB.
        snr_db: f32,
    },
    /// More than 0.5 % of captured samples clipped at -0.3 dBFS.
    Clipping {
        /// Fraction of clipped samples (0.0..=1.0).
        pct: f32,
    },
    /// Fewer than 1.5 s of voiced audio in the 5 s speech phase.
    NotEnoughSpeech {
        /// Total voiced audio in milliseconds.
        voiced_ms: u32,
    },
    /// The gain needed to reach -18 dBFS speech exceeds the +36 dB range.
    GainOutOfRange {
        /// Gain the calibration would have needed, in dB.
        needed_db: f32,
    },
}

impl CalFailureUi {
    /// Short machine-stable summary (`AutoTuneStep::Failed` payload used in logs and tests).
    #[must_use]
    pub fn summary(self) -> String {
        match self {
            Self::TooQuiet => String::from("Your microphone stayed nearly silent."),
            Self::TooNoisy { snr_db } => {
                format!("Your room is too noisy for a clean measurement (SNR {snr_db:.1} dB).")
            }
            Self::Clipping { pct } => {
                format!("Your mic is clipping ({:.1} % of samples clipped).", pct * 100.0)
            }
            Self::NotEnoughSpeech { voiced_ms } => {
                format!("Only {voiced_ms} ms of speech detected in the 5-second sample.")
            }
            Self::GainOutOfRange { needed_db } => {
                format!("Your mic needs about {needed_db:+.0} dB of gain — beyond Tine's range.")
            }
        }
    }
}

/// Current step of the 8-second Auto-Tune wizard modal, mapped 1:1 from
/// `tine_dsp::calibrator::CalPhase` (Countdown → Noise → Voice → Analyzing →
/// Done/Failed) by the adapter in `views/autotune.rs`.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum AutoTuneStep {
    /// Wizard is closed.
    #[default]
    Idle,
    /// Countdown phase: the first second, discarded (get ready / settle) —
    /// tips plus a 1 s countdown ring.
    Prepare {
        /// Remaining seconds in the countdown phase.
        remaining_secs: f32,
    },
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
    /// Calibration failed or was rejected; the workaround for each variant is
    /// rendered by the failure card (`views/autotune.rs`).
    Failed(CalFailureUi),
}

/// Built-in 5-second Mic Check (Raw vs Tine A/B playback) state.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum MicCheckStep {
    /// Mic Check modal is closed.
    #[default]
    Idle,
    /// Capturing 5 seconds of raw audio.
    Recording {
        /// Normalized progress (`0.0..=1.0`).
        progress: f32,
        /// Live input level in dBFS.
        level_db: f32,
    },
    /// Playing back the sample-aligned A/B loop.
    Playing {
        /// `true` when auditioning Raw Mic, `false` for Tine Processed.
        raw: bool,
        /// Normalized playback loop position (`0.0..=1.0`).
        progress: f32,
        /// Total snippet duration in seconds.
        duration_s: f32,
    },
    /// Recording or playback failed with a message.
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
    /// Total voiced audio detected so far in the voice phase (ms)
    /// (`CalProgress::voiced_ms`).
    pub voiced_ms: u32,
}

impl Default for AutoTuneUi {
    fn default() -> Self {
        Self {
            step: AutoTuneStep::Idle,
            progress: 0.0,
            level_db: -60.0,
            prompt_text: String::from("The quick brown fox jumps over the lazy dog."),
            voiced_ms: 0,
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
#[derive(Clone, Debug, PartialEq)]
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
    /// Click-free RT output mute state (`true` = muted).
    pub muted: bool,
    /// Active Stage 2 denoiser backend (`Rnnoise`, `DeepFilterNet3`, or `Off`).
    pub denoise_engine: DenoiseEngine,
    /// Automatic idle parking enabled (`false` = Keep Warm for PTT).
    pub park_when_unused: bool,
    /// "Hear Myself" monitor state.
    pub monitor: MonitorUi,
    /// Completed Auto-Tune calibration results awaiting Apply/Discard.
    pub calibration: Option<CalibrationUi>,
    /// Full calibration struct for pending Auto-Tune results (carries tilt/crest metrics).
    pub pending_calibration: Option<Calibration>,
    /// Currently active Auto-Tune calibration applied to this microphone profile.
    pub active_calibration: Option<Calibration>,
    /// Auto-Tune wizard progress state.
    pub autotune: AutoTuneUi,
    /// Built-in 5-second Mic Check A/B modal state.
    pub mic_check: MicCheckStep,
    /// Whether to show the first-run onboarding overlay.
    pub first_run: bool,
    /// Optional dismissible status or error banner shown in the main view.
    pub status_banner: Option<String>,
    /// Other apps currently recording directly from the physical mic instead of `CABLE Output`.
    pub raw_mic_apps: Vec<String>,
}

impl Default for UiModel {
    fn default() -> Self {
        Self {
            endpoints: Vec::new(),
            selected: None,
            apo: ApoBadge::default(),
            archetype: Archetype::default(),
            macros: MacroPositions::default(),
            pins: PinSet::default(),
            params: DspParams::default(),
            bypass: false,
            muted: false,
            denoise_engine: DenoiseEngine::default(),
            park_when_unused: true,
            monitor: MonitorUi::default(),
            calibration: None,
            pending_calibration: None,
            active_calibration: None,
            autotune: AutoTuneUi::default(),
            mic_check: MicCheckStep::default(),
            first_run: false,
            status_banner: None,
            raw_mic_apps: Vec::new(),
        }
    }
}
