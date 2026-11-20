//! Top-level Rune application state, live telemetry frame, and local UI flags.

use std::rc::Rc;

use rune::TextEdit;
use tine_dsp::calibrator::{LIVE_COUNTDOWN_S, VoiceStyle};
use tine_protocol::params::ParamId;
use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::intent::{Intent, IntentSink, NoopSink};
use crate::model::{AutoTuneStep, AutoTuneUi, Macro, UiModel};
use crate::theme::{ThemeMode, TineTheme};

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
    /// Currently hovered or actively dragged macro node (drives spring tooltip & coach bar).
    pub hovered_macro: Option<Macro>,
    /// Currently hovered or actively dragged parameter control (drives beginner hint bar).
    pub hovered_param: Option<ParamId>,
    /// Key of the currently focused element, if tracked locally.
    pub focused_key: Option<String>,
    /// Whether the user dismissed the first-run onboarding overlay locally.
    pub onboarding_dismissed: bool,
    /// Whether the user dismissed the active "app using raw mic" nudge banner.
    pub raw_mic_banner_dismissed: bool,
    /// First-run onboarding step (`1..=4`: mic → APO install → Auto-Tune → finish).
    pub onboarding_step: u8,
    /// Whether an in-flight APO install (from onboarding or the header) is still running.
    pub apo_installing: bool,
    /// The Auto-Tune results card's "Keep my Pro pins" checkbox (D-007).
    pub autotune_keep_pins: bool,
    /// Selected post-calibration voice style recommendation on the Auto-Tune results card.
    pub autotune_style: VoiceStyle,
    /// Active Auto-Tune voice-loop audition mode (`None` = stopped, `Some(false)` = Tuned, `Some(true)` = Raw).
    pub autotune_audition_raw: Option<bool>,
    /// The active Pro Rack numeric field (one at a time).
    pub pro_field: ProFieldEdit,
    /// Active light or dark theme mode ("Decky Midnight" vs "Decky Daylight").
    pub theme_mode: ThemeMode,
    pub(crate) sink: Rc<dyn IntentSink>,
}

impl core::fmt::Debug for UiLocal {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("UiLocal")
            .field("pro_drawer_open", &self.pro_drawer_open)
            .field("hovered_macro", &self.hovered_macro)
            .field("hovered_param", &self.hovered_param)
            .field("focused_key", &self.focused_key)
            .field("onboarding_dismissed", &self.onboarding_dismissed)
            .field("raw_mic_banner_dismissed", &self.raw_mic_banner_dismissed)
            .field("onboarding_step", &self.onboarding_step)
            .field("apo_installing", &self.apo_installing)
            .field("autotune_keep_pins", &self.autotune_keep_pins)
            .field("autotune_style", &self.autotune_style)
            .field("autotune_audition_raw", &self.autotune_audition_raw)
            .field("pro_field", &self.pro_field.param.map(|p| format!("{p:?}")))
            .field("theme_mode", &self.theme_mode)
            .finish_non_exhaustive()
    }
}

impl PartialEq for UiLocal {
    fn eq(&self, other: &Self) -> bool {
        self.pro_drawer_open == other.pro_drawer_open
            && self.hovered_macro == other.hovered_macro
            && self.hovered_param == other.hovered_param
            && self.focused_key == other.focused_key
            && self.onboarding_dismissed == other.onboarding_dismissed
            && self.raw_mic_banner_dismissed == other.raw_mic_banner_dismissed
            && self.onboarding_step == other.onboarding_step
            && self.apo_installing == other.apo_installing
            && self.autotune_keep_pins == other.autotune_keep_pins
            && self.autotune_style == other.autotune_style
            && self.autotune_audition_raw == other.autotune_audition_raw
            && self.pro_field == other.pro_field
            && self.theme_mode == other.theme_mode
    }
}

