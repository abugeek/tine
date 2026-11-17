//! User-facing engine audio services owned by the tray parent: the live
//! spectrum feed (T16 `Visualizer` + `SpectrumAnalyzer`), the 8-second
//! Auto-Tune runner flow (`tine_engine::autotune`), and the "Hear Myself"
//! monitor (T16 `Monitor`).
//!
//! These are the T17 `TODO(T16)` seams, now backed by the real T16
//! services that merged in #23. Everything lives behind process-wide
//! state so the shell's existing call sites (`services::spectrum()` in
//! `meters`, `services::autotune_start()` in `calibrate`,
//! `services::monitor_apply()` when the monitor setting changes) stay the
//! single entry points; progress, reports, and the latency badge are
//! exposed additively for the control plane (`status`, the UI child).
//!
//! Under D-018 the audio path routes through the in-process
//! virtual-cable loop, which the shell does not start yet (see this
//! task's `## Requests`); until it does there is no APO-side mapping in
//! production, the visualizer stays torn down, and the spectrum reports
//! its floor instead of failing silently.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::{Value, json};
use tine_engine::audio_io::DeviceRef;
use tine_engine::autotune::{AutoTuneConfig, AutoTuneEvent, AutoTuneOutcome, AutoTuneRunner, CalPhase, CaptureSpec};
use tine_engine::model::{AppModel, KeepPins, resolve};
use tine_engine::monitor::Monitor;
use tine_engine::paths::{EnginePaths, PathsError};
use tine_engine::persist;
use tine_engine::shm_client::{LinkState, ShmClient};
use tine_engine::visualizer::{EngineSignals, UiFrame, Visualizer};
use tine_protocol::sync::Triple;
use tine_protocol::telemetry::AudioTelemetryFrame;

/// How long after the last `meters` pull the visualizer pump keeps
/// running before the teardown reaper stops it (the UI child stops
/// polling when its window hides or closes; CLI `meters` keeps a short
/// tail).
const VISUALIZER_TAIL: Duration = Duration::from_secs(10);

/// One service-side spectrum snapshot handed to `meters` consumers.
#[derive(Clone)]
pub struct FrameSnapshot {
    /// 32 band levels in dBFS ([`crate::ui_bridge::SPECTRUM_FLOOR_DB`] is
    /// the floor; normalize with [`crate::ui_bridge::band_level`]).
    pub spectrum: [f32; 32],
}

/// Auto-Tune flow state mirrored for the control plane.
enum AutotuneState {
    /// A run is in flight; mirrors the latest `CalProgress`.
    Running {
        /// Phase the calibrator is in.
        phase: CalPhase,
        /// Fraction of the current phase captured.
        progress: f32,
        /// Live input level in dBFS.
        level_db: f32,
    },
    /// The run produced a report the controller has not applied (the
    /// pipe-spawned flow owns no `ControllerHandle`; see `## Requests`).
    ReportPending {
        /// Human-readable summary of the measured calibration.
        summary: String,
    },
    /// The run failed (or was cancelled); the message is user-facing.
    Failed(String),
}

/// Shared, process-wide audio service state. The shell's app loop owns
/// the conceptual lifecycle; the lazy static removes the need to plumb a
/// services object through every shell call site.
struct Services {
    visualizer: Option<Visualizer>,
    frames: Option<Arc<Triple<UiFrame>>>,
    last_frame: FrameSnapshot,
    last_pull: Option<Instant>,
    monitor: Option<Monitor>,
    autotune: Option<AutotuneState>,
    autotune_cancel: Arc<AtomicBool>,
    messages: Vec<String>,
}

impl Services {
    fn new() -> Self {
        Self {
            visualizer: None,
            frames: None,
            last_frame: FrameSnapshot { spectrum: [0.0; 32] },
            last_pull: None,
            monitor: None,
            autotune: None,
            autotune_cancel: Arc::new(AtomicBool::new(false)),
            messages: Vec::new(),
        }
    }

    /// Logs and queues a user-facing message (never a silent failure).
    fn surface(&mut self, msg: String) {
        tracing::warn!(target: "tine::services", "{msg}");
        self.messages.push(msg);
    }
}

