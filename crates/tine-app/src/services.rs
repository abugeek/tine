//! User-facing engine audio services owned by the tray parent: the live
//! spectrum feed (T16 `Visualizer` + `SpectrumAnalyzer`), the 8-second
//! Auto-Tune runner flow (`tine_engine::autotune`), the "Hear Myself"
//! monitor (T16 `Monitor`), and — per D-018 — the **virtual-cable routing
//! loop** (`tine_engine::virtual_cable::VirtualCableRouter`) plus the
//! parameter block the controller publishes into.
//!
//! Everything lives behind process-wide state so the shell's existing call
//! sites (`services::spectrum()` in `meters`, `services::autotune_start()`
//! in `calibrate`, `services::monitor_apply()` when the monitor setting
//! changes) stay the single entry points; progress, reports, router
//! health, and the latency badge are exposed additively for the control
//! plane (`status`, the pipe, the UI child).
//!
//! Topology (D-018, in-process): the controller resolves the model into
//! [`services::param_block`] while the router captures the mic, runs the
//! `DspPipeline` against the same block, and plays the result into the
//! virtual cable; telemetry and post-DSP tap land in the process-wide
//! triple buffer and ring the visualizer drains. Auto-Tune's bypass lease
//! normally lives in the APO-side shared memory; with no APO in this
//! topology the app hosts the lease mapping itself and honors it
//! (state-flag mirror + DSP bypass assert), so the unmodified T16 runner
//! calibrates against the raw capture.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};
use tine_engine::audio_io::DeviceRef;
use tine_engine::autotune::{
    AutoTuneConfig, AutoTuneEvent, AutoTuneOutcome, AutoTuneRunner, CalFailure, CalPhase, CaptureSpec,
};
use tine_engine::model::{AppModel, EndpointId, KeepPins, resolve};
use tine_engine::monitor::Monitor;
use tine_engine::paths::{EnginePaths, PathsError};
use tine_engine::persist;
use tine_engine::shm_client::{LinkState, ShmClient};
use tine_engine::virtual_cable::VirtualCableRouter;
use tine_engine::visualizer::{EngineSignals, InProcessSignals, UiFrame, Visualizer};
use tine_protocol::params::DspParams;
use tine_protocol::presets::Calibration;
use tine_protocol::shm::{ShmLayout, init_layout_in_place};
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};
use windows::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};

/// How long after the last `meters` pull the visualizer pump keeps
/// running before the teardown reaper stops it (the UI child stops
/// polling when its window hides or closes; CLI `meters` keeps a short
/// tail).
const VISUALIZER_TAIL: Duration = Duration::from_secs(10);

/// Post-DSP tap capacity shared with the routing loop (341 ms @ 48 kHz —
/// far above the 60 Hz visualizer tick's 16 ms of samples).
pub const ROUTER_TAP_N: usize = 16_384;

/// Assumed tap rate when the capture device's default rate cannot be
/// queried (band edges may drift slightly; levels are unaffected).
const ROUTER_FALLBACK_RATE: u32 = 48_000;

/// Wall-clock ms since the Unix epoch (lease clocks are absolute time).
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// One service-side meters snapshot handed to `meters` consumers; the
/// pump's single `UiFrame` read per pull is shared (D-026: the
/// `UiFrame` triple has exactly one reader — this path, under the
/// services lock).
#[derive(Clone)]
pub struct FrameSnapshot {
    /// 32 band levels in dBFS ([`crate::ui_bridge::SPECTRUM_FLOOR_DB`] is
    /// the floor; normalize with [`crate::ui_bridge::band_level`]).
    pub spectrum: [f32; 32],
    /// The telemetry frame the bands came with (matches to the tick).
    pub telemetry: AudioTelemetryFrame,
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
        /// Voiced audio seen so far in the voice phase (ms).
        voiced_ms: u32,
    },
    /// The run produced a report the user has not applied or discarded
    /// yet; the pipe's `calibrate_apply` / `calibrate_discard` decide its
    /// fate.
    ReportPending {
        /// The measured report (kept whole: the results card shows its
        /// numbers and per-parameter changes).
        report: tine_engine::autotune::CalibrationReport,
        /// Human-readable summary of the measured calibration.
        summary: String,
    },
    /// The run failed (or was cancelled); `failure` is the typed
    /// rejection when the calibrator produced one, `message` is always
    /// user-facing.
    Failed {
        /// Typed rejection (the wizard's failure card), when known.
        failure: Option<CalFailure>,
        /// User-facing message.
        message: String,
    },
}

