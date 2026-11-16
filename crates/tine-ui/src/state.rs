//! Top-level Rune application state, live telemetry frame, and local UI flags.

use std::rc::Rc;

use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::intent::{Intent, IntentSink, NoopSink};
use crate::model::{Macro, UiModel};

/// High-frequency live metering & spectrum frame updated at ~60 Hz.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LiveFrame {
    /// Real-time telemetry published by the APO.
    pub telemetry: AudioTelemetryFrame,
    /// 32-band voice spectrum magnitudes normalized to `0.0..=1.0`.
    pub spectrum: [f32; 32],
}

impl Default for LiveFrame {
    fn default() -> Self {
        Self {
            telemetry: AudioTelemetryFrame {
                input_rms_db: -60.0,
                input_peak_db: -60.0,
                output_rms_db: -60.0,
                output_peak_db: -60.0,
                ..AudioTelemetryFrame::default()
            },
            spectrum: [0.0; 32],
        }
    }
}

/// Ephemeral UI-only state not persisted or owned by the engine.
#[derive(Clone)]
pub struct UiLocal {
    /// Whether the bottom Surgical Pro Rack drawer is expanded.
    pub pro_drawer_open: bool,
    /// Currently hovered or actively dragged macro node (drives spring tooltip).
    pub hovered_macro: Option<Macro>,
    /// Key of the currently focused element, if tracked locally.
    pub focused_key: Option<String>,
    /// Whether the monitor/settings popover is open.
    pub settings_open: bool,
    /// Whether the user dismissed the first-run onboarding overlay locally.
    pub onboarding_dismissed: bool,
    pub(crate) sink: Rc<dyn IntentSink>,
}

impl core::fmt::Debug for UiLocal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UiLocal")
            .field("pro_drawer_open", &self.pro_drawer_open)
            .field("hovered_macro", &self.hovered_macro)
            .field("focused_key", &self.focused_key)
            .field("settings_open", &self.settings_open)
            .field("onboarding_dismissed", &self.onboarding_dismissed)
            .finish_non_exhaustive()
    }
}

impl PartialEq for UiLocal {
    fn eq(&self, other: &Self) -> bool {
        self.pro_drawer_open == other.pro_drawer_open
            && self.hovered_macro == other.hovered_macro
            && self.focused_key == other.focused_key
            && self.settings_open == other.settings_open
            && self.onboarding_dismissed == other.onboarding_dismissed
    }
}

impl Default for UiLocal {
    fn default() -> Self {
        Self {
            pro_drawer_open: false,
            hovered_macro: None,
            focused_key: None,
            settings_open: false,
            onboarding_dismissed: false,
            sink: Rc::new(NoopSink),
        }
    }
}

/// Complete Rune application state passed to [`crate::view`].
#[derive(Clone, Debug, Default, PartialEq)]
pub struct AppState {
    /// Authoritative model pushed by the engine.
    pub model: UiModel,
    /// Live 60 Hz telemetry and spectrum.
    pub live: LiveFrame,
    /// Ephemeral UI-local state.
    pub ui: UiLocal,
}

impl AppState {
    /// Creates an [`AppState`] from a [`UiModel`] with default live/local state.
    #[must_use]
    pub fn from_model(model: UiModel) -> Self {
        Self {
            model,
            live: LiveFrame::default(),
            ui: UiLocal::default(),
        }
    }

    /// Attaches an [`IntentSink`] to this state.
    pub fn set_sink(&mut self, sink: impl IntentSink) {
        self.ui.sink = Rc::new(sink);
    }

    /// Emits an [`Intent`] to the attached [`IntentSink`].
    pub fn emit(&self, intent: Intent) {
        self.ui.sink.send(intent);
    }

    /// Optimistically updates a macro locally and emits [`Intent::SetMacro`].
    pub fn set_macro_optimistic(&mut self, m: Macro, value: f32) {
        m.set(&mut self.model.macros, value);
        let clamped = m.get(self.model.macros);
        self.emit(Intent::SetMacro(m, clamped));
    }

    /// Optimistically toggles bypass locally and emits [`Intent::ToggleBypass`].
    pub fn toggle_bypass_optimistic(&mut self) {
        self.model.bypass = !self.model.bypass;
        self.emit(Intent::ToggleBypass);
    }
}