/// Takes `f` over the process-wide [`Services`] and returns its output.
fn with<R>(f: impl FnOnce(&mut Services) -> R) -> R {
    static SERVICES: OnceLock<Mutex<Services>> = OnceLock::new();
    let lock = SERVICES.get_or_init(|| Mutex::new(Services::new()));
    match lock.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => f(&mut poisoned.into_inner()),
    }
}

/// The persisted endpoint the services operate on (D-018: the selected
/// capture device; falls back to a placeholder while nothing is persisted).
#[must_use]
pub fn service_endpoint(paths: &EnginePaths) -> tine_engine::model::EndpointId {
    let settings = persist::load_settings(paths).into_value();
    tine_engine::model::EndpointId::new(settings.selected_endpoint_id.unwrap_or_else(|| String::from("default")))
}

/// The APO-side shared-memory client of the persisted endpoint, when a
/// live mapping exists (D-018 production has none until the router lands;
/// the session-local mappings of tests open fine).
fn endpoint_client(paths: &EnginePaths) -> Option<Arc<ShmClient>> {
    let id = service_endpoint(paths);
    match ShmClient::open(id.as_str()) {
        Ok(client) => Some(Arc::new(client)),
        Err(e) => {
            tracing::debug!(target: "tine::services", endpoint = id.as_str(), error = %e, "no live processing link");
            None
        }
    }
}

/// The 32-band spectrum shown in `tine meters` and the UI canvas.
///
/// Backed by the T16 `Visualizer`: while a consumer keeps pulling (the
/// UI child's ≤ 60 Hz `meters` poll), a pump thread drains the endpoint's
/// post-DSP tap, runs the `SpectrumAnalyzer`, and publishes a `UiFrame`
/// through a lock-free triple buffer; the pull here reads the latest wins
/// (an in-flight frame is simply dropped). When pulls stop for
/// [`VISUALIZER_TAIL`], a reaper stops the pump (thread + timer) so the
/// service costs nothing while unused.
#[must_use]
pub fn spectrum() -> [f32; 32] {
    ok_or_default(&EnginePaths::system(), [0.0; 32], |paths, out| {
        out.copy_from_slice(&spectrum_on(paths).spectrum);
    })
}

/// [`spectrum`] against explicit persistence roots (test seam).
#[must_use]
pub fn spectrum_on(paths: &EnginePaths) -> FrameSnapshot {
    ensure_reaper();
    with(|s| {
        s.last_pull = Some(Instant::now());
        if s.visualizer.is_none()
            && let Some((v, frames)) = start_visualizer(paths)
        {
            s.visualizer = Some(v);
            s.frames = Some(frames);
        }
        if let Some(frames) = &s.frames {
            let mut f = UiFrame::default();
            if frames.latest(&mut f) {
                s.last_frame = FrameSnapshot { spectrum: f.spectrum };
            }
        }
        s.last_frame.clone()
    })
}

/// Runs `f` with valid paths or writes `default` into `out`.
fn ok_or_default<T>(paths: &Result<EnginePaths, PathsError>, default: T, f: impl FnOnce(&EnginePaths, &mut T)) -> T {
    let mut out = default;
    match paths {
        Ok(p) => f(p, &mut out),
        Err(e) => with(|s| s.surface(format!("Audio services: storage paths unavailable: {e}"))),
    }
    out
}

/// Whether the user-facing Auto-Tune flow is available (`true`: the flow
/// is live; failure reasons are reported, never swallowed).
pub const AUTOTUNE_AVAILABLE: bool = true;

/// Starts the 8-second Auto-Tune calibration for the current endpoint.
///
/// Wired to the T16 runner: bypass lease → raw capture
/// (`CaptureSpec::Device`) → the real `AutoCalibrator` (`CalProgress` at
/// 30 Hz mirrored for [`autotune_state`]) → `CalibrationReport` on
/// success. A second concurrent run is rejected politely. The lease is
/// absolute wall-clock time: killing `tine.exe` mid-run self-heals
/// within 15 s (T16). The controller-side apply
/// (`Command::ApplyCalibration`) needs the shell's `ControllerHandle`
/// and is recorded in `## Requests`; the report summary is mirrored
/// instead of being dropped silently.
pub fn autotune_start() {
    let Ok(paths) = EnginePaths::system() else {
        with(|s| s.surface("Auto-Tune: storage paths unavailable".to_owned()));
        return;
    };
    autotune_start_on(&paths);
}