/// The running virtual-cable routing loop and the identifiers it serves.
struct RouterSlot {
    /// The owned routing loop (capture → DSP → cable output).
    router: VirtualCableRouter,
    /// Capture endpoint id the loop was started with.
    endpoint: String,
    /// Friendly name of the cable render sink.
    sink: String,
    /// Capture sample rate the loop runs at.
    rate_hz: u32,
}

/// Owns the session-local lease mapping the Auto-Tune runner writes to
/// plus the lease-honoring mirror thread (D-018 in-process topology: no
/// APO exists to host the lease, so the app hosts it itself).
///
/// The thread mirrors the runner's lease exactly like T16's APO-side
/// design: while the lease is hot it sets the mapping's `BYPASS` state
/// flag (the runner's `confirm_bypass` handshake) and asserts `bypass`
/// on the DSP parameter block (the cable output goes raw); when the
/// lease ends it restores the block's pre-lease bypass setting. The
/// block writes are outside the controller's model, so the persisted
/// user state can never record a calibration-induced bypass.
struct LeaseShim {
    mapping: HANDLE,
    base: *mut c_void,
    stop: Arc<AtomicBool>,
    join: Option<std::thread::JoinHandle<()>>,
}

// SAFETY: the view is only accessed through `ShmLayout`'s lock-free
// atomics, exactly as a real APO would across processes; the handle and
// the thread are owned by this guard, and every use is serialized by
// the process-wide `Services` mutex plus the `stop` flag on teardown.
unsafe impl Send for LeaseShim {}

/// Shared, process-wide audio service state. The shell's app loop owns
/// the conceptual lifecycle; the lazy static removes the need to plumb a
/// services object through every shell call site.
struct Services {
    visualizer: Option<Visualizer>,
    frames: Option<Arc<Triple<UiFrame>>>,
    last_frame: FrameSnapshot,
    last_pull: Option<Instant>,
    monitor: Option<Monitor>,
    router: Option<RouterSlot>,
    autotune: Option<AutotuneState>,
    autotune_cancel: Arc<AtomicBool>,
    lease: Option<LeaseShim>,
    messages: Vec<String>,
}

