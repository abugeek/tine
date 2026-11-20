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
use tine_dsp::calibrator::{
    CHANGED_PARAMS, COUNTDOWN_S, LIVE_COUNTDOWN_S, LIVE_NOISE_S, LIVE_VOICE_S, NOISE_S, VOICE_S, VoiceStyle,
    recommend_macros,
};
use tine_dsp::macros::{Macro as DspMacro, MacroMapper};
use tine_engine::audio_io::DeviceRef;
use tine_engine::autotune::{
    AutoTuneConfig, AutoTuneEvent, AutoTuneOutcome, AutoTuneRunner, CalFailure, CalPhase, CaptureSpec,
    LiveAutoCalibratorFactory,
};
use tine_engine::listeners::{
    GateAction, PARKED_POLL, ParkPolicy, RUNNING_POLL, foreign_listener, is_sink_capture_side, raw_mic_app_names,
};
use tine_engine::mic_check::{CalibrationPreviewPlayer, MicCheckMode, MicCheckRunner, MicCheckState};
use tine_engine::model::{AppModel, EndpointId, KeepPins, resolve};
use tine_engine::monitor::Monitor;
use tine_engine::paths::{EnginePaths, PathsError};
use tine_engine::persist;
use tine_engine::shm_client::{LinkState, ShmClient};
use tine_engine::spectrum::SpectrumAnalyzer;
use tine_engine::virtual_cable::VirtualCableRouter;
use tine_engine::visualizer::{EngineSignals, InProcessSignals, UiFrame, Visualizer};
use tine_engine::win::{CableSessionEventHook, CaptureSessionWatcher, SessionWake, process_name_for_pid};
use tine_protocol::params::{DenoiseEngine, DspParams};
use tine_protocol::presets::Calibration;
use tine_protocol::shm::{ShmLayout, init_layout_in_place};
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};
use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError, HANDLE, INVALID_HANDLE_VALUE};
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
        /// `(countdown_s, noise_s, voice_s)` durations for this run.
        phase_durations: (f32, f32, f32),
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
        /// Voice-phase mono capture (`rate_hz` Hz) for instant Raw vs. Tuned preview looping.
        captured_voice: Arc<Vec<f32>>,
        /// Sample rate of `captured_voice` in Hz.
        rate_hz: u32,
        /// Currently previewed post-calibration voice style.
        preview_style: VoiceStyle,
        /// Whether pinned Pro parameters are kept in the preview.
        preview_keep_pins: bool,
        /// Active voice-loop audition mode (`None` = stopped, `Some(false)` = tuned, `Some(true)` = raw).
        audition_mode: Option<bool>,
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

/// Where a parked routing loop resumes (T24 listener gate).
#[derive(Clone)]
struct ParkedRoute {
    endpoint: String,
    sink: String,
}

/// Desired "Hear Myself" state (remembered when audio routing is not yet active).
#[derive(Clone, Default)]
enum MonitorWanted {
    #[default]
    Off,
    On(Option<String>),
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
    monitor_wanted: MonitorWanted,
    denoise_engine: DenoiseEngine,
    router: Option<RouterSlot>,
    /// Routing loop stopped by the listener gate (T24): no app records the
    /// virtual mic and nothing in Tine needs the processed signal.
    parked: Option<ParkedRoute>,
    /// When `false` (Keep Warm / PTT mode), the routing loop stays hot permanently.
    park_when_unused: bool,
    raw_mic_apps: Vec<String>,
    raw_mic_checked_at: Option<Instant>,
    mic_check: Option<MicCheckRunner>,
    mic_check_paused_monitor: bool,
    cal_preview_player: Option<CalibrationPreviewPlayer>,
    cal_preview_paused_monitor: bool,
    autotune: Option<AutotuneState>,
    autotune_cancel: Arc<AtomicBool>,
    lease: Option<LeaseShim>,
    messages: Vec<String>,
}

fn silent_telemetry() -> AudioTelemetryFrame {
    AudioTelemetryFrame {
        input_rms_db: -90.0,
        input_peak_db: -90.0,
        output_rms_db: -90.0,
        output_peak_db: -90.0,
        ..AudioTelemetryFrame::default()
    }
}

impl Services {
    fn new() -> Self {
        Self {
            visualizer: None,
            frames: None,
            last_frame: FrameSnapshot {
                spectrum: [0.0; 32],
                telemetry: silent_telemetry(),
            },
            last_pull: None,
            monitor: None,
            monitor_wanted: MonitorWanted::Off,
            denoise_engine: DenoiseEngine::default(),
            router: None,
            parked: None,
            park_when_unused: true,
            raw_mic_apps: Vec::new(),
            raw_mic_checked_at: None,
            mic_check: None,
            mic_check_paused_monitor: false,
            cal_preview_player: None,
            cal_preview_paused_monitor: false,
            autotune: None,
            autotune_cancel: Arc::new(AtomicBool::new(false)),
            lease: None,
            messages: Vec::new(),
        }
    }