impl Default for UiLocal {
    fn default() -> Self {
        Self {
            pro_drawer_open: false,
            hovered_macro: None,
            hovered_param: None,
            focused_key: None,
            onboarding_dismissed: false,
            raw_mic_banner_dismissed: false,
            onboarding_step: 1,
            apo_installing: false,
            autotune_keep_pins: true,
            autotune_style: VoiceStyle::AutoBalanced,
            autotune_audition_raw: None,
            pro_field: ProFieldEdit::default(),
            theme_mode: ThemeMode::Dark,
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

    /// Optimistically updates a macro locally (including unpinning driven
    /// parameters so dragging a main knob or Voice FX always updates its
    /// sub-knobs and DSP response), resolves `DspParams` at 60 Hz, and emits
    /// [`Intent::SetMacro`].
    pub fn set_macro_optimistic(&mut self, m: Macro, value: f32) {
        let dsp_m = match m {
            Macro::Purity => tine_dsp::macros::Macro::Purity,
            Macro::Body => tine_dsp::macros::Macro::Body,
            Macro::Presence => tine_dsp::macros::Macro::Presence,
            Macro::Leveler => tine_dsp::macros::Macro::Leveler,
        };
        for &id in tine_dsp::macros::MacroMapper::drives(dsp_m) {
            if self.model.pins.is_pinned(id) {
                self.model.pins.unpin(id);
                self.emit(Intent::Unpin(id));
            }
        }
        m.set(&mut self.model.macros, value);
        let clamped = m.get(self.model.macros);
        self.resolve_params_optimistic();
        self.emit(Intent::SetMacro(m, clamped));
    }

    /// Resets all pins and restores the 4 macros to the current archetype's defaults.
    pub fn reset_deck_to_neutral(&mut self) {
        if !self.model.pins.is_empty() {
            self.model.pins.clear();
            self.emit(Intent::UnpinAll);
        }
        let d = tine_dsp::presets::builtin(self.model.archetype).macro_defaults;
        self.set_macro_optimistic(Macro::Purity, d.purity);
        self.set_macro_optimistic(Macro::Body, d.body);
        self.set_macro_optimistic(Macro::Presence, d.presence);
        self.set_macro_optimistic(Macro::Leveler, d.leveler);
    }

    /// Relinks a pinned parameter back to macro/archetype control on double-click,
    /// or sets an unpinned parameter back to the archetype baseline.
    pub fn reset_param_or_unpin(&mut self, id: ParamId) {
        if self.model.pins.is_pinned(id) {
            self.model.pins.unpin(id);
            self.resolve_params_optimistic();
            self.emit(Intent::Unpin(id));
        } else {
            let base = tine_dsp::presets::builtin(self.model.archetype).base.get(id);
            self.set_param_optimistic(id, base);
        }
    }

    /// Optimistically switches the microphone archetype locally (updating the
    /// Golden Line curve immediately) and emits [`Intent::SetArchetype`].
    pub fn set_archetype_optimistic(&mut self, arch: tine_protocol::presets::Archetype) {
        self.model.archetype = arch;
        self.resolve_params_optimistic();
        self.emit(Intent::SetArchetype(arch));
    }

    /// Re-computes [`UiModel::params`] from the active archetype, calibration,
    /// macro positions, and pinned Pro overrides.
    pub fn resolve_params_optimistic(&mut self) {
        let preset = tine_dsp::presets::builtin(self.model.archetype);
        let mut resolved = tine_dsp::macros::MacroMapper::apply(
            preset,
            self.model.active_calibration.as_ref(),
            self.model.macros,
            self.model.pins,
            &self.model.params,
        );
        // Preserve any non-macro Pro-pinned parameters already present in `self.model.params`.
        for i in 0..tine_protocol::params::PARAM_COUNT {
            if let Some(id) = ParamId::from_index(i)
                && self.model.pins.is_pinned(id)
            {
                resolved.set(id, self.model.params.get(id));
            }
        }
        resolved.bypass = self.model.bypass;
        self.model.params = resolved;
    }

    /// Optimistically opens the Auto-Tune wizard in `Prepare` and emits [`Intent::StartAutoTune`].
    pub fn start_autotune_optimistic(&mut self) {
        self.model.autotune = AutoTuneUi {
            step: AutoTuneStep::Prepare {
                remaining_secs: LIVE_COUNTDOWN_S,
            },
            progress: 0.0,
            level_db: self.live.telemetry.input_rms_db.max(-60.0),
            prompt_text: String::new(),
            voiced_ms: 0,
        };
        self.emit(Intent::StartAutoTune);
    }

    /// Returns the active [`TineTheme`] for the current Light / Dark mode.
    #[must_use]
    pub fn theme(&self) -> TineTheme {
        TineTheme::for_mode(self.ui.theme_mode)
    }

    /// Toggles between Dark ("Decky Midnight") and Light ("Decky Daylight") modes.
    pub fn toggle_theme(&mut self) {
        self.ui.theme_mode = self.ui.theme_mode.toggle();
    }

    fn set_raw_param(&mut self, id: ParamId, value: f32) {
        self.model.params.set(id, value);
        self.model.pins.pin(id);
        self.emit(Intent::SetParam(id, self.model.params.get(id)));
    }

    /// Optimistically sets a DSP parameter, pins it (D-007), and emits [`Intent::SetParam`].
    ///
    /// Editing a parameter of a disabled stage automatically enables that stage
    /// (and adjusting manual compressor makeup turns off Auto Makeup) so every
    /// control produces an immediate audible effect.
    pub fn set_param_optimistic(&mut self, id: ParamId, value: f32) {
        match id {
            ParamId::HpfFreqHz if !self.model.params.hpf_enabled => {
                self.set_raw_param(ParamId::HpfEnabled, 1.0);
            }
            ParamId::GateThresholdDb
            | ParamId::GateRatio
            | ParamId::GateHysteresisDb
            | ParamId::GateAttackMs
            | ParamId::GateReleaseMs
                if !self.model.params.gate_enabled =>
            {
                self.set_raw_param(ParamId::GateEnabled, 1.0);
            }
            ParamId::DeesserFreqHz | ParamId::DeesserThreshDb | ParamId::DeesserAmountDb
                if !self.model.params.deesser_enabled =>
            {
                self.set_raw_param(ParamId::DeesserEnabled, 1.0);
            }
            ParamId::CompThreshDb
            | ParamId::CompRatio
            | ParamId::CompAttackMs
            | ParamId::CompReleaseMs
            | ParamId::CompKneeDb
                if !self.model.params.comp_enabled =>
            {
                self.set_raw_param(ParamId::CompEnabled, 1.0);
            }
            ParamId::CompMakeupDb => {
                if !self.model.params.comp_enabled {
                    self.set_raw_param(ParamId::CompEnabled, 1.0);
                }
                if self.model.params.comp_auto_makeup {
                    self.set_raw_param(ParamId::CompAutoMakeup, 0.0);
                }
            }
            _ => {}
        }
        self.set_raw_param(id, value);
    }

    /// Optimistically toggles bypass locally and emits [`Intent::ToggleBypass`].
    pub fn toggle_bypass_optimistic(&mut self) {
        self.model.bypass = !self.model.bypass;
        self.model.params.bypass = self.model.bypass;
        self.emit(Intent::ToggleBypass);
    }

    /// Previews the pending Auto-Tune calibration with `style`, `keep_pins`, and `audition_raw`,
    /// updating `self.model.params` optimistically so the background EQ curve and Results card
    /// reflect the selected style immediately, and emitting [`Intent::PreviewCalibration`].
    pub fn preview_autotune(&mut self, style: VoiceStyle, keep_pins: bool, audition_raw: Option<bool>) {
        self.ui.autotune_style = style;
        self.ui.autotune_keep_pins = keep_pins;
        self.ui.autotune_audition_raw = audition_raw;
        let preset = tine_dsp::presets::builtin(self.model.archetype);
        if let Some(cal) = self.model.pending_calibration.or_else(|| {
            self.model
                .calibration
                .as_ref()
                .map(|c| crate::views::canvas::calibration_from_ui(c, &preset.base))
        }) {
            let macros = tine_dsp::calibrator::recommend_macros(&cal, self.model.archetype, style, self.model.macros);
            let mut pins = self.model.pins;
            if !keep_pins {
                for id in tine_dsp::calibrator::CHANGED_PARAMS {
                    pins.unpin(id);
                }
                if style != VoiceStyle::KeepCurrent {
                    for m in tine_dsp::macros::Macro::ALL {
                        for &id in tine_dsp::macros::MacroMapper::drives(m) {
                            pins.unpin(id);
                        }
                    }
                }
            }
            let mut resolved =
                tine_dsp::macros::MacroMapper::apply(preset, Some(&cal), macros, pins, &self.model.params);
            resolved.bypass = self.model.bypass;
            self.model.params = resolved;
        }
        self.emit(Intent::PreviewCalibration {
            style,
            keep_pins,
            audition_raw,
        });
    }
}