impl Services {
    fn new() -> Self {
        Self {
            visualizer: None,
            frames: None,
            last_frame: FrameSnapshot {
                spectrum: [0.0; 32],
                telemetry: AudioTelemetryFrame::default(),
            },
            last_pull: None,
            monitor: None,
            router: None,
            autotune: None,
            autotune_cancel: Arc::new(AtomicBool::new(false)),
            lease: None,
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

// ---------------------------------------------------------------------------
// D-018 routing: parameter block, telemetry/tap plumbing, router
// ---------------------------------------------------------------------------

/// The process-wide DSP parameter block: the controller publishes
/// resolved parameters into it and the routing loop reads them
/// lock-free. The shell hands [`ParamBlock`] to `EngineConfig::block`.
#[must_use]
pub fn param_block() -> Arc<ParamBlock> {
    static BLOCK: OnceLock<Arc<ParamBlock>> = OnceLock::new();
    Arc::clone(BLOCK.get_or_init(|| Arc::new(ParamBlock::new())))
}

/// Telemetry frames published by the routing loop (process-wide, so the
/// visualizer and the pipe stay connected across router restarts).
fn router_telemetry() -> Arc<Triple<AudioTelemetryFrame>> {
    static T: OnceLock<Arc<Triple<AudioTelemetryFrame>>> = OnceLock::new();
    Arc::clone(T.get_or_init(|| Arc::new(Triple::new(AudioTelemetryFrame::default()))))
}

/// Post-DSP tap samples published by the routing loop (process-wide).
fn router_tap() -> Arc<SpscRing<ROUTER_TAP_N>> {
    static T: OnceLock<Arc<SpscRing<ROUTER_TAP_N>>> = OnceLock::new();
    Arc::clone(T.get_or_init(|| Arc::new(SpscRing::new())))
}

/// Default rate of a capture endpoint (fallback 48 kHz, like the router).
fn input_rate(endpoint: &str) -> u32 {
    tine_engine::audio_io::list_inputs()
        .ok()
        .and_then(|list| list.iter().find(|d| d.id == endpoint).map(|d| d.default_rate))
        .unwrap_or(ROUTER_FALLBACK_RATE)
}

/// Starts (or retargets) the D-018 virtual-cable routing loop in this
/// process: capture `endpoint` → `DspPipeline` (shared parameter block)
/// → the virtual cable render sink. Idempotent: a loop already running
/// on the same endpoint is a no-op; an endpoint change stops the old
/// loop only after the new one started (no silence through a restart
/// failure). A missing or disabled cable, or a failed stream open,
/// is surfaced as a user message — the control plane keeps working.
///
/// Returns whether the loop is running for `endpoint` (the shell uses
/// that as the D-018 "linked" notion: there is no APO link event in
/// this topology).
#[must_use]
pub fn router_follow(endpoint: &str) -> bool {
    let wanted = endpoint.to_owned();
    let mut failures: Vec<String> = Vec::new();
    let rate = input_rate(&wanted);
    with(|s| {
        if s.router.is_some() && s.router.as_ref().is_some_and(|r| r.endpoint == wanted) {
            return;
        }
        let sink = match tine_engine::apo_admin::find_virtual_cable_sink() {
            Ok(Some(info)) => info,
            Ok(None) => {
                failures.push(
                    "No virtual audio cable found: install VB-Audio Virtual Cable, then use Repair (D-018).".to_owned(),
                );
                return;
            }
            Err(e) => {
                failures.push(format!("Virtual-cable discovery failed: {e}"));
                return;
            }
        };
        if !sink.is_available {
            failures.push(format!(
                "{} is disabled: enable it in the Sound control panel, then use Repair.",
                sink.name
            ));
            return;
        }
        match VirtualCableRouter::start::<ROUTER_TAP_N>(
            &DeviceRef::Id(wanted.clone()),
            &DeviceRef::Id(sink.id.clone()),
            param_block(),
            router_telemetry(),
            router_tap(),
        ) {
            Ok(router) => {
                let old = s.router.take();
                s.router = Some(RouterSlot {
                    router,
                    endpoint: wanted.clone(),
                    sink: sink.name.clone(),
                    rate_hz: rate,
                });
                if let Some(old) = old {
                    old.router.stop();
                    tracing::info!(target: "tine::services",
                        from = %old.endpoint, to = %wanted, "routing loop retargeted");
                } else {
                    tracing::info!(target: "tine::services",
                        endpoint = %wanted, sink = %sink.name, rate_hz = rate,
                        "virtual-cable routing loop started");
                }
            }
            Err(e) => {
                failures.push(format!("Virtual-cable routing failed to start: {e}"));
            }
        }
    });
    for msg in &failures {
        with(|s| s.surface(msg.clone()));
    }
    with(|s| s.router.as_ref().is_some_and(|r| r.endpoint == endpoint))
}

/// Stops the routing loop (shutdown path; releases the mic and the cable
/// sink). Idempotent.
pub fn router_stop() {
    with(|s| {
        if let Some(slot) = s.router.take() {
            tracing::info!(target: "tine::services", endpoint = %slot.endpoint, "routing loop stopped");
            slot.router.stop();
        }
    });
}

/// Latest live telemetry, as cached by the last [`spectrum`] pull (the
/// pull is the `UiFrame` triple's single reader; `meters` pulls spectrum
/// first, then this — never a second triple read by the same request).
///
/// `None` while the router is down or no pull has produced a frame yet;
/// the shell's `meters` then falls back to the controller handle.
#[must_use]
pub fn telemetry_live() -> Option<AudioTelemetryFrame> {
    with(|s| {
        s.router.as_ref()?;
        Some(s.last_frame.telemetry)
    })
}

/// JSON-able routing snapshot for `status`-side consumers.
#[must_use]
pub fn router_status() -> Value {
    with(|s| match &s.router {
        None => json!({ "on": false }),
        Some(slot) => {
            let stats = slot.router.stats();
            json!({
                "on": true,
                "endpoint": slot.endpoint,
                "sink": slot.sink,
                "rate_hz": slot.rate_hz,
                "underruns": stats.underruns,
                "overruns": stats.overruns,
                "callbacks": stats.callbacks,
                "max_callback_us": stats.max_callback_us,
            })
        }
    })
}

// ---------------------------------------------------------------------------
// Spectrum
// ---------------------------------------------------------------------------

/// The persisted endpoint the services operate on (D-018: the selected
/// capture device; falls back to a placeholder while nothing is persisted).
#[must_use]
pub fn service_endpoint(paths: &EnginePaths) -> EndpointId {
    let settings = persist::load_settings(paths).into_value();
    EndpointId::new(settings.selected_endpoint_id.unwrap_or_else(|| String::from("default")))
}

/// The shared-memory client of the persisted endpoint, when a live APO
/// mapping exists (production D-018 has none — the router is in-process;
/// session-local test mappings open fine).
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
/// UI child's ≤ 60 Hz `meters` poll), a pump thread drains the post-DSP
/// tap, runs the `SpectrumAnalyzer`, and publishes a `UiFrame` through a
/// lock-free triple buffer; the pull here reads the latest wins (an
/// in-flight frame is simply dropped). With the D-018 routing loop
/// online the tap is the in-process ring; otherwise it falls back to the
/// APO mapping (none in production, test mappings covered). When pulls
/// stop for [`VISUALIZER_TAIL`], a reaper stops the pump (thread + timer)
/// so the service costs nothing while unused.
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
            && let Some((v, frames)) = start_visualizer(paths, s.router.as_ref().map(|r| r.rate_hz))
        {
            s.visualizer = Some(v);
            s.frames = Some(frames);
        }
        if let Some(frames) = &s.frames {
            let mut f = UiFrame::default();
            if frames.latest(&mut f) {
                s.last_frame = FrameSnapshot {
                    spectrum: f.spectrum,
                    telemetry: f.telemetry,
                };
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

// ---------------------------------------------------------------------------
// Auto-Tune (T16 runner flow)
// ---------------------------------------------------------------------------

/// Starts the 8-second Auto-Tune calibration for the current endpoint.
///
/// Wired to the T16 runner: bypass lease → raw capture
/// (`CaptureSpec::Device`) → the real `AutoCalibrator` (`CalProgress` at
/// 30 Hz mirrored for [`autotune_state`]) → `CalibrationReport` on
/// success, held for [`autotune_take_pending`] / [`autotune_discard`].
/// A second concurrent run is rejected politely. The lease is absolute
/// wall-clock time: killing `tine.exe` mid-run self-heals within 15 s
/// (T16). Without a live audio path (no APO mapping and the routing loop
/// down) the request surfaces a user message instead of failing silently.
pub fn autotune_start() {
    let Ok(paths) = EnginePaths::system() else {
        with(|s| s.surface("Auto-Tune: storage paths unavailable".to_owned()));
        return;
    };
    autotune_start_on(&paths);
}

/// Starts the shared tap as a [`tine_engine::autotune::SampleSource`] for
/// the Auto-Tune run (D-018 in-process topology): the routing loop is
/// already the microphone's only capture client (this machine's USB mic
/// refuses a second shared-mode client), so the calibrator drains the
/// post-DSP tap instead — while the lease shim asserts DSP bypass the
/// tap carries exactly the raw (latency-aligned, ms-crossfaded) signal
/// the APO world would hand over. The calibrator is the tap's only
/// reader: the pump is torn down for the run (its next pull re-creates
/// it), so meters stall at the cached floor for the ~9 s — the same
/// trade a hardware APO makes while its lease is hot.
struct RouterTapSource {
    /// Post-DSP tap the routing loop publishes.
    tap: Arc<SpscRing<ROUTER_TAP_N>>,
    /// Capture rate the loop runs at.
    rate_hz: u32,
}

impl tine_engine::autotune::SampleSource for RouterTapSource {
    fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        self.tap.pop_slice(buf)
    }

    fn has_error(&self) -> bool {
        false
    }
}

/// [`autotune_start`] against explicit persistence roots (test seam).
pub fn autotune_start_on(paths: &EnginePaths) {
    if with(|s| matches!(s.autotune, Some(AutotuneState::Running { .. }))) {
        tracing::info!(target: "tine::services", "auto-tune already running");
        return;
    }
    if with(|s| matches!(s.autotune, Some(AutotuneState::ReportPending { .. }))) {
        with(|s| {
            s.surface("Auto-Tune: apply or discard the pending Auto-Tune results before starting a new run".to_owned());
        });
        return;
    }
    // A terminal `Failed` state from a previous run is a stale failure card,
    // not a lock (T22 QA: a NotEnoughSpeech rejection left every retry
    // silently rejected as "already running"). Drop it so the new run
    // starts from `idle`; `Running` and `ReportPending` above stay protected.
    if with(|s| {
        let stale = matches!(s.autotune, Some(AutotuneState::Failed { .. }));
        if stale {
            s.autotune = None;
        }
        stale
    }) {
        tracing::info!(target: "tine::services", "auto-tune stale failure cleared for retry");
    }
    let routed_rate = with(|s| s.router.as_ref().map(|r| r.rate_hz));
    let client = endpoint_client(paths).or_else(|| {
        if let Some(_rate) = routed_rate {
            match start_lease_shim() {
                Ok((client, shim)) => {
                    with(|s| s.lease = Some(shim));
                    Some(client)
                }
                Err(e) => {
                    with(|s| s.surface(format!("Auto-Tune: {e}")));
                    None
                }
            }
        } else {
            None
        }
    });
    let Some(client) = client else {
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
    // The capture is the shared tap while the router runs (single device
    // client), else a direct device open (APO-style topology).
    let capture = match routed_rate {
        Some(rate) => {
            with(|s| {
                if let Some(mut v) = s.visualizer.take() {
                    v.stop();
                }
                s.frames = None;
            });
            tracing::debug!(target: "tine::services", rate_hz = rate, "auto-tune captures from the routing tap");
            CaptureSpec::Source(Box::new(RouterTapSource {
                tap: router_tap(),
                rate_hz: rate,
            }))
        }
        None => CaptureSpec::Device {
            device: DeviceRef::Id(endpoint.as_str().to_owned()),
            want_rate: None,
        },
    };
    let cfg = AutoTuneConfig {
        client,
        controller: None, // the pipe's calibrate_apply owns application
        capture,
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
            voiced_ms: 0,
        });
    });
    let mut handle = AutoTuneRunner::start(cfg);
    // Feed thread: mirrors progress at the runner's cadence until exactly
    // one terminal event, then settles the mirror and tears the lease
    // shim down (its mirror thread restores the DSP's bypass setting).
    // It also honors an orderly cancel requested through
    // [`autotune_cancel`].
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
                            voiced_ms: p.voiced_ms,
                        });
                    }),
                    AutoTuneEvent::Done(outcome) => {
                        with(|s| {
                            s.lease = None;
                            s.autotune = match outcome {
                                AutoTuneOutcome::Applied(report) => Some(AutotuneState::ReportPending {
                                    summary: format!(
                                        "measured noise {:+.1} dBFS, speech {:+.1} dBFS, verified peak {:+.1} dBFS",
                                        report.calibration.noise_rms_db,
                                        report.calibration.speech_rms_db,
                                        report.simulated_peak_db
                                    ),
                                    report: *report,
                                }),
                                AutoTuneOutcome::Failed(failure) => Some(AutotuneState::Failed {
                                    failure: Some(failure),
                                    message: format!("Auto-Tune rejected the capture: {failure:?}"),
                                }),
                                AutoTuneOutcome::Error(e) => Some(AutotuneState::Failed {
                                    failure: None,
                                    message: format!("Auto-Tune failed: {e}"),
                                }),
                            };
                        });
                        break;
                    }
                }
            }
        })
        .is_err()
    {
        with(|s| {
            s.lease = None;
            s.autotune = None;
            s.surface("Auto-Tune: progress thread spawn failed".to_owned());
        });
    }
}

