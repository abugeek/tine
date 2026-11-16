//! Top-level Rune application state, live telemetry frame, and local UI flags.

use std::rc::Rc;

use rune::TextEdit;
use tine_protocol::params::ParamId;
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

/// The single Pro Rack numeric field being edited ("one field at a time").
///
/// The live text lives here; the wizard of views renders a real `text_input` for the
/// active parameter only. `commit` (`views/pro_rack.rs`) parses and clamps it, then
/// pins the parameter and emits `Intent::SetParam`.
#[derive(Clone, Default, PartialEq)]
pub struct ProFieldEdit {
    /// The parameter being edited; `None` when no field is open.
    pub param: Option<ParamId>,
    /// The in-progress text.
    pub edit: TextEdit,
    /// Inline validation message shown next to the field (e.g. "Clamped to -80…-20").
    pub message: Option<String>,
}

impl ProFieldEdit {
    /// Opens the field for `param`, seeded with its current value as plain
    /// numeric text (no unit suffixes; `views::pro_rack::parse_numeric` accepts
    /// them anyway).
    #[must_use]
    pub fn open(param: ParamId, current: &crate::model::UiModel) -> Self {
        let seed = crate::views::pro_rack::numeric_seed(param, current.params.get(param));
        Self {
            param: Some(param),
            edit: TextEdit::new(seed),
            message: None,
        }
    }

    /// Closes the field, discarding any uncommitted text.
    pub fn close(&mut self) {
        self.param = None;
        self.message = None;
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
    /// First-run onboarding step (`1..=4`: mic → APO install → Auto-Tune → finish).
    pub onboarding_step: u8,
    /// Whether an in-flight APO install (from onboarding or the header) is still running.
    pub apo_installing: bool,
    /// The Auto-Tune results card's "Keep my Pro pins" checkbox (D-007).
    pub autotune_keep_pins: bool,
    /// The active Pro Rack numeric field (one at a time).
    pub pro_field: ProFieldEdit,
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
            .field("onboarding_step", &self.onboarding_step)
            .field("apo_installing", &self.apo_installing)
            .field("autotune_keep_pins", &self.autotune_keep_pins)
            .field("pro_field", &self.pro_field.param.map(|p| format!("{p:?}")))
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
            && self.onboarding_step == other.onboarding_step
            && self.apo_installing == other.apo_installing
            && self.autotune_keep_pins == other.autotune_keep_pins
            && self.pro_field == other.pro_field
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
            onboarding_step: 1,
            apo_installing: false,
            autotune_keep_pins: true,
            pro_field: ProFieldEdit::default(),
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
