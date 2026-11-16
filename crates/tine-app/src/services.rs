//! T16-dependent service wiring: spectrum feed, Auto-Tune runner, "Hear
//! Myself" monitor.
//!
//! T16 (`engine audio services`) is built concurrently; these seams are the
//! documented contracts the app calls once it merges. Each seam is marked
//! `TODO(T16)` and no-ops today so `tine.exe` is fully functional on `main`
//! (shell, tray, CLI, persistence) without the services.

/// The 32-band spectrum shown in `tine meters` and the UI placeholder.
///
/// TODO(T16): fed by `tine_engine::visualizer::Visualizer` (drains the tap
/// at 60 Hz while the window is visible, publishes `UiFrame` through
/// `Triple<UiFrame>`). Until then the spectrum is flat.
#[must_use]
pub fn spectrum() -> [f32; 32] {
    [0.0; 32]
}

/// Whether the user-facing Auto-Tune flow is available (false while stubbed).
pub const AUTOTUNE_AVAILABLE: bool = false;

/// Starts the 8-second Auto-Tune calibration for the current endpoint.
///
/// TODO(T16): wire to `tine_engine::autotune::AutoTuneRunner::start`
/// (bypass lease → raw capture → `AutoCalibrator::push` → `CalProgress` at
/// 30 Hz), then hand `Calibration` to `Command::ApplyCalibration` on
/// completion.
pub fn autotune_start() {
    tracing::warn!(target: "tine::services",
        "Auto-Tune requested but the engine audio services are pending (T16)");
}

/// Applies the "Hear Myself" monitor state (capture processed → play to the
/// chosen device).
///
/// TODO(T16): the engine `Monitor` service owns the cpal streams
/// (start/stop, restart on device change, latency badge); the controller
/// already persists `MonitorSettings` via `Command::SetMonitor`.
pub fn monitor_apply(enabled: bool, device: Option<&str>) {
    if enabled {
        tracing::warn!(target: "tine::services",
            device = device.unwrap_or("<default>"),
            "Hear-Myself requested but the engine audio services are pending (T16)");
    } else {
        tracing::info!(target: "tine::services", "Hear-Myself off");
    }
}