/// Requests an orderly stop of the Auto-Tune runner: the feed thread sees
/// the flag, cancels the run, and settles on the runner's terminal event
/// (which also clears the bypass lease, T16).
pub fn autotune_cancel() {
    with(|s| s.autotune_cancel.store(true, Ordering::Release));
}

/// Takes the pending Auto-Tune calibration for application (the pipe's
/// `calibrate_apply` hands it to `Command::ApplyCalibration`); `None`
/// when no run has produced results (or they were already handled).
#[must_use]
pub fn autotune_take_pending() -> Option<Calibration> {
    with(|s| match s.autotune.take() {
        Some(AutotuneState::ReportPending { report, .. }) => Some(report.calibration),
        other => {
            s.autotune = other;
            None
        }
    })
}

/// Discards the pending Auto-Tune results (the pipe's
/// `calibrate_discard`); a no-op when there are none.
pub fn autotune_discard() {
    let cleared = with(|s| s.autotune.take().is_some());
    if cleared {
        tracing::info!(target: "tine::services", "auto-tune results discarded");
    }
}

/// JSON-able snapshot of the Auto-Tune flow for `status`-side consumers
/// (the UI child's wizard poll): the running phase carries
/// `phase`/`progress`/`level_db`/`voiced_ms`; `report-pending` carries
/// the results card's numbers and per-parameter changes (SPECS keys);
/// `failed` carries the typed rejection for the failure card.
#[must_use]
pub fn autotune_state() -> Value {
    with(|s| match &s.autotune {
        None => json!({ "flow": "idle" }),
        Some(AutotuneState::Running {
            phase,
            progress,
            level_db,
            voiced_ms,
        }) => {
            json!({
                "flow": "running",
                "phase": format!("{phase:?}"),
                "progress": progress,
                "level_db": level_db,
                "voiced_ms": voiced_ms,
            })
        }
        Some(AutotuneState::ReportPending { report, summary }) => {
            let cal = &report.calibration;
            json!({
                "flow": "report-pending",
                "summary": summary,
                "report": {
                    "noise_rms_db": cal.noise_rms_db,
                    "speech_rms_db": cal.speech_rms_db,
                    "simulated_peak_db": report.simulated_peak_db,
                    "simulated_rms_db": report.simulated_rms_db,
                    "changes": report.changes.iter().map(|&(id, before, after)| {
                        json!({
                            "key": crate::ui_bridge::param_key(id).unwrap_or_default(),
                            "before": before,
                            "after": after,
                        })
                    }).collect::<Vec<_>>(),
                },
            })
        }
        Some(AutotuneState::Failed { failure, message }) => {
            let f = match failure {
                Some(CalFailure::TooQuiet) => json!({ "type": "TooQuiet" }),
                Some(CalFailure::TooNoisy { snr_db }) => json!({ "type": "TooNoisy", "snr_db": snr_db }),
                Some(CalFailure::Clipping { pct }) => json!({ "type": "Clipping", "pct": pct }),
                Some(CalFailure::NotEnoughSpeech { voiced_ms }) => {
                    json!({ "type": "NotEnoughSpeech", "voiced_ms": voiced_ms })
                }
                Some(CalFailure::GainOutOfRange { needed_db }) => {
                    json!({ "type": "GainOutOfRange", "needed_db": needed_db })
                }
                None => Value::Null,
            };
            json!({ "flow": "failed", "failure": f, "message": message })
        }
    })
}