/// [`autotune_start`] against explicit persistence roots (test seam).
pub fn autotune_start_on(paths: &EnginePaths) {
    if with(|s| s.autotune.is_some()) {
        tracing::info!(target: "tine::services", "auto-tune already running");
        return;
    }
    let Some(client) = endpoint_client(paths) else {
        with(|s| {
            s.surface(
                "Auto-Tune needs a live processing link: start Tine's audio routing first (virtual cable, D-018)."
                    .to_owned(),
            );
        });
        return;
    };
    let endpoint = service_endpoint(paths);
    let profile = persist::load_profile(endpoint.as_str(), paths).into_value();
    let model = AppModel {
        monitor: tine_engine::model::MonitorSettings::default(),
        endpoint: Some(endpoint.clone()),
        bypass: false,
        profile: profile.clone(),
    };
    let cfg = AutoTuneConfig {
        client,
        controller: None,
        capture: CaptureSpec::Device {
            device: DeviceRef::Id(endpoint.as_str().to_owned()),
            want_rate: None,
        },
        factory: Arc::new(tine_engine::autotune::AutoCalibratorFactory),
        preset: *tine_dsp::presets::builtin(profile.archetype),
        macros: profile.macros,
        pins: profile.pins,
        current: resolve(&model),
        keep_pins: KeepPins::Keep,
    };
    let cancel = with(|s| {
        s.autotune_cancel = Arc::new(AtomicBool::new(false));
        Arc::clone(&s.autotune_cancel)
    });
    with(|s| {
        s.autotune = Some(AutotuneState::Running {
            phase: CalPhase::Countdown,
            progress: 0.0,
            level_db: -90.0,
        });
    });
    let mut handle = AutoTuneRunner::start(cfg);
    // Feed thread: mirrors progress at the runner's cadence until exactly
    // one terminal event, then settles the mirror. It also honors an
    // orderly cancel requested through [`autotune_cancel`].
    if std::thread::Builder::new()
        .name("tine-autotune-feed".to_owned())
        .spawn(move || {
            loop {
                if cancel.load(Ordering::Acquire) {
                    handle.cancel();
                }
                let ev = match handle.events().recv_timeout(Duration::from_millis(100)) {
                    Ok(ev) => ev,
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                };
                match ev {
                    AutoTuneEvent::Started { rate_hz } => {
                        tracing::info!(target: "tine::services", rate_hz, "auto-tune capture started");
                    }
                    AutoTuneEvent::Progress(p) => with(|s| {
                        s.autotune = Some(AutotuneState::Running {
                            phase: p.phase,
                            progress: p.phase_progress,
                            level_db: p.live_level_db,
                        });
                    }),
                    AutoTuneEvent::Done(outcome) => {
                        with(|s| match outcome {
                            AutoTuneOutcome::Applied(report) => {
                                s.autotune = Some(AutotuneState::ReportPending {
                                    summary: format!(
                                        "measured noise {:+.1} dBFS, speech {:+.1} dBFS, verified peak {:+.1} dBFS",
                                        report.calibration.noise_rms_db,
                                        report.calibration.speech_rms_db,
                                        report.simulated_peak_db
                                    ),
                                });
                            }
                            AutoTuneOutcome::Failed(failure) => {
                                s.autotune = Some(AutotuneState::Failed(format!(
                                    "Auto-Tune rejected the capture: {failure:?}"
                                )));
                            }
                            AutoTuneOutcome::Error(e) => {
                                s.autotune = Some(AutotuneState::Failed(format!("Auto-Tune failed: {e}")));
                            }
                        });
                        break;
                    }
                }
            }
        })
        .is_err()
    {
        with(|s| {
            s.autotune = None;
            s.surface("Auto-Tune: progress thread spawn failed".to_owned());
        });
    }
}

