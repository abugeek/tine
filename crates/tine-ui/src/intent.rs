//! User intents emitted by UI interactions toward the engine control plane.

use tine_protocol::params::ParamId;
use tine_protocol::presets::Archetype;

use crate::model::Macro;

/// Discrete user intent emitted by `tine-ui` controls.
#[derive(Clone, Debug, PartialEq)]
pub enum Intent {
    /// Select a microphone endpoint by index in `UiModel::endpoints`.
    SelectEndpoint(usize),
    /// Switch the microphone hardware archetype preset.
    SetArchetype(Archetype),
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
    /// Toggle "Hear Myself" local monitor.
    ToggleMonitor,
    /// Pick the playback device for "Hear Myself".
    PickMonitorDevice(String),
    /// Start the 8-second Auto-Tune wizard.
    StartAutoTune,
    /// Cancel an in-progress Auto-Tune wizard.
    CancelAutoTune,
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