// ---------------------------------------------------------------------------
// Lease shim (D-018 production Auto-Tune)
// ---------------------------------------------------------------------------

/// Creates the session-local lease mapping and its lease-honoring mirror
/// thread, and returns the client the runner talks to.
///
/// The mapping name is session-local (`Local\`) and process-unique, so
/// it can never collide with another instance or a real APO mapping;
/// it is destroyed when the shim guard drops (right after the run's
/// terminal event). Errors are surfaced by the caller (they mean
/// Auto-Tune cannot run in this session).
fn start_lease_shim() -> Result<(Arc<ShmClient>, LeaseShim), String> {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = format!("Local\\Tine.v1.autotune-shim-{}-{n}", std::process::id());
    let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    let len = std::mem::size_of::<ShmLayout>();
    // SAFETY: `wide` is NUL-terminated; the page file backs the mapping;
    // the size matches `ShmLayout` exactly.
    let mapping = match unsafe {
        CreateFileMappingW(
            INVALID_HANDLE_VALUE,
            None,
            PAGE_READWRITE,
            0,
            len as u32,
            windows::core::PCWSTR(wide.as_ptr()),
        )
    } {
        Ok(h) => h,
        Err(e) => return Err(format!("lease shim: CreateFileMappingW failed: {e}")),
    };
    // SAFETY: handle from CreateFileMappingW with a matching size.
    let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) };
    if view.Value.is_null() {
        // SAFETY: mapping handle created above, closed on this branch.
        unsafe { CloseHandle(mapping) }.ok();
        return Err("lease shim: MapViewOfFile returned null".to_owned());
    }
    // SAFETY: fresh RW view of exactly `len` bytes.
    let init = unsafe {
        init_layout_in_place(
            view.Value.cast::<u8>(),
            len,
            *b"tine-app\0\0\0\0\0\0\0\0",
            ROUTER_FALLBACK_RATE,
            1,
            &DspParams::DEFAULT,
        )
    };
    if init.is_err() {
        // SAFETY: the view and mapping from above, unmapped/closed once.
        unsafe {
            let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view.Value });
            CloseHandle(mapping).ok();
        }
        return Err("lease shim: layout init failed".to_owned());
    }
    let client = match ShmClient::open_name(&name) {
        Ok(c) => Arc::new(c),
        Err(e) => {
            // SAFETY: the view and mapping from above, unmapped/closed once.
            unsafe {
                let _ = UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: view.Value });
                CloseHandle(mapping).ok();
            }
            return Err(format!("lease shim: open failed: {e}"));
        }
    };
    // SAFETY: `view.Value` is a valid mapping kept alive by the shim
    // guard for the thread's whole life (see `LeaseShim`); only this
    // thread writes the state flags.
    let header = unsafe { &(*(view.Value as *const ShmLayout)).header };
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = Arc::clone(&stop);
    let block = param_block();
    let join = std::thread::Builder::new()
        .name("tine-lease-shim".to_owned())
        .spawn(move || {
            // The bypass setting to restore when the lease ends (captured
            // at the first hot observation, i.e. the user's pre-run A/B).
            let mut restore_bypass: Option<bool> = None;
            let mut last_gen = 0_u64;
            let mut params = DspParams::DEFAULT;
            loop {
                let stop_now = stop_thread.load(Ordering::Acquire);
                let now = now_ms();
                let hot = header.bypass_lease_until_ms.load(Ordering::Acquire) > now;
                if hot {
                    if block.load_if_changed(&mut last_gen, &mut params) || last_gen == 0 {
                        restore_bypass.get_or_insert(params.bypass);
                        if !params.bypass {
                            params.bypass = true;
                            block.store(&params);
                        }
                    }
                } else if let Some(was) = restore_bypass.take() {
                    // The local `params` is the block's current content: while
                    // the lease was hot this thread was the last writer, so a
                    // re-assert (controller publish mid-run) is already in it.
                    params.bypass = was;
                    block.store(&params);
                }
                // Flag mirror: the runner's `confirm_bypass` handshake.
                let state = header.state.load(Ordering::Relaxed);
                let wanted = if hot { flags::BYPASS } else { 0 };
                if state & flags::BYPASS != wanted {
                    header.state.store((state & !flags::BYPASS) | wanted, Ordering::Release);
                }
                if stop_now {
                    break;
                }
                std::thread::sleep(Duration::from_millis(25));
            }
        })
        .map_err(|e| format!("lease shim: thread spawn failed: {e}"))?;
    let shim = LeaseShim {
        mapping,
        base: view.Value,
        stop,
        join: Some(join),
    };
    tracing::info!(target: "tine::services", name = %name, "auto-tune lease shim started");
    Ok((client, shim))
}

