//! `--ui`: the Rune window child process (D-017). Spawning a short-lived
//! child for the window means closing the window releases 100 % of the GPU
//! memory, threads, and driver state with the OS, keeping the parent
//! tray app inside the idle budgets.
//!
//! The view is a **placeholder** until T21 swaps in `tine-ui`: it renders the
//! live `status` snapshot pulled from the parent's command pipe, and a Close
//! button. Spectrum/visualizer wiring lands with T16/T21 (see `services.rs`).

use rune::tok::{accent, radius, space, theme, type_};
use rune::{App, AppSpec, Cx, El, Key, KeyEvent, Tx, col, el, row, text};

use serde_json::Value;

use crate::proto::{PipeRequest, PipeResponse};
use crate::single_instance;

/// How often the placeholder view refreshes its status (spec allows ≤ 60 Hz;
/// this is a status snapshot, not the visualizer feed).
const STATUS_POLL: std::time::Duration = std::time::Duration::from_millis(500);

/// Placeholder UI state.
struct UiState {
    /// Latest status payload from the parent (`None` until it answers).
    status: Option<Value>,
    /// Server error text when the parent cannot be reached.
    error: Option<String>,
    /// Poll tick count (drives the "live" indicator).
    ticks: u64,
}

/// Pulls one `status` snapshot from the running instance.
fn fetch_status() -> Result<Value, String> {
    let name = single_instance::pipe_name_for_user();
    match single_instance::send_request(&name, &PipeRequest::Status) {
        Ok(PipeResponse { ok: true, data: Some(data), .. }) => Ok(data),
        Ok(resp) => Err(resp.error.unwrap_or_else(|| "unexpected reply".to_owned())),
        Err(e) => Err(e),
    }
}

fn field(s: &UiState, key: &str) -> String {
    let missing = "—".to_owned();
    s.status
        .as_ref()
        .and_then(|st| st.get(key))
        .map(|v| match v {
            Value::String(t) => t.clone(),
            Value::Bool(b) => b.to_string(),
            Value::Null => missing.clone(),
            other => other.to_string(),
        })
        .unwrap_or(missing)
}

fn view(s: &UiState, cx: &mut Cx<UiState>) -> El {
    let t = theme(cx.system_dark());
    let accent = accent(0, cx.system_dark());
    let line = |key: &'static str, label: &'static str| {
        row()
            .key(key)
            .gap(space::S3)
            .child(
                el()
                    .w(110.0)
                    .child(text(label).font(type_::LABEL).color(t.text_secondary)),
            )
            .child(text(field(s, key)).font(type_::BODY).color(t.text))
    };
    let live_dot = if s.ticks.is_multiple_of(2) { accent } else { t.text_secondary };
    col()
        .key("root")
        .w_full()
        .h_full()
        .fill(t.bg)
        .px(space::S6)
        .py(space::S6)
        .gap(space::S3)
        .child(
            row()
                .key("header")
                .gap(space::S3)
                .center()
                .child(text("Tine").font(type_::TITLE).color(t.text))
                .child(el().key("live-dot").size(8.0, 8.0).radius(4.0).fill(live_dot)),
        )
        .child(
            text("Placeholder window — the full tine-ui lands in T21.")
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(line("mic_name", "Microphone"))
        .child(line("archetype", "Archetype"))
        .child(line("bypass", "Bypass"))
        .child(line("monitor", "Hear Myself"))
        .child(line("state", "State"))
        .child(line("link", "Link"))
        .child(line("autostart", "Autostart"))
        .child(
            text(s.error.clone().map(|e| format!("{e}")).unwrap_or_default())
                .font(type_::BODY)
                .color(t.text_secondary),
        )
        .child(
            row()
                .key("actions")
                .gap(space::S2)
                .child(
                    row()
                        .key("close-button")
                        .role("button")
                        .label("Close")
                        .focusable(true)
                        .h(30.0)
                        .px(space::S3)
                        .center()
                        .radius(radius::MD)
                        .fill(t.card)
                        .border(1.0, t.card_border)
                        .on_press_tx(|_: &mut UiState, tx| tx.close_window())
                        .child(text("Close").font(type_::LABEL).color(t.text)),
                ),
        )
}

fn on_key(_: &mut UiState, ev: &KeyEvent, tx: &mut Tx) -> bool {
    match ev.key {
        Key::Command('w') | Key::Command('q') | Key::Escape => tx.close_window(),
        _ => return false,
    }
    true
}

/// Runs the UI child (called from `main` for `tine --ui`).
pub fn run_ui_child() {
    if let Ok(paths) = tine_engine::paths::EnginePaths::system() {
        let _ = crate::logging::init(&paths.logs_dir());
    }
    let app = App::new(AppSpec::<UiState> {
        scenes: &[("default", |_| {})],
        on_key: Some(on_key),
        ..AppSpec::new("Tine", (520.0, 460.0), UiState::default, view)
    })
    .args(Vec::new())
    .init(|| {
        // TODO(T16): start the Visualizer service here; it publishes
        // `UiFrame` at 60 Hz while the window is visible and is fully
        // stopped when this process exits (window closed). Until then the
        // placeholder polls the parent's status snapshot (below).
        tracing::info!(target: "tine::ui", "UI child started (placeholder view)");
        UiState {
            status: None,
            error: None,
            ticks: 0,
        }
    })
    .on_window(|_| {
        tracing::debug!(target: "tine::ui", "window opened");
    });

    let handle = app.handle();
    std::thread::Builder::new()
        .name("tine-status-poll".to_owned())
        .spawn(move || loop {
            match fetch_status() {
                Ok(status) => {
                    handle.update(move |s, _| {
                        s.status = Some(status);
                        s.error = None;
                        s.ticks = s.ticks.wrapping_add(1);
                    });
                }
                Err(e) => {
                    handle.update(move |s, _| {
                        s.error = Some(e);
                        s.ticks = s.ticks.wrapping_add(1);
                    });
                }
            }
            std::thread::sleep(STATUS_POLL);
        })
        .ok();

    app.run();
}

impl Default for UiState {
    fn default() -> Self {
        Self {
            status: None,
            error: None,
            ticks: 0,
        }
    }
}
