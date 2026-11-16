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
    ApoBadge, AutoTuneStep, AutoTuneUi, CalFailureUi, CalibrationParamDiff, CalibrationUi, EndpointRow, Macro,
    MonitorUi, UiModel,
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

/// Window-level key handler: `Space` toggles A/B when no focused control took it
/// (a focused text field or dropdown keeps its Space; macro nodes handle Space
/// themselves through [`rune::El::on_key`]).
#[must_use]
pub fn on_key(s: &mut AppState, ev: &KeyEvent, _tx: &mut Tx) -> bool {
    match &ev.key {
        Key::Text(t) if t == " " && !ev.mods.cmd && !ev.mods.ctrl && !ev.mods.alt => {
            // Space toggles A/B bypass when not handled by a focused control.
            s.toggle_bypass_optimistic();
            true
        }
        _ => false,
    }
}