/// Requests an orderly stop of the Auto-Tune runner: the feed thread sees
/// the flag, cancels the run, and settles on the runner's terminal
/// event (which also clears the bypass lease, T16).
pub fn autotune_cancel() {
    with(|s| s.autotune_cancel.store(true, Ordering::Release));
}

/// JSON-able snapshot of the Auto-Tune flow for `status`-side consumers.
#[must_use]
pub fn autotune_state() -> Value {
    with(|s| match &s.autotune {
        None => json!({ "flow": "idle" }),
        Some(AutotuneState::Running {
            phase,
            progress,
            level_db,
        }) => {
            json!({ "flow": "running", "phase": format!("{phase:?}"), "progress": progress, "level_db": level_db })
        }
        Some(AutotuneState::ReportPending { summary }) => {
            json!({ "flow": "report-pending", "summary": summary })
        }
        Some(AutotuneState::Failed(msg)) => json!({ "flow": "failed", "message": msg }),
    })
}

/// User-facing service messages since the last call (error surfacing).
#[must_use]
pub fn take_messages() -> Vec<String> {
    with(|s| std::mem::take(&mut s.messages))
}

/// Applies the "Hear Myself" monitor state (capture the processed
/// endpoint → play to the chosen device).
///
/// The T16 `Monitor` owns the cpal streams from here: start on enable,
/// restart on a device change, stop on disable (dropping the streams
/// releases the devices); failures surface and never panic. The
/// controller persists `MonitorSettings` through `Command::SetMonitor`
/// (the shell already sends it alongside this call).
pub fn monitor_apply(enabled: bool, device: Option<&str>) {
    let Ok(paths) = EnginePaths::system() else {
        with(|s| s.surface("Hear Myself: storage paths unavailable".to_owned()));
        return;
    };
    monitor_apply_on(&paths, enabled, device);
}

/// [`monitor_apply`] against explicit persistence roots (test seam).
pub fn monitor_apply_on(paths: &EnginePaths, enabled: bool, device: Option<&str>) {
    if !enabled {
        with(|s| {
            if let Some(m) = s.monitor.take() {
                m.stop();
                tracing::info!(target: "tine::services", "Hear Myself off (streams stopped)");
            }
        });
        return;
    }
    let endpoint = service_endpoint(paths);
    let in_ref = DeviceRef::Id(endpoint.as_str().to_owned());
    let out = device.map_or_else(DeviceRef::default, DeviceRef::parse);
    with(|s| match &mut s.monitor {
        Some(m) => {
            if let Err(e) = m.restart(&in_ref, &out) {
                s.surface(format!("Hear Myself restart failed: {e}"));
                s.monitor = None;
            }
        }
        None => match Monitor::start(&in_ref, &out) {
            Ok(m) => {
                tracing::info!(target: "tine::services",
                    endpoint = endpoint.as_str(),
                    latency_ms = m.latency_ms(),
                    "Hear Myself started");
                s.monitor = Some(m);
            }
            Err(e) => s.surface(format!("Hear Myself failed to start: {e}")),
        },
    });
}

/// JSON-able "Hear Myself" snapshot (on-state, latency badge, health).
#[must_use]
pub fn monitor_status() -> Value {
    with(|s| match &s.monitor {
        None => json!({ "on": false }),
        Some(m) => json!({ "on": true, "latency_ms": m.latency_ms(), "error": m.has_error() }),
    })
}

/// Tears every service down (shutdown path): monitor streams, the
/// Auto-Tune mirror, and the visualizer pump. Idempotent.
pub fn shutdown() {
    with(|s| {
        if let Some(m) = s.monitor.take() {
            m.stop();
        }
        s.autotune = None;
        s.autotune_cancel.store(true, Ordering::Release);
        if let Some(mut v) = s.visualizer.take() {
            v.stop();
        }
        s.frames = None;
    });
}

// ---------------------------------------------------------------------------
// Visualizer plumbing
// ---------------------------------------------------------------------------

