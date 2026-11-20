//! User intents emitted by UI interactions toward the engine control plane.

use tine_dsp::calibrator::VoiceStyle;
use tine_protocol::params::{DenoiseEngine, ParamId};
use tine_protocol::presets::Archetype;

use crate::model::Macro;

/// Discrete user intent emitted by `tine-ui` controls.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Select a microphone endpoint by its identifier (`EndpointRow::id`).
    SelectEndpoint(String),
    /// Switch the microphone hardware archetype preset.
    SetArchetype(Archetype),
    /// Switch the Stage 2 denoiser engine (`Rnnoise`, `DeepFilterNet3`, `Off`).
    SetDenoiseEngine(DenoiseEngine),
    /// Enable or disable automatic idle parking (`false` = Keep Warm for PTT).
    SetParkWhenUnused(bool),
    /// Move a Golden Line macro node (`0.0..=1.0`).
    SetMacro(Macro, f32),
    /// Set a Pro-mode parameter directly (pins it).
    SetParam(ParamId, f32),
    /// Unpin a single parameter so macros drive it again.
    Unpin(ParamId),
    /// Unpin all parameters.
    UnpinAll,
    /// Toggle global A/B bypass.
    ToggleBypass,
    /// Set click-free RT output mute (`true` = muted).
    SetMuted(bool),
    /// Toggle "Hear Myself" local monitor.
    ToggleMonitor,
    /// Pick the playback device for "Hear Myself".
    PickMonitorDevice(String),
    /// Start the 8-second Auto-Tune wizard.
    StartAutoTune,
    /// Cancel an in-progress Auto-Tune wizard.
    CancelAutoTune,
    /// Preview pending Auto-Tune calibration + voice style on live DSP and recorded voice loop.
    PreviewCalibration {
        /// Selected post-calibration voice style.
        style: VoiceStyle,
        /// Whether to preserve existing Pro-mode pinned parameters.
        keep_pins: bool,
        /// `Some(false)` to audition Tuned voice, `Some(true)` to audition Raw voice, `None` to stop audition loop.
        audition_raw: Option<bool>,
    },
    /// Apply Auto-Tune calibration results.
    ApplyCalibration {
        /// Whether to preserve existing Pro-mode pinned parameters.
        keep_pins: bool,
    },
    /// Discard Auto-Tune calibration results.
    DiscardCalibration,
    /// Install the Windows COM APO on the selected endpoint.
    InstallApo,
    /// Repair a broken or stripped APO registration.
    RepairApo,
    /// Open the settings / monitor device popover.
    OpenSettings,
    /// Set the Pro Rack drawer's open state (persisted by the engine).
    SetProDrawerOpen(bool),
    /// Start the 5-second Raw vs Tine Mic Check A/B recording.
    StartMicCheck,
    /// Switch Mic Check playback mode (`true` = Raw Mic, `false` = Tine Processed).
    SetMicCheckMode(bool),
    /// Stop and close the Mic Check modal.
    StopMicCheck,
    /// Request closing the UI window.
    CloseWindow,
}

/// Sink receiving [`Intent`]s from the UI.
///
/// Implemented by `tine-app` to forward to the engine, and by recording fakes in tests.
pub trait IntentSink: 'static {
    /// Dispatches a user intent.
    fn send(&self, i: Intent);
}

/// No-op [`IntentSink`] used by default state and headless scenes.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopSink;

impl IntentSink for NoopSink {
    fn send(&self, _i: Intent) {}
}
