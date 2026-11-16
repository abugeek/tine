//! Pure Anomaly Rune UI views, view model, theme, and headless verification scenes.
//!
//! Non-RT UI layer; no dependency on `tine-engine`, `cpal`, or `windows`.

use std::cell::RefCell;

use rune::{AppSpec, Cx, El, Key, KeyEvent, Tx};

pub mod fake;
pub mod intent;
pub mod model;
pub mod scenes;
pub mod state;
pub mod theme;
pub mod views;

pub use intent::{Intent, IntentSink, NoopSink};
pub use model::{
    ApoBadge, AutoTuneStep, AutoTuneUi, CalibrationParamDiff, CalibrationUi, EndpointRow, Macro, MonitorUi, UiModel,
};
pub use state::{AppState, LiveFrame, UiLocal};

thread_local! {
    static PENDING_INIT: RefCell<Option<AppState>> = const { RefCell::new(None) };
}

fn take_pending_init() -> AppState {
    PENDING_INIT.with(|slot| slot.borrow_mut().take()).unwrap_or_default()
}

/// Builds the Rune [`AppSpec`] for Tine, wiring `sink` and `init` state.
#[must_use]
pub fn app_spec(sink: impl IntentSink, mut init: AppState) -> AppSpec<AppState> {
    init.set_sink(sink);
    PENDING_INIT.with(|slot| {
        *slot.borrow_mut() = Some(init);
    });
    AppSpec::<AppState> {
        min_size: Some((900.0, 560.0)),
        scenes: scenes::SCENES,
        on_key: Some(on_key),
        ..AppSpec::new("Tine", (1280.0, 800.0), take_pending_init, view)
    }
}

/// Top-level Rune view function.
#[must_use]
pub fn view(s: &AppState, cx: &mut Cx<AppState>) -> El {
    views::root::view(s, cx)
}

fn on_key(s: &mut AppState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match &ev.key {
        Key::Text(t) if t == " " && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt => {
            // Space toggles A/B bypass when not handled by a focused control.
            s.toggle_bypass_optimistic();
            true
        }
        Key::Up | Key::Down => {
            let focused = tx.focused().unwrap_or("");
            let target = Macro::ALL.into_iter().find(|m| m.key() == focused);
            if let Some(m) = target {
                let delta = if ev.mods.shift { 0.1 } else { 0.01 };
                let signed = if matches!(ev.key, Key::Up) { delta } else { -delta };
                let next = m.get(s.model.macros) + signed;
                s.set_macro_optimistic(m, next);
                true
            } else {
                false
            }
        }
        _ => false,
    }
}