    /// Logs and queues a user-facing message (never a silent failure).
    fn surface(&mut self, msg: String) {
        if self.messages.iter().any(|m| m == &msg) {
            return;
        }
        tracing::warn!(target: "tine::services", "{msg}");
        if self.messages.len() >= 16 {
            self.messages.remove(0);
        }
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

/// Guards the shared post-DSP `SpscRing` against dual readers: while
/// Auto-Tune is active (`RouterTapSource` drains `router_tap()`), the
/// visualizer must never start or read the tap.
static CALIBRATING: AtomicBool = AtomicBool::new(false);

/// RAII guard that clears [`CALIBRATING`] when the Auto-Tune feed thread
/// exits (or if spawning it fails).
struct CalibratingGuard;

impl Drop for CalibratingGuard {
    fn drop(&mut self) {
        CALIBRATING.store(false, Ordering::Release);
    }
}

/// Serializes all writes to [`param_block`] so the controller and the
/// Auto-Tune lease shim never call `ParamBlock::store` concurrently.
struct ParamWriteState {
    last_controller_params: DspParams,
    lease_bypass_active: bool,
    preview_params: Option<DspParams>,
}

fn effective_params(st: &ParamWriteState) -> DspParams {
    if st.lease_bypass_active {
        let mut e = st.last_controller_params;
        e.bypass = true;
        e.input_gain_db = 0.0;
        e
    } else if let Some(preview) = st.preview_params {
        preview
    } else {
        st.last_controller_params
    }
}

fn with_param_write<R>(f: impl FnOnce(&mut ParamWriteState) -> R) -> R {
    static STATE: OnceLock<Mutex<ParamWriteState>> = OnceLock::new();
    let lock = STATE.get_or_init(|| {
        Mutex::new(ParamWriteState {
            last_controller_params: DspParams::DEFAULT,
            lease_bypass_active: false,
            preview_params: None,
        })
    });
    match lock.lock() {
        Ok(mut guard) => f(&mut guard),
        Err(poisoned) => f(&mut poisoned.into_inner()),
    }
}

/// Publishes resolved controller parameters into [`param_block`],
/// serialized against the Auto-Tune lease shim so `ParamBlock::store`
/// never has concurrent writers.
pub fn publish_params(params: &DspParams) {
    with_param_write(|st| {
        st.last_controller_params = *params;
        param_block().store(&effective_params(st));
    });
}

/// Latest resolved DSP parameters published by the controller.
#[must_use]
pub fn last_controller_params() -> DspParams {
    with_param_write(|st| st.last_controller_params)
}

fn set_lease_bypass(active: bool) {
    with_param_write(|st| {
        if st.lease_bypass_active != active {
            st.lease_bypass_active = active;
            param_block().store(&effective_params(st));
        }
    });
}

fn set_calibration_preview(preview: Option<DspParams>) {
    with_param_write(|st| {
        st.preview_params = preview;
        param_block().store(&effective_params(st));
    });
}

/// The process-wide DSP parameter block: the controller publishes
/// resolved parameters into it (via [`publish_params`]) and the routing
/// loop reads them lock-free.
#[must_use]
pub fn param_block() -> Arc<ParamBlock> {
    static BLOCK: OnceLock<Arc<ParamBlock>> = OnceLock::new();
    Arc::clone(BLOCK.get_or_init(|| Arc::new(ParamBlock::new())))
}

/// Telemetry frames published by the routing loop (process-wide, so the
/// visualizer and the pipe stay connected across router restarts).
fn router_telemetry() -> Arc<Triple<AudioTelemetryFrame>> {
    static T: OnceLock<Arc<Triple<AudioTelemetryFrame>>> = OnceLock::new();
    Arc::clone(T.get_or_init(|| Arc::new(Triple::new(silent_telemetry()))))
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
        .and_then(|list| {
            list.iter()
                .find(|d| d.id == endpoint || d.id.ends_with(endpoint))
                .map(|d| d.default_rate)
        })
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
    // T24: a parked loop counts as healthy; it resumes on the (possibly new)
    // endpoint when a listener or an internal consumer appears.
    if with(|s| match s.parked.as_mut() {
        Some(p) => {
            p.endpoint.clone_from(&wanted);
            true
        }
        None => false,
    }) {
        return true;
    }
    // E5: check idempotency (same endpoint AND healthy router) before
    // calling `input_rate()` / COM device enumeration.
    if with(|s| {
        s.router
            .as_ref()
            .is_some_and(|r| r.endpoint == wanted && !r.router.has_error())
    }) {
        return true;
    }
    let mut failures: Vec<String> = Vec::new();
    let rate = input_rate(&wanted);
    with(|s| {
        if let Some(r) = s.router.as_ref() {
            if r.endpoint == wanted && !r.router.has_error() {
                return;
            }
            // E1: tear down unhealthy routers before rebuilding so stale
            // streams are released cleanly.
            if r.router.has_error()
                && let Some(old) = s.router.take()
            {
                tracing::warn!(target: "tine::services", endpoint = %old.endpoint, "tearing down unhealthy routing loop");
                old.router.stop();
            }
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
        // Stop the old router before starting the new one so `router_tap()`
        // never has two concurrent writers during retargeting.
        let old_endpoint = s.router.take().map(|old| {
            tracing::info!(target: "tine::services", endpoint = %old.endpoint, "stopping old routing loop before retarget");
            old.router.stop();
            old.endpoint
        });
        match VirtualCableRouter::start_with_engine_and_mute::<ROUTER_TAP_N>(
            &DeviceRef::Id(wanted.clone()),
            &DeviceRef::Id(sink.id.clone()),
            param_block(),
            router_telemetry(),
            router_tap(),
            s.denoise_engine,
            muted_flag(),
        ) {
            Ok(router) => {
                let new_feed = router.monitor_feed();
                let actual_rate = router.input_rate();
                s.router = Some(RouterSlot {
                    router,
                    endpoint: wanted.clone(),
                    sink: sink.name.clone(),
                    rate_hz: if actual_rate > 0 { actual_rate } else { rate },
                });
                if let Some(prev) = old_endpoint {
                    tracing::info!(target: "tine::services",
                        from = %prev, to = %wanted, "routing loop retargeted");
                } else {
                    tracing::info!(target: "tine::services",
                        endpoint = %wanted, sink = %sink.name, rate_hz = rate,
                        "virtual-cable routing loop started");
                }
                if let MonitorWanted::On(wanted_dev) = s.monitor_wanted.clone() {
                    let out = wanted_dev.as_deref().map_or_else(DeviceRef::default, DeviceRef::parse);
                    match &mut s.monitor {
                        Some(m) => {
                            if let Err(e) = m.restart(&new_feed, &out) {
                                failures.push(format!("Hear Myself restart failed: {e}"));
                                s.monitor = None;
                            }
                        }
                        None => match Monitor::start(&new_feed, &out) {
                            Ok(m) => {
                                tracing::info!(target: "tine::services",
                                    endpoint = %wanted,
                                    latency_ms = m.latency_ms(),
                                    "Hear Myself started");
                                s.monitor = Some(m);
                            }
                            Err(e) => {
                                failures.push(format!("Hear Myself failed to start: {e}"));
                            }
                        },
                    }
                } else if let Some(m) = s.monitor.take() {
                    m.stop();
                }
            }
            Err(e) => {
                if let Some(m) = s.monitor.take() {
                    m.stop();
                }
                failures.push(format!("Virtual-cable routing failed to start: {e}"));
            }
        }
    });
    for msg in &failures {
        with(|s| s.surface(msg.clone()));
    }
    let routed = with(|s| s.router.as_ref().is_some_and(|r| r.endpoint == endpoint));
    if routed {
        ensure_listener_gate();
    }
    routed
}

static MUTED: OnceLock<Arc<AtomicBool>> = OnceLock::new();

fn muted_flag() -> Arc<AtomicBool> {
    Arc::clone(MUTED.get_or_init(|| Arc::new(AtomicBool::new(false))))
}

/// Returns whether click-free RT microphone muting is active.
#[must_use]
pub fn is_muted() -> bool {
    muted_flag().load(Ordering::Acquire)
}

/// Sets click-free RT microphone mute (`true` = muted) and returns the active state.
#[must_use]
pub fn set_muted(muted: bool) -> bool {
    muted_flag().store(muted, Ordering::Release);
    muted
}

/// Toggles click-free RT microphone mute and returns the new state.
#[must_use]
pub fn toggle_muted() -> bool {
    let flag = muted_flag();
    !flag.fetch_xor(true, Ordering::AcqRel)
}

/// Forces a clean tear-down and rebuild of the virtual-cable routing loop for `endpoint_id`
/// (used on USB replug or Windows resume from sleep).
#[must_use]
pub fn router_force_rebuild(endpoint_id: &str) -> bool {
    with(|s| {
        s.parked = None;
        if let Some(old) = s.router.take() {
            tracing::info!(target: "tine::services", endpoint = %old.endpoint, "forcing routing loop rebuild");
            old.router.stop();
        }
    });
    router_follow(endpoint_id)
}

/// Returns `true` when an active routing loop has latched a stream
/// error (e.g., mic unplug, sleep/wake, or format change).
#[must_use]
pub fn router_has_error() -> bool {
    with(|s| s.router.as_ref().is_some_and(|r| r.router.has_error()))
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

/// Active Stage 2 denoiser engine (`Rnnoise`, `DeepFilterNet3`, or `Off`).
#[must_use]
pub fn denoise_engine() -> DenoiseEngine {
    with(|s| s.denoise_engine)
}

/// Switches the Stage 2 denoiser engine, rebuilding the routing loop immediately
/// if currently running. Returns whether routing is healthy afterwards.
#[must_use]
pub fn set_denoise_engine(engine: DenoiseEngine) -> bool {
    let rebuild_endpoint = with(|s| {
        if s.denoise_engine == engine {
            return None;
        }
        s.denoise_engine = engine;
        s.router.take().map(|old| {
            let ep = old.endpoint.clone();
            tracing::info!(target: "tine::services", engine = engine.as_str(), endpoint = %ep, "rebuilding routing loop for denoiser change");
            old.router.stop();
            ep
        })
    });
    match rebuild_endpoint {
        Some(ep) => router_follow(&ep),
        None => with(|s| s.router.is_some() || s.parked.is_some()),
    }
}

/// Lists external active apps currently recording from the selected physical
/// microphone directly (instead of Tine's `CABLE Output`), cached for 2 s.
#[must_use]
pub fn raw_mic_apps() -> Vec<String> {
    const CACHE_TTL: Duration = Duration::from_secs(2);
    enum Probe {
        Cached(Vec<String>),
        Query(String),
    }
    let probe = with(|s| {
        let Some(slot) = s.router.as_ref() else {
            s.raw_mic_apps.clear();
            s.raw_mic_checked_at = None;
            return Probe::Cached(Vec::new());
        };
        if s.raw_mic_checked_at.is_some_and(|t| t.elapsed() < CACHE_TTL) {
            return Probe::Cached(s.raw_mic_apps.clone());
        }
        Probe::Query(slot.endpoint.clone())
    });
    let endpoint = match probe {
        Probe::Cached(list) => return list,
        Probe::Query(ep) => ep,
    };
    let own_pid = std::process::id();
    let apps = CaptureSessionWatcher::new()
        .ok()
        .and_then(|mut w| w.sessions_by_id(&endpoint).ok().flatten())
        .map(|sessions| raw_mic_app_names(&sessions, own_pid, process_name_for_pid))
        .unwrap_or_default();
    with(|s| {
        s.raw_mic_apps.clone_from(&apps);
        s.raw_mic_checked_at = Some(Instant::now());
    });
    apps
}

/// JSON-able routing snapshot for `status`-side consumers.
#[must_use]
pub fn router_status() -> Value {
    with(|s| match &s.router {
        None => match &s.parked {
            Some(p) => json!({
                "on": false,
                "parked": true,
                "endpoint": p.endpoint,
                "sink": p.sink,
                "denoise_engine": s.denoise_engine.as_str(),
            }),
            None => json!({
                "on": false,
                "denoise_engine": s.denoise_engine.as_str(),
            }),
        },
        Some(slot) => {
            let stats = slot.router.stats();
            json!({
                "on": true,
                "endpoint": slot.endpoint,
                "sink": slot.sink,
                "denoise_engine": s.denoise_engine.as_str(),
                "rate_hz": slot.rate_hz,
                "underruns": stats.underruns,
                "overruns": stats.overruns,
                "xruns": stats.xruns,
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
    router_wake("meters");
    if CALIBRATING.load(Ordering::Acquire) {
        return with(|s| {
            s.last_pull = Some(Instant::now());
            s.last_frame.clone()
        });
    }
    with(|s| {
        s.last_pull = Some(Instant::now());
        if s.visualizer.is_none()
            && !CALIBRATING.load(Ordering::Acquire)
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
    /// Live telemetry triple published by the routing loop.
    telemetry: Arc<Triple<AudioTelemetryFrame>>,
    /// Spectrum analyzer keeping background UI bars live while Auto-Tune owns the tap.
    spectrum: SpectrumAnalyzer,
    /// Latest telemetry frame cached between pulls.
    last_tel: AudioTelemetryFrame,
    /// Capture rate the loop runs at.
    rate_hz: u32,
    /// `true` once stale pre-bypass samples in `tap` have been flushed.
    flushed: bool,
}

impl RouterTapSource {
    fn new(tap: Arc<SpscRing<ROUTER_TAP_N>>, telemetry: Arc<Triple<AudioTelemetryFrame>>, rate_hz: u32) -> Self {
        Self {
            tap,
            telemetry,
            spectrum: SpectrumAnalyzer::new(rate_hz as f32),
            last_tel: silent_telemetry(),
            rate_hz,
            flushed: false,
        }
    }
}

impl tine_engine::autotune::SampleSource for RouterTapSource {
    fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        if !self.flushed {
            // Allow DspPipeline's 10 ms bypass & dry-gain smoothers to settle to unity,
            // then flush any pre-bypass audio buffered in the tap ring so it cannot
            // eat into the Countdown or pollute room-noise measurements.
            std::thread::sleep(Duration::from_millis(25));
            while self.tap.pop_slice(buf) > 0 {}
            self.flushed = true;
            return 0;
        }
        let n = self.tap.pop_slice(buf);
        if n > 0
            && let Some(chunk) = buf.get(..n)
        {
            self.spectrum.feed(chunk);
            let _ = self.telemetry.latest(&mut self.last_tel);
            let tel = self.last_tel;
            let bands = *self.spectrum.bands();
            with(|s| {
                s.last_frame = FrameSnapshot {
                    spectrum: bands,
                    telemetry: tel,
                };
            });
        }
        n
    }

    fn has_error(&self) -> bool {
        false
    }
}

struct CapturingCalibrator {
    inner: Box<dyn tine_engine::autotune::Calibrator>,
    fs: f32,
    slot: Arc<Mutex<(Vec<f32>, u32)>>,
}

impl tine_engine::autotune::Calibrator for CapturingCalibrator {
    fn push(&mut self, samples: &[f32]) -> tine_dsp::calibrator::CalProgress {
        self.inner.push(samples)
    }

    fn finish(
        &mut self,
        preset: &tine_dsp::presets::PresetDef,
        macros: tine_protocol::presets::MacroPositions,
        pins: tine_protocol::presets::PinSet,
        current: &DspParams,
    ) -> Result<tine_engine::autotune::CalibrationReport, CalFailure> {
        let voice = self.inner.captured_voice().to_vec();
        let rate = self.fs as u32;
        if let Ok(mut guard) = self.slot.lock() {
            *guard = (voice, rate);
        }
        self.inner.finish(preset, macros, pins, current)
    }

    fn captured_voice(&self) -> &[f32] {
        self.inner.captured_voice()
    }
}

struct CapturingCalibratorFactory {
    inner: Arc<dyn tine_engine::autotune::CalibratorFactory>,
    slot: Arc<Mutex<(Vec<f32>, u32)>>,
}

impl tine_engine::autotune::CalibratorFactory for CapturingCalibratorFactory {
    fn make(&self, fs: f32) -> Box<dyn tine_engine::autotune::Calibrator> {
        Box::new(CapturingCalibrator {
            inner: self.inner.make(fs),
            fs,
            slot: Arc::clone(&self.slot),
        })
    }
}

fn stop_cal_preview_loop_only() {
    let (player, was_paused) = with(|s| {
        let player = s.cal_preview_player.take();
        let paused = std::mem::replace(&mut s.cal_preview_paused_monitor, false);
        (player, paused)
    });
    if let Some(p) = player {
        p.stop();
    }
    if was_paused {
        with(|s| {
            if let MonitorWanted::On(dev) = s.monitor_wanted.clone()
                && let Some(slot) = s.router.as_ref()
                && s.monitor.is_none()
            {
                let feed = slot.router.monitor_feed();
                let out = dev.as_deref().map_or_else(DeviceRef::default, DeviceRef::parse);
                match Monitor::start(&feed, &out) {
                    Ok(m) => s.monitor = Some(m),
                    Err(e) => s.surface(format!("Hear Myself resume failed: {e}")),
                }
            }
        });
    }
}

fn stop_cal_preview() {
    stop_cal_preview_loop_only();
    set_calibration_preview(None);
}

fn resolve_preview_params(
    paths: Option<&EnginePaths>,
    cal: &Calibration,
    style: VoiceStyle,
    keep_pins: bool,
) -> DspParams {
    let owned_paths;
    let resolved_paths = if let Some(p) = paths {
        Some(p)
    } else {
        owned_paths = EnginePaths::system().ok();
        owned_paths.as_ref()
    };
    let profile = resolved_paths.map_or_else(
        || tine_protocol::presets::EndpointProfile::new("default", tine_protocol::presets::Archetype::default()),
        |p| {
            let endpoint = service_endpoint(p);
            persist::load_profile(endpoint.as_str(), p).into_value()
        },
    );
    let current = last_controller_params();
    let macros = recommend_macros(cal, profile.archetype, style, profile.macros);
    let mut pins = profile.pins;
    if !keep_pins {
        for id in CHANGED_PARAMS {
            pins.unpin(id);
        }
        if style != VoiceStyle::KeepCurrent {
            for m in DspMacro::ALL {
                for &id in MacroMapper::drives(m) {
                    pins.unpin(id);
                }
            }
        }
    }
    let mut out = MacroMapper::apply(
        tine_dsp::presets::builtin(profile.archetype),
        Some(cal),
        macros,
        pins,
        &current,
    );
    out.bypass = current.bypass;
    out.clamped()
}

/// [`autotune_start`] against explicit persistence roots (test seam).
pub fn autotune_start_on(paths: &EnginePaths) {
    router_wake("auto-tune");
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
    // Set CALIBRATING before stopping the visualizer so no concurrent
    // `spectrum_on` call can re-open `router_tap()` while Auto-Tune owns it.
    CALIBRATING.store(true, Ordering::Release);
    let cal_guard = CalibratingGuard;
    let is_live_routed = routed_rate.is_some();
    let phase_durations = if is_live_routed {
        (LIVE_COUNTDOWN_S, LIVE_NOISE_S, LIVE_VOICE_S)
    } else {
        (COUNTDOWN_S, NOISE_S, VOICE_S)
    };
    let base_factory: Arc<dyn tine_engine::autotune::CalibratorFactory> = if is_live_routed {
        Arc::new(LiveAutoCalibratorFactory)
    } else {
        Arc::new(tine_engine::autotune::AutoCalibratorFactory)
    };
    let captured_slot: Arc<Mutex<(Vec<f32>, u32)>> = Arc::new(Mutex::new((Vec::new(), ROUTER_FALLBACK_RATE)));
    let factory: Arc<dyn tine_engine::autotune::CalibratorFactory> = Arc::new(CapturingCalibratorFactory {
        inner: base_factory,
        slot: Arc::clone(&captured_slot),
    });
    let paths_owned = paths.clone();
    let capture = match routed_rate {
        Some(rate) => {
            with(|s| {
                if let Some(mut v) = s.visualizer.take() {
                    v.stop();
                }
                s.frames = None;
            });
            tracing::debug!(target: "tine::services", rate_hz = rate, "auto-tune captures from the routing tap");
            CaptureSpec::Source(Box::new(RouterTapSource::new(router_tap(), router_telemetry(), rate)))
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
        factory,
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
            phase_durations,
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
            let _cal_guard = cal_guard;
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
                        let next_prog = if p.phase == CalPhase::Analyzing {
                            0.15
                        } else {
                            p.phase_progress
                        };
                        s.autotune = Some(AutotuneState::Running {
                            phase: p.phase,
                            progress: next_prog,
                            level_db: p.live_level_db,
                            voiced_ms: p.voiced_ms,
                            phase_durations,
                        });
                    }),
                    AutoTuneEvent::Done(outcome) => {
                        // Release the bypass lease first so live DSP resumes right away.
                        with(|s| {
                            s.lease = None;
                        });
                        if is_live_routed && matches!(outcome, AutoTuneOutcome::Applied(_)) {
                            for step in 2..=10 {
                                if cancel.load(Ordering::Acquire) {
                                    break;
                                }
                                let p = (step as f32) / 10.0;
                                with(|s| {
                                    if let Some(AutotuneState::Running {
                                        level_db, voiced_ms, ..
                                    }) = s.autotune
                                    {
                                        s.autotune = Some(AutotuneState::Running {
                                            phase: CalPhase::Analyzing,
                                            progress: p,
                                            level_db,
                                            voiced_ms,
                                            phase_durations,
                                        });
                                    }
                                });
                                std::thread::sleep(Duration::from_millis(35));
                            }
                        }
                        if cancel.load(Ordering::Acquire) {
                            with(|s| {
                                s.autotune = Some(AutotuneState::Failed {
                                    failure: None,
                                    message: "Auto-Tune failed: cancelled".to_owned(),
                                });
                            });
                            break;
                        }
                        match outcome {
                            AutoTuneOutcome::Applied(report) => {
                                let (voice_vec, cap_rate) = captured_slot.lock().map_or_else(
                                    |p| std::mem::take(&mut *p.into_inner()),
                                    |mut g| std::mem::take(&mut *g),
                                );
                                let preview_params = resolve_preview_params(
                                    Some(&paths_owned),
                                    &report.calibration,
                                    VoiceStyle::AutoBalanced,
                                    true,
                                );
                                set_calibration_preview(Some(preview_params));
                                with(|s| {
                                    s.autotune = Some(AutotuneState::ReportPending {
                                        summary: format!(
                                            "measured noise {:+.1} dBFS, speech {:+.1} dBFS, verified peak {:+.1} dBFS",
                                            report.calibration.noise_rms_db,
                                            report.calibration.speech_rms_db,
                                            report.simulated_peak_db
                                        ),
                                        report: *report,
                                        captured_voice: Arc::new(voice_vec),
                                        rate_hz: cap_rate,
                                        preview_style: VoiceStyle::AutoBalanced,
                                        preview_keep_pins: true,
                                        audition_mode: None,
                                    });
                                });
                            }
                            AutoTuneOutcome::Failed(failure) => with(|s| {
                                s.autotune = Some(AutotuneState::Failed {
                                    failure: Some(failure),
                                    message: format!("Auto-Tune rejected the capture: {failure:?}"),
                                });
                            }),
                            AutoTuneOutcome::Error(e) => with(|s| {
                                s.autotune = Some(AutotuneState::Failed {
                                    failure: None,
                                    message: format!("Auto-Tune failed: {e}"),
                                });
                            }),
                        }
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
    with(|s| {
        s.autotune_cancel.store(true, Ordering::Release);
        if matches!(
            s.autotune,
            Some(AutotuneState::ReportPending { .. } | AutotuneState::Failed { .. })
        ) {
            s.autotune = None;
        }
    });
    stop_cal_preview();
}

/// Takes the pending Auto-Tune calibration for application (the pipe's
/// `calibrate_apply` hands it to `Command::ApplyCalibration`); `None`
/// when no run has produced results (or they were already handled).
#[must_use]
pub fn autotune_take_pending() -> Option<Calibration> {
    let cal = with(|s| match s.autotune.take() {
        Some(AutotuneState::ReportPending { report, .. }) => Some(report.calibration),
        other => {
            s.autotune = other;
            None
        }
    });
    if cal.is_some() {
        stop_cal_preview();
    }
    cal
}

/// Discards the pending Auto-Tune results (the pipe's
/// `calibrate_discard`); a no-op when there are none.
pub fn autotune_discard() {
    let cleared = with(|s| s.autotune.take().is_some());
    stop_cal_preview();
    if cleared {
        tracing::info!(target: "tine::services", "auto-tune results discarded");
    }
}

/// Previews the pending Auto-Tune calibration with `style` and `keep_pins`
/// on the live DSP parameter block, and optionally loops the recorded voice
/// in Tuned (`Some(false)`) or Raw (`Some(true)`) mode (`None` stops looping).
#[must_use]
pub fn autotune_preview(
    style: VoiceStyle,
    keep_pins: bool,
    audition_raw: Option<bool>,
    output_device: Option<&str>,
) -> Value {
    let pending = with(|s| match &s.autotune {
        Some(AutotuneState::ReportPending {
            report,
            captured_voice,
            rate_hz,
            ..
        }) => Some((report.calibration, Arc::clone(captured_voice), *rate_hz)),
        _ => None,
    });
    let Some((cal, voice, rate_hz)) = pending else {
        return autotune_state();
    };

    let preview_params = resolve_preview_params(EnginePaths::system().ok().as_ref(), &cal, style, keep_pins);
    set_calibration_preview(Some(preview_params));

    if let Some(play_raw) = audition_raw
        && !voice.is_empty()
    {
        let paused_mon = with(|s| {
            if let Some(m) = s.monitor.take() {
                s.cal_preview_paused_monitor = true;
                Some(m)
            } else {
                None
            }
        });
        if let Some(m) = paused_mon {
            m.stop();
        }

        let still_pending = with(|s| {
            if !matches!(s.autotune, Some(AutotuneState::ReportPending { .. })) {
                return false;
            }
            let engine = s.denoise_engine;
            if let Some(player) = s.cal_preview_player.as_ref() {
                player.update(preview_params, engine, play_raw);
            } else {
                let out_ref = output_device.map_or_else(DeviceRef::default, DeviceRef::parse);
                s.cal_preview_player = Some(CalibrationPreviewPlayer::start(
                    (*voice).clone(),
                    rate_hz,
                    out_ref,
                    preview_params,
                    engine,
                    play_raw,
                ));
            }
            if let Some(AutotuneState::ReportPending {
                preview_style,
                preview_keep_pins,
                audition_mode,
                ..
            }) = s.autotune.as_mut()
            {
                *preview_style = style;
                *preview_keep_pins = keep_pins;
                *audition_mode = Some(play_raw);
            }
            true
        });
        if !still_pending {
            stop_cal_preview();
        }
    } else {
        stop_cal_preview_loop_only();
        with(|s| {
            if let Some(AutotuneState::ReportPending {
                preview_style,
                preview_keep_pins,
                audition_mode,
                ..
            }) = s.autotune.as_mut()
            {
                *preview_style = style;
                *preview_keep_pins = keep_pins;
                *audition_mode = None;
            }
        });
    }

    autotune_state()
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
            phase_durations: (cd_s, noise_s, voice_s),
        }) => {
            let p = progress.clamp(0.0, 1.0);
            let remaining_secs = match phase {
                CalPhase::Countdown => cd_s * (1.0 - p),
                CalPhase::Noise => noise_s * (1.0 - p),
                CalPhase::Voice => voice_s * (1.0 - p),
                CalPhase::Analyzing | CalPhase::Done | CalPhase::Failed => 0.0,
            };
            let denom = (noise_s + voice_s).max(0.001);
            let overall_progress = match phase {
                CalPhase::Countdown => 0.0,
                CalPhase::Noise => (p * noise_s / denom).clamp(0.0, 1.0),
                CalPhase::Voice => ((noise_s + p * voice_s) / denom).clamp(0.0, 1.0),
                CalPhase::Analyzing | CalPhase::Done | CalPhase::Failed => 1.0,
            };
            json!({
                "flow": "running",
                "phase": format!("{phase:?}"),
                "progress": progress,
                "remaining_secs": remaining_secs,
                "overall_progress": overall_progress,
                "level_db": level_db,
                "voiced_ms": voiced_ms,
            })
        }
        Some(AutotuneState::ReportPending {
            report,
            summary,
            captured_voice,
            audition_mode,
            ..
        }) => {
            let cal = &report.calibration;
            json!({
                "flow": "report-pending",
                "summary": summary,
                "report": {
                    "noise_rms_db": cal.noise_rms_db,
                    "noise_peak_db": cal.noise_peak_db,
                    "speech_rms_db": cal.speech_rms_db,
                    "crest_db": cal.crest_db,
                    "tilt_db": cal.tilt_db,
                    "simulated_peak_db": report.simulated_peak_db,
                    "simulated_rms_db": report.simulated_rms_db,
                    "audition_mode": match audition_mode {
                        None => "stopped",
                        Some(false) => "tuned",
                        Some(true) => "raw",
                    },
                    "has_voice_preview": !captured_voice.is_empty(),
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
    let len = std::mem::size_of::<ShmLayout>();
    let mut created: Option<(String, HANDLE)> = None;
    for _ in 0..8 {
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("Local\\Tine.v1.autotune-shim-{}-{n}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
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
        // SAFETY: `GetLastError` reads the thread's last-error value set by
        // `CreateFileMappingW`.
        if unsafe { GetLastError() } == ERROR_ALREADY_EXISTS {
            // SAFETY: handle returned by `CreateFileMappingW` on collision;
            // close and retry with a fresh counter value.
            unsafe { CloseHandle(mapping) }.ok();
            continue;
        }
        created = Some((name, mapping));
        break;
    }
    let Some((name, mapping)) = created else {
        return Err("lease shim: mapping name collision".to_owned());
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
    let join = std::thread::Builder::new()
        .name("tine-lease-shim".to_owned())
        .spawn(move || {
            loop {
                let stop_now = stop_thread.load(Ordering::Acquire);
                let now = now_ms();
                let hot = header.bypass_lease_until_ms.load(Ordering::Acquire) > now;
                set_lease_bypass(hot && !stop_now);
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
pub fn monitor_apply_on(_paths: &EnginePaths, enabled: bool, device: Option<&str>) {
    if !enabled {
        with(|s| {
            s.monitor_wanted = MonitorWanted::Off;
            if let Some(m) = s.monitor.take() {
                m.stop();
                tracing::info!(target: "tine::services", "Hear Myself off (streams stopped)");
            }
        });
        return;
    }
    router_wake("hear-myself");
    let out = device.map_or_else(DeviceRef::default, DeviceRef::parse);
    with(|s| {
        s.monitor_wanted = MonitorWanted::On(device.map(str::to_owned));
        let Some(feed) = s.router.as_ref().map(|r| r.router.monitor_feed()) else {
            if let Some(m) = s.monitor.take() {
                m.stop();
            }
            s.surface("Hear Myself will start once audio routing is active".to_owned());
            return;
        };
        match &mut s.monitor {
            Some(m) => {
                if let Err(e) = m.restart(&feed, &out) {
                    s.surface(format!("Hear Myself restart failed: {e}"));
                    s.monitor = None;
                }
            }
            None => match Monitor::start(&feed, &out) {
                Ok(m) => {
                    tracing::info!(target: "tine::services",
                        latency_ms = m.latency_ms(),
                        "Hear Myself started");
                    s.monitor = Some(m);
                }
                Err(e) => s.surface(format!("Hear Myself failed to start: {e}")),
            },
        }
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

/// Returns `true` when the active "Hear Myself" monitor stream reported a device error.
#[must_use]
pub fn monitor_has_error() -> bool {
    with(|s| s.monitor.as_ref().is_some_and(Monitor::has_error))
}

/// Rebuilds the active "Hear Myself" monitor stream after an error or sleep/resume event.
#[must_use]
pub fn monitor_recover(output_device: Option<&str>) -> bool {
    let should_recover = with(|s| {
        if let Some(m) = s.monitor.take() {
            m.stop();
            true
        } else {
            !matches!(s.monitor_wanted, MonitorWanted::Off)
        }
    });
    if !should_recover {
        return false;
    }
    monitor_apply(true, output_device);
    with(|s| s.monitor.is_some())
}

/// Starts a 5-second Mic Check (Raw vs Tine sample-aligned A/B) session.
#[must_use]
pub fn mic_check_start(params: DspParams, output_device: Option<&str>) -> Value {
    stop_cal_preview();
    let _ = router_wake("mic-check");
    let out_ref = output_device.map_or_else(DeviceRef::default, DeviceRef::parse);
    let mut err: Option<String> = None;
    let prev = with(|s| s.mic_check.take());
    if let Some(old) = prev {
        old.stop();
    }
    let old_mon = with(|s| {
        if let Some(m) = s.monitor.take() {
            s.mic_check_paused_monitor = true;
            Some(m)
        } else {
            None
        }
    });
    if let Some(m) = old_mon {
        m.stop();
    }
    with(|s| {
        let Some(slot) = s.router.as_ref() else {
            err = Some("Mic Check requires an active microphone routing loop.".to_owned());
            return;
        };
        let feed = slot.router.raw_capture_feed();
        s.mic_check = Some(MicCheckRunner::start(feed, out_ref, params, s.denoise_engine));
    });
    if let Some(message) = err {
        mic_check_restore_monitor();
        return json!({ "flow": "failed", "message": message });
    }
    mic_check_query()
}

/// Current state of the 5-second Mic Check flow for `mic_check_query`.
#[must_use]
pub fn mic_check_query() -> Value {
    with(|s| match s.mic_check.as_ref().map(MicCheckRunner::state) {
        None | Some(MicCheckState::Idle) => json!({ "flow": "idle" }),
        Some(MicCheckState::Recording { progress, level_db }) => json!({
            "flow": "recording",
            "progress": progress,
            "level_db": level_db,
        }),
        Some(MicCheckState::Playing {
            mode,
            progress,
            duration_s,
        }) => json!({
            "flow": "playing",
            "raw": mode == MicCheckMode::Raw,
            "progress": progress,
            "duration_s": duration_s,
        }),
        Some(MicCheckState::Failed(message)) => json!({
            "flow": "failed",
            "message": message,
        }),
    })
}

/// Switches Mic Check playback between Tine (`false`) and raw mic (`true`).
#[must_use]
pub fn mic_check_set_mode(raw: bool) -> bool {
    with(|s| {
        if let Some(mc) = s.mic_check.as_ref() {
            mc.set_mode(if raw {
                MicCheckMode::Raw
            } else {
                MicCheckMode::Processed
            });
            true
        } else {
            false
        }
    })
}

/// Stops any active Mic Check recording/playback and restores paused monitoring.
pub fn mic_check_stop() {
    let runner = with(|s| s.mic_check.take());
    if let Some(mc) = runner {
        mc.stop();
    }
    mic_check_restore_monitor();
}

fn mic_check_restore_monitor() {
    with(|s| {
        if !s.mic_check_paused_monitor {
            return;
        }
        s.mic_check_paused_monitor = false;
        if let MonitorWanted::On(dev) = s.monitor_wanted.clone()
            && let Some(slot) = s.router.as_ref()
            && s.monitor.is_none()
        {
            let feed = slot.router.monitor_feed();
            let out = dev.as_deref().map_or_else(DeviceRef::default, DeviceRef::parse);
            match Monitor::start(&feed, &out) {
                Ok(m) => s.monitor = Some(m),
                Err(e) => s.surface(format!("Hear Myself resume failed: {e}")),
            }
        }
    });
}

/// Tears every service down (shutdown path): monitor streams, the
/// Auto-Tune mirror and lease shim, the routing loop, and the
/// visualizer pump. Idempotent.
pub fn shutdown() {
    CALIBRATING.store(false, Ordering::Release);
    GATE_STOP.store(true, Ordering::Release);
    wake_gate();
    let preview_player = with(|s| {
        s.monitor_wanted = MonitorWanted::Off;
        if let Some(m) = s.monitor.take() {
            m.stop();
        }
        if let Some(mc) = s.mic_check.take() {
            mc.stop();
        }
        s.mic_check_paused_monitor = false;
        let player = s.cal_preview_player.take();
        s.cal_preview_paused_monitor = false;
        s.autotune = None;
        s.autotune_cancel.store(true, Ordering::Release);
        s.lease = None;
        if let Some(slot) = s.router.take() {
            slot.router.stop();
        }
        s.parked = None;
        if let Some(mut v) = s.visualizer.take() {
            v.stop();
        }
        s.frames = None;
        player
    });
    drop(preview_player);
    set_calibration_preview(None);
}

// ---------------------------------------------------------------------------
// Listener gate (T24): park the routing loop while nobody listens
// ---------------------------------------------------------------------------

/// Set on shutdown so the gate never resumes a loop the app is tearing down.
static GATE_STOP: AtomicBool = AtomicBool::new(false);
static GATE_WAKE: OnceLock<Arc<SessionWake>> = OnceLock::new();

fn gate_wake_signal() -> &'static Arc<SessionWake> {
    GATE_WAKE.get_or_init(SessionWake::new)
}

/// Wakes the listener gate immediately when internal need or policy changes.
pub fn wake_gate() {
    gate_wake_signal().notify();
}

/// Returns `true` when automatic idle parking is enabled (`false` = Keep Warm / PTT mode).
#[must_use]
pub fn park_when_unused() -> bool {
    with(|s| s.park_when_unused)
}

/// Enables or disables automatic idle parking (disabling keeps the DSP warm for PTT).
pub fn set_park_when_unused(enabled: bool) {
    with(|s| s.park_when_unused = enabled);
    if !enabled {
        let _ = router_wake("keep-warm");
    }
    wake_gate();
}

/// Whether something inside Tine consumes the processed signal right now:
/// Keep-Warm PTT mode, "Hear Myself", the window's meters/spectrum, Mic Check,
/// or Auto-Tune.
fn internal_need(s: &Services) -> bool {
    !s.park_when_unused
        || !matches!(s.monitor_wanted, MonitorWanted::Off)
        || s.monitor.is_some()
        || s.visualizer.is_some()
        || s.last_pull.is_some_and(|t| t.elapsed() < VISUALIZER_TAIL)
        || s.autotune.is_some()
        || s.mic_check.is_some()
        || s.lease.is_some()
        || CALIBRATING.load(Ordering::Acquire)
}

/// Resumes a parked routing loop now. Internal consumers call this before
/// they need the signal, so they never wait for the gate's poll. Returns
/// `true` when a parked loop was restarted; a failed restart re-parks so the
/// gate retries on its next poll.
pub fn router_wake(reason: &str) -> bool {
    let Some(parked) = with(|s| s.parked.take()) else {
        return false;
    };
    let routed = router_follow(&parked.endpoint);
    if routed {
        tracing::info!(target: "tine::services", reason, endpoint = %parked.endpoint, "routing loop resumed");
    } else {
        tracing::warn!(target: "tine::services", reason, endpoint = %parked.endpoint,
            "routing loop resume failed; staying parked");
        with(|s| {
            if s.router.is_none() {
                s.parked = Some(parked);
            }
        });
    }
    wake_gate();
    routed
}

/// Stops the routing loop if nothing needs it (re-checked under the lock).
fn park() {
    with(|s| {
        if internal_need(s) {
            return;
        }
        if let Some(slot) = s.router.take() {
            tracing::info!(target: "tine::services", endpoint = %slot.endpoint,
                "routing loop parked (no app is recording the virtual mic)");
            s.parked = Some(ParkedRoute {
                endpoint: slot.endpoint.clone(),
                sink: slot.sink.clone(),
            });
            slot.router.stop();
        }
    });
}

/// Starts the gate thread once (after the first successful route).
fn ensure_listener_gate() {
    static GATE: OnceLock<()> = OnceLock::new();
    GATE.get_or_init(|| {
        if std::thread::Builder::new()
            .name("tine-listener-gate".to_owned())
            .spawn(gate_loop)
            .is_err()
        {
            tracing::warn!(target: "tine::services", "listener gate spawn failed; routing never parks");
        }
    });
}

fn gate_loop() {
    let mut watcher = match CaptureSessionWatcher::new() {
        Ok(w) => w,
        Err(e) => {
            tracing::warn!(target: "tine::services", "listener gate disabled: {e}");
            return;
        }
    };
    let wake = Arc::clone(gate_wake_signal());
    let own_pid = std::process::id();
    let mut policy = ParkPolicy::default();
    let mut hook: Option<(String, CableSessionEventHook)> = None;
    loop {
        if GATE_STOP.load(Ordering::Acquire) {
            return;
        }
        let (parked, sink) = with(|s| match (&s.router, &s.parked) {
            (Some(r), _) => (false, Some(r.sink.clone())),
            (None, Some(p)) => (true, Some(p.sink.clone())),
            (None, None) => (false, None),
        });
        // Router down for another reason (error backoff, no cable): the
        // shell's recovery owns it; the gate only acts on its own states.
        let Some(sink) = sink else {
            hook = None;
            policy = ParkPolicy::default();
            let _ = wake.wait_timeout(RUNNING_POLL);
            continue;
        };

        if hook.as_ref().is_some_and(|(s, _)| s != &sink) {
            hook = None;
        }
        if hook.is_none()
            && let Ok(Some(h)) = watcher.attach_event_hook(&|name| is_sink_capture_side(name, &sink), Arc::clone(&wake))
        {
            tracing::debug!(target: "tine::services", sink = %sink, "attached event-driven WASAPI session hook");
            hook = Some((sink.clone(), h));
        }

        // Unknown (probe error, capture side missing) counts as listening.
        let listener = if let Ok(Some(list)) = watcher.sessions(&|name| is_sink_capture_side(name, &sink)) {
            foreign_listener(&list, own_pid)
        } else {
            hook = None;
            true
        };
        let needed = listener || with(|s| internal_need(s));
        match policy.decide(Instant::now(), parked, needed) {
            GateAction::Park => park(),
            GateAction::Resume => {
                if !GATE_STOP.load(Ordering::Acquire) {
                    router_wake("listener");
                }
            }
            GateAction::Stay => {}
        }

        let wait_dur = if hook.is_some() {
            Duration::from_secs(if parked { 5 } else { 2 })
        } else if parked {
            PARKED_POLL
        } else {
            RUNNING_POLL
        };
        let signaled = wake.wait_timeout(wait_dur);
        if GATE_STOP.load(Ordering::Acquire) {
            return;
        }
        if signaled && let Some((_, h)) = hook.as_mut() {
            h.refresh_sessions();
        }
    }
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
    if CALIBRATING.load(Ordering::Acquire) {
        return None;
    }
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

    // All `services::tests` share the process-global `SERVICES` / `BLOCK` state,
    // so serialize them when `cargo test` runs tests in parallel threads.
    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn lock_test() -> std::sync::MutexGuard<'static, ()> {
        TEST_LOCK.lock().unwrap_or_else(std::sync::PoisonError::into_inner)
    }

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
        let _guard = lock_test();
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
        let _guard = lock_test();
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
        let _guard = lock_test();
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
        let _guard = lock_test();
        let paths = temp_paths();
        super::shutdown();
        // In-flight run: politely rejected, untouched.
        with(|s| {
            s.autotune = Some(AutotuneState::Running {
                phase: CalPhase::Countdown,
                progress: 0.0,
                level_db: -90.0,
                voiced_ms: 0,
                phase_durations: (COUNTDOWN_S, NOISE_S, VOICE_S),
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
                captured_voice: Arc::new(Vec::new()),
                rate_hz: ROUTER_FALLBACK_RATE,
                preview_style: VoiceStyle::AutoBalanced,
                preview_keep_pins: true,
                audition_mode: None,
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
    fn autotune_preview_updates_param_block_and_reverts_on_discard() {
        let _guard = lock_test();
        super::shutdown();
        let baseline = DspParams::DEFAULT;
        publish_params(&baseline);

        let report = tine_engine::autotune::CalibrationReport {
            calibration: tine_protocol::presets::Calibration {
                noise_rms_db: -50.0,
                noise_peak_db: -44.0,
                speech_rms_db: -22.0,
                crest_db: 12.0,
                tilt_db: 3.0,
                derived: tine_protocol::presets::CalibrationOverrides {
                    input_gain_db: 4.0,
                    gate_threshold_db: -44.0,
                    denoise_mix: 0.50,
                    comp_thresh_db: -16.0,
                    comp_makeup_db: 2.0,
                },
            },
            changes: [
                (tine_protocol::params::ParamId::InputGainDb, 0.0, 4.0),
                (tine_protocol::params::ParamId::GateThresholdDb, -40.0, -44.0),
                (tine_protocol::params::ParamId::DenoiseMix, 0.5, 0.50),
                (tine_protocol::params::ParamId::CompThreshDb, -20.0, -16.0),
                (tine_protocol::params::ParamId::CompMakeupDb, 0.0, 2.0),
            ],
            simulated_peak_db: -7.5,
            simulated_rms_db: -20.0,
        };
        with(|s| {
            s.autotune = Some(AutotuneState::ReportPending {
                report,
                summary: "measured".to_owned(),
                captured_voice: Arc::new(Vec::new()),
                rate_hz: ROUTER_FALLBACK_RATE,
                preview_style: VoiceStyle::AutoBalanced,
                preview_keep_pins: true,
                audition_mode: None,
            });
        });

        let state_b = autotune_preview(VoiceStyle::Broadcast, true, None, None);
        assert_eq!(state_b["flow"], "report-pending");
        assert_eq!(state_b["report"]["audition_mode"], "stopped");
        assert_eq!(state_b["report"]["has_voice_preview"], false);

        let block = param_block();
        let mut generation = 0_u64;
        let mut params_broadcast = DspParams::DEFAULT;
        assert!(block.load_if_changed(&mut generation, &mut params_broadcast));

        let _state_c = autotune_preview(VoiceStyle::Crisp, true, None, None);
        let mut params_crisp = DspParams::DEFAULT;
        assert!(block.load_if_changed(&mut generation, &mut params_crisp));
        assert_ne!(
            params_broadcast, params_crisp,
            "Broadcast and Crisp previews must differ"
        );

        autotune_discard();
        let mut params_reverted = DspParams::DEFAULT;
        assert!(block.load_if_changed(&mut generation, &mut params_reverted));
        assert_eq!(
            params_reverted,
            last_controller_params(),
            "discard must revert param_block() back to last_controller_params()"
        );
    }

    #[test]
    fn surface_message_round_trips_through_take_messages() {
        let _guard = lock_test();
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
        let _guard = lock_test();
        super::shutdown();
        assert_eq!(autotune_state()["flow"], "idle");
        assert!(autotune_take_pending().is_none());
        autotune_discard();
        assert_eq!(autotune_state()["flow"], "idle");
        assert_eq!(router_status()["on"], false);
    }

    #[test]
    fn lease_shim_confirms_bypass_and_restores_on_clear() {
        let _guard = lock_test();
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
        let _guard = lock_test();
        monitor_apply(false, None);
        assert_eq!(monitor_status()["on"], false);
    }

    #[test]
    fn monitor_without_router_defers_and_surfaces_a_user_message() {
        let _guard = lock_test();
        super::shutdown();
        let paths = temp_paths();
        monitor_apply_on(&paths, true, Some("__nonexistent_tine_output_device__"));
        assert!(
            take_messages()
                .iter()
                .any(|m| m.contains("Hear Myself will start once audio routing is active"))
        );
        assert_eq!(monitor_status()["on"], false);
        monitor_apply(false, None);
    }
}