/// One-shot teardown reaper: when the last pull is older than
/// [`VISUALIZER_TAIL`], it stops the pump so a closed window leaves no
/// threads or timer wakeups behind (the 5 s cadence costs ~0 % CPU).
fn ensure_reaper() {
    static REAPER: OnceLock<()> = OnceLock::new();
    REAPER.get_or_init(|| {
        let spawned = std::thread::Builder::new()
            .name("tine-services-reaper".to_owned())
            .spawn(|| {
                loop {
                    std::thread::sleep(Duration::from_secs(VISUALIZER_TAIL.as_secs() / 2));
                    with(|s| {
                        let stale = s.last_pull.is_some_and(|t| t.elapsed() > VISUALIZER_TAIL);
                        if stale && let Some(mut v) = s.visualizer.take() {
                            v.stop();
                            s.frames = None;
                            tracing::info!(target: "tine::services", "spectrum pump torn down (no consumer)");
                        }
                    });
                }
            })
            .is_ok();
        if !spawned {
            tracing::warn!(target: "tine::services", "teardown reaper spawn failed");
        }
    });
}

/// Adapts an open shared-memory client to the visualizer's signals trait
/// (telemetry + tap + rate from the mapping; heartbeat from the link
/// state). This pump is the mapping's single tap reader by convention —
/// nothing else in the parent ever drains the tap.
struct ClientSignals(Arc<ShmClient>);

impl EngineSignals for ClientSignals {
    fn telemetry(&self) -> AudioTelemetryFrame {
        self.0.telemetry()
    }

    fn drain_tap(&self, buf: &mut [f32]) -> usize {
        self.0.drain_tap(buf)
    }

    fn tap_rate_hz(&self) -> u32 {
        self.0.sink_format().0
    }

    fn heartbeat_ok(&self) -> bool {
        matches!(self.0.state(), LinkState::Linked { .. })
    }
}

/// Starts the pump when the persisted endpoint has a live mapping.
fn start_visualizer(paths: &EnginePaths) -> Option<(Visualizer, Arc<Triple<UiFrame>>)> {
    let client = endpoint_client(paths)?;
    if !matches!(client.state(), LinkState::Linked { .. }) {
        // A mapping exists but its heartbeat is not fresh: pumping would
        // only burn a thread on empty reads.
        return None;
    }
    let v = Visualizer::start(ClientSignals(client));
    let frames = v.frames();
    tracing::info!(target: "tine::services", "spectrum pump started");
    Some((v, frames))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_paths() -> EnginePaths {
        // The tempdir's guard drops at test end; the paths only have to
        // outlive this function (all loads from empty dirs return
        // `Missing` and fall back to defaults).
        let dir = tempfile::tempdir().expect("tempdir");
        EnginePaths::from_roots(
            dir.path().join("app"),
            dir.path().join("prog"),
            dir.path().join("local"),
        )
    }

    #[test]
    fn spectrum_stays_on_the_floor_without_a_live_link() {
        let paths = temp_paths();
        let snap = spectrum_on(&paths);
        assert!(
            snap.spectrum.iter().all(|&b| b == 0.0),
            "no mapping → floor, not a panic"
        );
        assert_eq!(spectrum(), [0.0; 32]);
    }

    #[test]
    fn autotune_without_a_link_surfaces_a_message() {
        let paths = temp_paths();
        super::shutdown();
        autotune_start_on(&paths);
        assert!(
            take_messages()
                .iter()
                .any(|m| m.contains("Auto-Tune needs a live processing link"))
        );
        assert_eq!(autotune_state()["flow"], "idle");
    }

    #[test]
    fn monitor_off_tears_down_cleanly() {
        monitor_apply(false, None);
        assert_eq!(monitor_status()["on"], false);
    }

    #[test]
    fn monitor_start_failure_surfaces_a_user_message() {
        let paths = temp_paths();
        monitor_apply_on(&paths, true, Some("__nonexistent_tine_output_device__"));
        assert!(
            take_messages()
                .iter()
                .any(|m| m.contains("Hear Myself failed to start"))
        );
        assert_eq!(monitor_status()["on"], false);
        monitor_apply(false, None);
    }
}