impl Drop for LeaseShim {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        // SAFETY: view from MapViewOfFile, unmapped exactly once.
        let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base }) };
        // SAFETY: mapping handle created by CreateFileMappingW, closed once.
        unsafe { CloseHandle(self.mapping) }.ok();
    }
}

/// User-facing service messages since the last call (error surfacing).
#[must_use]
pub fn take_messages() -> Vec<String> {
    with(|s| std::mem::take(&mut s.messages))
}

/// Queues one user-facing message for the next [`take_messages`] pull
/// (`status` notes and the window's message toast both show it).
///
/// Control-plane callers state non-fatal failures through this so nothing
/// fails silently (the engine error path in `shell.rs` loses a user's
/// changes otherwise, e.g. a full disk rejects the settings save).
pub fn surface_message(msg: impl Into<String>) {
    with(|s| s.surface(msg.into()));
}

// ---------------------------------------------------------------------------
// "Hear Myself" monitor
// ---------------------------------------------------------------------------

/// Applies the "Hear Myself" monitor state (capture the selected mic →
/// play to the chosen device; D-018 note: the processed-capture variant
/// needs render-loopback, which lives outside `tine-engine`'s audio IO
/// (T12) — the monitor therefore replays the *input* signal).
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
/// Auto-Tune mirror and lease shim, the routing loop, and the
/// visualizer pump. Idempotent.
pub fn shutdown() {
    with(|s| {
        if let Some(m) = s.monitor.take() {
            m.stop();
        }
        s.autotune = None;
        s.autotune_cancel.store(true, Ordering::Release);
        s.lease = None;
        if let Some(slot) = s.router.take() {
            slot.router.stop();
        }
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

/// Starts the pump on the live tap: the in-process routing loop's ring
/// when the router runs (D-018 production), else an APO-side mapping
/// when one exists (none in production; test mappings). The caller
/// passes the router's rate from inside the services lock (this
/// function must not re-enter it).
fn start_visualizer(paths: &EnginePaths, router_rate: Option<u32>) -> Option<(Visualizer, Arc<Triple<UiFrame>>)> {
    if let Some(rate_hz) = router_rate {
        let signals = InProcessSignals {
            telemetry: router_telemetry(),
            tap: router_tap(),
            rate_hz,
        };
        let v = Visualizer::start(signals);
        let frames = v.frames();
        tracing::info!(target: "tine::services", rate_hz, "spectrum pump started (virtual-cable tap)");
        return Some((v, frames));
    }
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
    fn autotune_retry_after_a_failed_run_is_not_wedged() {
        // Regression (T22 QA): a terminal `Failed` state (e.g. the silent-room
        // `NotEnoughSpeech` rejection) used to block every later start as an
        // unnoticed "already running" reject. A retry must proceed: here the
        // proceed-path surfaces the same live-link message a cold start does
        // (no router is up in this test) and leaves the flow back at `idle`.
        let paths = temp_paths();
        super::shutdown();
        with(|s| {
            s.autotune = Some(AutotuneState::Failed {
                failure: Some(CalFailure::NotEnoughSpeech { voiced_ms: 0 }),
                message: "Auto-Tune rejected the capture".to_owned(),
            });
        });
        autotune_start_on(&paths);
        assert!(
            take_messages()
                .iter()
                .any(|m| m.contains("Auto-Tune needs a live processing link")),
            "retry must have proceeded past the stale failure card"
        );
        assert_eq!(autotune_state()["flow"], "idle");
    }

    #[test]
    fn autotune_retry_keeps_running_and_pending_states_protected() {
        let paths = temp_paths();
        super::shutdown();
        // In-flight run: politely rejected, untouched.
        with(|s| {
            s.autotune = Some(AutotuneState::Running {
                phase: CalPhase::Countdown,
                progress: 0.0,
                level_db: -90.0,
                voiced_ms: 0,
            });
        });
        autotune_start_on(&paths);
        assert_eq!(autotune_state()["flow"], "running");
        assert!(
            !take_messages().iter().any(|m| m.contains("live processing link")),
            "a concurrent run must not proceed to a second capture"
        );

        // Applied-pending results: rejected with an actionable message.
        let report = tine_engine::autotune::CalibrationReport {
            calibration: tine_protocol::presets::Calibration {
                noise_rms_db: -80.0,
                noise_peak_db: -70.0,
                speech_rms_db: -18.0,
                crest_db: 12.0,
                tilt_db: 3.0,
                derived: tine_protocol::presets::CalibrationOverrides {
                    input_gain_db: 9.5,
                    gate_threshold_db: -55.0,
                    denoise_mix: 0.85,
                    comp_thresh_db: -16.0,
                    comp_makeup_db: 2.5,
                },
            },
            changes: [
                (tine_protocol::params::ParamId::InputGainDb, 0.0, 9.5),
                (tine_protocol::params::ParamId::GateThresholdDb, -40.0, -55.0),
                (tine_protocol::params::ParamId::DenoiseMix, 0.5, 0.85),
                (tine_protocol::params::ParamId::CompThreshDb, -20.0, -16.0),
                (tine_protocol::params::ParamId::CompMakeupDb, 0.0, 2.5),
            ],
            simulated_peak_db: -5.2,
            simulated_rms_db: -18.4,
        };
        with(|s| {
            s.autotune = Some(AutotuneState::ReportPending {
                report,
                summary: "measured".to_owned(),
            });
        });
        autotune_start_on(&paths);
        assert_eq!(autotune_state()["flow"], "report-pending");
        assert!(
            take_messages()
                .iter()
                .any(|m| m.contains("apply or discard the pending Auto-Tune results")),
            "pending results must be protected with an actionable message"
        );
    }

    #[test]
    fn surface_message_round_trips_through_take_messages() {
        // Regression (T22 QA, FI-5): a persist failure surfaced by the shell
        // must land in the message queue the next `take_messages` drain
        // consumes (the `status` notes and the window toast), never be
        // dropped on the floor.
        super::shutdown();
        surface_message("Tine couldn't apply or save a change: disk full (simulated)");
        let drained = take_messages();
        assert_eq!(drained.len(), 1);
        assert!(drained[0].contains("couldn't apply or save"));
        let _ = take_messages();
    }

    #[test]
    fn autotune_state_and_pending_accessors_are_idle_side_safe() {
        super::shutdown();
        assert_eq!(autotune_state()["flow"], "idle");
        assert!(autotune_take_pending().is_none());
        autotune_discard();
        assert_eq!(autotune_state()["flow"], "idle");
        assert_eq!(router_status()["on"], false);
    }

    #[test]
    fn lease_shim_confirms_bypass_and_restores_on_clear() {
        let (client, _shim) = start_lease_shim().expect("shim standalone");
        tine_engine::autotune::renew_lease(&client, now_ms());
        let block = param_block();
        let mut confirmed_bypass = false;
        for _ in 0..200 {
            if client.state_flags() & flags::BYPASS != 0 {
                confirmed_bypass = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(confirmed_bypass, "the shim must confirm the lease (T16 handshake)");
        let mut last_gen = 0_u64;
        let mut params = DspParams::DEFAULT;
        for _ in 0..200 {
            if block.load_if_changed(&mut last_gen, &mut params) && params.bypass {
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(params.bypass, "the shim asserts DSP bypass while the lease is hot");
        tine_engine::autotune::clear_lease(&client);
        let mut restored = false;
        for _ in 0..200 {
            let mut p = DspParams::DEFAULT;
            let mut g = 0_u64;
            block.load_if_changed(&mut g, &mut p);
            if !p.bypass {
                restored = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        assert!(restored, "the shim restores processing when the lease ends");
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
