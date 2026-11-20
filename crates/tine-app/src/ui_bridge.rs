//! Bridge between the engine control plane and `tine-ui`.
//!
//! Two hosts share this module. Per D-017 the Rune window lives in the
//! short-lived `--ui` child process while the controller and the audio
//! services are owned by the tray parent; the in-process layout this task
//! was originally specified against is kept alive by the headless bridge
//! that the integration suite drives.
//!
//! 1. **In-process [`EngineBridge`]** maps every [`Intent`] to
//!    `ControllerHandle::send`, the T16 `AutoTuneRunner`, the T16
//!    `Monitor` and `apo_admin`, and maps engine `Event::ModelChanged` /
//!    visualizer [`UiFrame`]s into `tine-ui`'s `UiModel` / [`LiveFrame`].
//!    It owns the visualizer lifecycle (a pump only while "visible") and
//!    feeds frames latest-wins, so an in-flight frame is simply dropped —
//!    the coalescing the spec asks for. The integration suite in
//!    `crates/tine-app/tests/e2e_flow.rs` runs the whole app through it
//!    headlessly (in-process [`tine_protocol::sync::ParamBlock`] target +
//!    fake audio source; intents → params + persisted files).
//! 2. **The `--ui` child host ([`run_ui_child`])** swaps T17's placeholder
//!    view for the real `tine-ui` app. Its [`ChildSink`] implements
//!    `IntentSink` by forwarding intents to the parent's command pipe,
//!    polling `status`/`meters` at ≤ 60 Hz while visible, enumerating
//!    endpoints and the D-018 virtual-cable badge locally through
//!    `apo_admin`, and detecting the first run from the settings file.
//!
//! Every engine or admin failure maps to a `tracing` entry **and** a
//! collected user-facing message; nothing fails silently.

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use serde_json::Value;
use tine_dsp::presets::builtin as builtin_preset;
use tine_engine::apo_admin::{self, ApoState, CaptureEndpoint};
use tine_engine::audio_io::{DeviceRef, list_outputs};
use tine_engine::autotune::{
    AutoTuneConfig, AutoTuneEvent, AutoTuneHandle, AutoTuneOutcome, AutoTuneRunner, CalFailure, CalPhase, CalProgress,
    CalibrationReport, CalibratorFactory, CaptureSpec,
};
use tine_engine::controller::{Command, ControllerHandle, Event};
use tine_engine::model::{AppModel, EndpointId, EngineError, KeepPins, Macro as EngineMacro, MonitorSettings, resolve};
use tine_engine::monitor::{Monitor, MonitorFeed};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;
use tine_engine::shm_client::ShmClient;
use tine_engine::visualizer::{EngineSignals, UiFrame, Visualizer};
use tine_protocol::params::{DenoiseEngine, DspParams, ParamId, SPECS, Unit};
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, EndpointProfile, MacroPositions};
use tine_protocol::sync::Triple;
use tine_protocol::telemetry::AudioTelemetryFrame;
use tine_ui::state::LiveFrame;
use tine_ui::views::autotune::{autotune_from_progress, calibration_from_report};
use tine_ui::{
    ApoBadge, AppState, AutoTuneStep, AutoTuneUi, CalFailureUi, CalibrationParamDiff, CalibrationUi, EndpointRow,
    Intent, IntentSink, Macro as UiMacro, MicCheckStep, MonitorUi, UiModel,
};

use crate::proto::{PipeRequest, PipeResponse};
use crate::single_instance;

/// Analyzer floor in dBFS; maps to bar level `0.0` ([`band_level`]).
pub const SPECTRUM_FLOOR_DB: f32 = -90.0;

// ---------------------------------------------------------------------------
// Pure adapters (every engine → UI mapping lives in this one module)
// ---------------------------------------------------------------------------

/// Maps one analyzer band value (dBFS) to the `0.0..=1.0` bar level
/// `tine-ui` renders (floor [`SPECTRUM_FLOOR_DB`], full scale 0 dBFS).
#[must_use]
pub fn band_level(db: f32) -> f32 {
    if !db.is_finite() {
        return 0.0;
    }
    ((db - SPECTRUM_FLOOR_DB) / -SPECTRUM_FLOOR_DB).clamp(0.0, 1.0)
}

/// Maps a visualizer [`UiFrame`] (the 60 Hz publication) to the UI's
/// [`LiveFrame`]. Coalescing is the triple buffer's latest-wins.
#[must_use]
pub fn live_from_frame(f: &UiFrame) -> LiveFrame {
    LiveFrame {
        telemetry: f.telemetry,
        spectrum: f.spectrum.map(band_level),
    }
}

/// Maps the pipe `meters` payload (telemetry object + `spectrum` array in
/// dBFS) into a [`LiveFrame`]; `None` when the payload is malformed.
#[must_use]
pub fn live_from_meters(data: &Value) -> Option<LiveFrame> {
    let tel = data.get("telemetry")?.as_object()?;
    let get = |k: &str| tel.get(k).and_then(Value::as_f64).map(|v| v as f32);
    let telemetry = AudioTelemetryFrame {
        sequence: tel.get("sequence").and_then(Value::as_u64).unwrap_or_default(),
        input_rms_db: get("input_rms_db").unwrap_or(-60.0),
        input_peak_db: get("input_peak_db").unwrap_or(-60.0),
        output_rms_db: get("output_rms_db").unwrap_or(-60.0),
        output_peak_db: get("output_peak_db").unwrap_or(-60.0),
        gate_attenuation_db: get("gate_attenuation_db").unwrap_or_default(),
        comp_gain_reduction_db: get("comp_gain_reduction_db").unwrap_or_default(),
        deesser_reduction_db: get("deesser_reduction_db").unwrap_or_default(),
        ..AudioTelemetryFrame::default()
    };
    let raw = data.get("spectrum")?.as_array()?;
    let mut spectrum = [0.0_f32; 32];
    for (slot, v) in spectrum.iter_mut().zip(raw.iter()) {
        *slot = band_level(v.as_f64().map_or(SPECTRUM_FLOOR_DB, |x| x as f32));
    }
    Some(LiveFrame { telemetry, spectrum })
}

/// Wire period of the child's Auto-Tune wizard poll (~30 Hz so countdown rings
/// and live level meters glide smoothly alongside Rune spring motion).
const WIZARD_PERIOD: Duration = Duration::from_millis(33);

/// Parses the wire phase name (`Debug` form of `CalPhase`).
fn phase_from_wire(s: &str) -> CalPhase {
    match s {
        "Noise" => CalPhase::Noise,
        "Voice" => CalPhase::Voice,
        "Analyzing" => CalPhase::Analyzing,
        "Done" => CalPhase::Done,
        "Failed" => CalPhase::Failed,
        _ => CalPhase::Countdown,
    }
}

/// Parses the wire rejection (`{"type":"TooNoisy","snr_db":…}`); `None`
/// for unknown or missing types.
fn failure_from_wire(v: &Value) -> Option<CalFailure> {
    let get = |k: &str| v.get(k).and_then(Value::as_f64).map(|x| x as f32);
    match v.get("type").and_then(Value::as_str)? {
        "TooQuiet" => Some(CalFailure::TooQuiet),
        "TooNoisy" => Some(CalFailure::TooNoisy {
            snr_db: get("snr_db").unwrap_or_default(),
        }),
        "Clipping" => Some(CalFailure::Clipping {
            pct: get("pct").unwrap_or_default(),
        }),
        "NotEnoughSpeech" => Some(CalFailure::NotEnoughSpeech {
            voiced_ms: v.get("voiced_ms").and_then(Value::as_u64).unwrap_or_default() as u32,
        }),
        "GainOutOfRange" => Some(CalFailure::GainOutOfRange {
            needed_db: get("needed_db").unwrap_or_default(),
        }),
        _ => None,
    }
}

/// Viewer-facing unit suffix for a diff row (the child-side twin of
/// `tine-ui`'s private `unit_suffix` — the wire carries SPECS keys, the
/// card carries `ParamId`s with labels).
fn unit_suffix(id: ParamId) -> &'static str {
    match id.spec().unit {
        Unit::Db => "dB",
        Unit::Dbfs => "dBFS",
        Unit::Hz => "Hz",
        Unit::Ms => "ms",
        Unit::Ratio | Unit::None => "",
    }
}

/// Maps the `calibrate_query` payload (the parent's Auto-Tune flow
/// state) into the wizard's view model, the results card it may carry,
/// and whether the flow reached a terminal state (the poll can stop).
/// `None` when the payload is malformed — callers ignore that frame.
#[must_use]
pub fn autotune_ui_from_state(data: &Value) -> Option<(AutoTuneUi, Option<CalibrationUi>, bool)> {
    match data.get("flow").and_then(Value::as_str)? {
        "idle" => Some((AutoTuneUi::default(), None, true)),
        "running" => {
            let progress = CalProgress {
                phase: phase_from_wire(data.get("phase").and_then(Value::as_str).unwrap_or_default()),
                phase_progress: data.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                live_level_db: data.get("level_db").and_then(Value::as_f64).unwrap_or(-90.0) as f32,
                voiced_ms: data.get("voiced_ms").and_then(Value::as_u64).unwrap_or_default() as u32,
            };
            let mut ui = autotune_from_progress(&progress, None);
            if let Some(rem) = data.get("remaining_secs").and_then(Value::as_f64).map(|x| x as f32) {
                match &mut ui.step {
                    AutoTuneStep::Prepare { remaining_secs }
                    | AutoTuneStep::Silence { remaining_secs }
                    | AutoTuneStep::Speak { remaining_secs } => {
                        *remaining_secs = rem.max(0.0);
                    }
                    _ => {}
                }
            }
            if let Some(overall) = data.get("overall_progress").and_then(Value::as_f64).map(|x| x as f32) {
                ui.progress = overall.clamp(0.0, 1.0);
            }
            Some((ui, None, false))
        }
        "report-pending" => {
            let calibration = data.get("report").and_then(calibration_from_state);
            let ui = AutoTuneUi {
                step: AutoTuneStep::Results,
                progress: 1.0,
                ..AutoTuneUi::default()
            };
            Some((ui, calibration, true))
        }
        "failed" => {
            let failure = data
                .get("failure")
                .and_then(failure_from_wire)
                .unwrap_or(CalFailure::TooQuiet);
            let ui = AutoTuneUi {
                step: AutoTuneStep::Failed(CalFailureUi::from(failure)),
                progress: 1.0,
                ..AutoTuneUi::default()
            };
            Some((ui, None, true))
        }
        _ => None, // unknown future flow: skip this frame
    }
}

/// Reconstructs the full [`Calibration`] (including `tilt_db`, `crest_db`, and
/// `noise_peak_db`) from the `calibrate_query` `report-pending` payload.
#[must_use]
pub fn pending_calibration_from_state(data: &Value, archetype: Archetype) -> Option<Calibration> {
    if data.get("flow").and_then(Value::as_str) != Some("report-pending") {
        return None;
    }
    let report = data.get("report")?;
    let get = |k: &str| report.get(k).and_then(Value::as_f64).map(|x| x as f32);
    let base = builtin_preset(archetype).base;
    let find_after = |target: &str, fallback: f32| {
        report
            .get("changes")
            .and_then(Value::as_array)
            .and_then(|arr| {
                arr.iter()
                    .find(|e| e.get("key").and_then(Value::as_str) == Some(target))
            })
            .and_then(|e| e.get("after").and_then(Value::as_f64))
            .map_or(fallback, |x| x as f32)
    };
    let noise_rms_db = get("noise_rms_db").unwrap_or(SPECTRUM_FLOOR_DB);
    Some(Calibration {
        noise_rms_db,
        noise_peak_db: get("noise_peak_db").unwrap_or(noise_rms_db),
        speech_rms_db: get("speech_rms_db").unwrap_or(SPECTRUM_FLOOR_DB),
        crest_db: get("crest_db").unwrap_or(12.0),
        tilt_db: get("tilt_db").unwrap_or(0.0),
        derived: CalibrationOverrides {
            input_gain_db: find_after("input_gain_db", base.input_gain_db),
            gate_threshold_db: find_after("gate_threshold_db", base.gate_threshold_db),
            denoise_mix: find_after("denoise_mix", base.denoise_mix),
            comp_thresh_db: find_after("comp_thresh_db", base.comp_thresh_db),
            comp_makeup_db: find_after("comp_makeup_db", base.comp_makeup_db),
        },
    })
}

/// Results-card view model from the wire report (SPECS-keyed changes
/// become `ParamId` rows with labels and units).
#[must_use]
pub fn calibration_from_state(data: &Value) -> Option<CalibrationUi> {
    let get = |k: &str| data.get(k).and_then(Value::as_f64).map(|x| x as f32);
    let mut changes = Vec::new();
    if let Some(list) = data.get("changes").and_then(Value::as_array) {
        for entry in list {
            let Some(key) = entry.get("key").and_then(Value::as_str) else {
                continue;
            };
            let Some(id) = crate::cli::param_id_by_key(key) else {
                continue;
            };
            changes.push(CalibrationParamDiff {
                param: id,
                label: id.spec().label,
                before: entry.get("before").and_then(Value::as_f64).map_or(0.0, |x| x as f32),
                after: entry.get("after").and_then(Value::as_f64).map_or(0.0, |x| x as f32),
                unit: unit_suffix(id),
            });
        }
    }
    Some(CalibrationUi {
        noise_rms_db: get("noise_rms_db").unwrap_or(SPECTRUM_FLOOR_DB),
        speech_rms_db: get("speech_rms_db").unwrap_or(SPECTRUM_FLOOR_DB),
        simulated_peak_db: get("simulated_peak_db").unwrap_or(SPECTRUM_FLOOR_DB),
        simulated_rms_db: get("simulated_rms_db").unwrap_or(SPECTRUM_FLOOR_DB),
        changes,
    })
}

/// Maps the T14 virtual-cable [`ApoState`] to the UI badge (D-018: the
/// badge reflects the virtual cable, not an APO registration).
#[must_use]
pub fn badge_from_health(state: &ApoState) -> ApoBadge {
    match state {
        ApoState::Active { .. } => ApoBadge::Active,
        ApoState::InstalledNotLoaded | ApoState::RemovedBySystem | ApoState::EnhancementsDisabled => ApoBadge::Inactive,
        ApoState::NotInstalled => ApoBadge::NotInstalled,
        ApoState::Error(e) => ApoBadge::Error(e.to_string()),
    }
}

/// Maps endpoint records to the UI rows (the id is kept as the picker's
/// stable choice key).
/// `true` when `spawn_device_watcher` saw a Windows audio device change (or on
/// initial startup) so `child_refresh_model` re-enumerates COM endpoints.
static DEVICES_DIRTY: AtomicBool = AtomicBool::new(true);

/// Maps `CaptureEndpoint` records into `tine-ui` picker rows, excluding
/// virtual-cable loopback inputs (`CABLE Output`) when physical mics exist so
/// users cannot accidentally route Tine's own output back into its input.
#[must_use]
pub fn endpoint_rows(list: &[CaptureEndpoint]) -> Vec<EndpointRow> {
    let physical: Vec<EndpointRow> = list
        .iter()
        .filter(|e| {
            let lower = e.name.to_ascii_lowercase();
            !lower.contains("cable output") && !lower.contains("vb-audio")
        })
        .map(|e| EndpointRow {
            id: e.id.clone(),
            name: e.name.clone(),
        })
        .collect();
    if !physical.is_empty() {
        return physical;
    }
    list.iter()
        .map(|e| EndpointRow {
            id: e.id.clone(),
            name: e.name.clone(),
        })
        .collect()
}

/// `tine-ui` macro node → engine macro (`tine-ui` must not depend on
/// `tine-engine`; the bridge owns the correspondence).
#[must_use]
pub fn engine_macro(m: UiMacro) -> EngineMacro {
    match m {
        UiMacro::Purity => EngineMacro::Purity,
        UiMacro::Body => EngineMacro::Body,
        UiMacro::Presence => EngineMacro::Presence,
        UiMacro::Leveler => EngineMacro::Leveler,
    }
}

/// The SPECS machine key of a parameter (the `set` pipe wire format).
#[must_use]
pub fn param_key(id: ParamId) -> Option<&'static str> {
    SPECS.iter().find(|s| s.id == id).map(|s| s.key)
}

/// Nominal "Hear Myself" latency badge while no live monitor reports its
/// measured value (D-026: capture block ≈ 10 ms + playback period 10 ms +
/// DSP 21.6 ms; R-18 keeps the playback term nominal).
#[must_use]
pub fn nominal_monitor_latency_ms() -> f32 {
    10.0 + tine_engine::monitor::NOMINAL_PERIOD_MS + tine_engine::monitor::DSP_LATENCY_MS
}

/// User-facing message for a controller error.
#[must_use]
pub fn engine_error_message(e: &EngineError) -> String {
    format!("Tine engine: {e}")
}

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// Endpoint enumeration, health, and change watching (COM in production,
/// fakes in tests).
pub trait EndpointAdmin: Send + Sync {
    /// Lists live capture endpoints (each carries its cable state).
    fn list(&self) -> Result<Vec<CaptureEndpoint>, String>;
    /// Virtual-cable health for `endpoint` (badge input, D-018).
    fn health(&self, endpoint: &str) -> ApoState;
    /// Starts a device watcher and returns its event stream.
    fn watch(&self) -> Result<Receiver<apo_admin::DeviceEvent>, String>;
}

/// Production [`EndpointAdmin`]: `tine_engine::apo_admin` (COM, MTA).
pub struct ComEndpointAdmin;

impl EndpointAdmin for ComEndpointAdmin {
    fn list(&self) -> Result<Vec<CaptureEndpoint>, String> {
        apo_admin::list_capture_endpoints().map_err(|e| e.to_string())
    }

    fn health(&self, endpoint: &str) -> ApoState {
        apo_admin::health(endpoint)
    }

    fn watch(&self) -> Result<Receiver<apo_admin::DeviceEvent>, String> {
        apo_admin::DeviceWatcher::new()
            .map(|(_, rx)| rx)
            .map_err(|e| e.to_string())
    }
}

/// "Hear Myself" service ownership: the T16 `Monitor` in production, a
/// recorder in tests.
pub trait MonitorHost: Send {
    /// Starts or restarts the streams and returns the latency badge (ms).
    fn apply(&mut self, enabled: bool, output: Option<&str>) -> Result<f32, String>;
    /// Stops monitoring (teardown on disable/shutdown).
    fn stop(&mut self);
    /// `true` when the underlying stream reported an error.
    fn has_error(&self) -> bool;
}

/// Production [`MonitorHost`] owning the T16 `Monitor` (cpal playback of the
/// post-DSP [`MonitorFeed`], restart on device change).
pub struct CpalMonitorHost {
    monitor: Option<Monitor>,
    feed: Option<MonitorFeed>,
}

impl CpalMonitorHost {
    /// Creates the idle host (no stream until [`MonitorHost::apply`]).
    #[must_use]
    pub fn new() -> Self {
        Self {
            monitor: None,
            feed: None,
        }
    }

    /// Attaches or replaces the post-DSP [`MonitorFeed`] from the active router.
    pub fn set_feed(&mut self, feed: Option<MonitorFeed>) {
        self.feed = feed;
    }
}

impl Default for CpalMonitorHost {
    fn default() -> Self {
        Self::new()
    }
}

impl MonitorHost for CpalMonitorHost {
    fn apply(&mut self, enabled: bool, output: Option<&str>) -> Result<f32, String> {
        let out = output.map_or_else(DeviceRef::default, DeviceRef::parse);
        if !enabled {
            self.stop();
            return Ok(0.0);
        }
        let Some(feed) = &self.feed else {
            return Err("Hear Myself will start once audio routing is active".to_owned());
        };
        match &mut self.monitor {
            Some(m) => {
                m.restart(feed, &out)
                    .map_err(|e| format!("Hear Myself restart failed: {e}"))?;
            }
            None => {
                self.monitor =
                    Some(Monitor::start(feed, &out).map_err(|e| format!("Hear Myself failed to start: {e}"))?);
            }
        }
        Ok(self
            .monitor
            .as_ref()
            .map_or_else(nominal_monitor_latency_ms, Monitor::latency_ms))
    }

    fn stop(&mut self) {
        if let Some(m) = self.monitor.take() {
            m.stop();
        }
    }

    fn has_error(&self) -> bool {
        self.monitor.as_ref().is_some_and(Monitor::has_error)
    }
}

/// Post-Run inputs the bridge needs for Auto-Tune runs.
pub struct AutotuneHooks {
    /// Shared-memory client of the endpoint being calibrated (bypass
    /// lease). `None` in topologies without a mapping (D-018 production
    /// until routing exists — the intent then surfaces a user message
    /// instead of failing silently).
    pub client: Option<Arc<ShmClient>>,
    /// Capture factory invoked on start (a device in production, a
    /// fixture source in tests).
    pub capture: Arc<dyn Fn() -> CaptureSpec + Send + Sync>,
    /// Calibrator factory.
    pub factory: Arc<dyn CalibratorFactory>,
}

/// Configuration for [`EngineBridge::new`].
pub struct BridgeCfg {
    /// Controller command channel (owns no audio).
    pub handle: ControllerHandle,
    /// Controller event stream (from `Controller::spawn`).
    pub events: Receiver<Event>,
    /// Persistence roots (first-run detection, profile reads).
    pub paths: EnginePaths,
    /// Where the visualizer drains telemetry + the post-DSP tap
    /// (`InProcessSignals` for the headless / virtual-cable topology,
    /// the `ControllerHandle` when an APO mapping is live).
    pub signals: Arc<dyn EngineSignals>,
    /// Endpoint, badge, and watcher source.
    pub admin: Arc<dyn EndpointAdmin>,
    /// "Hear Myself" ownership.
    pub monitor: Arc<Mutex<dyn MonitorHost>>,
    /// Auto-Tune runner inputs.
    pub autotune: AutotuneHooks,
}

/// Adapter that reimplements [`EngineSignals`] through an [`Arc`] so one
/// signal source can drive visualizer restarts.
struct SharedSignals(Arc<dyn EngineSignals>);

impl EngineSignals for SharedSignals {
    fn telemetry(&self) -> AudioTelemetryFrame {
        self.0.telemetry()
    }

    fn drain_tap(&self, buf: &mut [f32]) -> usize {
        self.0.drain_tap(buf)
    }

    fn tap_rate_hz(&self) -> u32 {
        self.0.tap_rate_hz()
    }

    fn heartbeat_ok(&self) -> bool {
        self.0.heartbeat_ok()
    }
}

/// The in-process end-to-end bridge (module docs, host 1).
pub struct EngineBridge {
    handle: ControllerHandle,
    events: Receiver<Event>,
    paths: EnginePaths,
    signals: Arc<dyn EngineSignals>,
    admin: Arc<dyn EndpointAdmin>,
    monitor: Arc<Mutex<dyn MonitorHost>>,
    autotune_hooks: AutotuneHooks,
    model: Option<Arc<AppModel>>,
    params: DspParams,
    endpoints: Vec<CaptureEndpoint>,
    badge: ApoBadge,
    monitor_latency_ms: f32,
    visualizer: Option<Visualizer>,
    frames: Arc<Triple<UiFrame>>,
    visible: bool,
    autotune: Option<AutoTuneHandle>,
    autotune_ui: AutoTuneUi,
    autotune_failure_phase: Option<CalProgress>,
    pending: Option<CalibrationReport>,
    messages: Vec<String>,
    monitor_endpoint: Option<EndpointId>,
}

impl EngineBridge {
    /// Builds the bridge around a spawned controller and its signals.
    #[must_use]
    pub fn new(cfg: BridgeCfg) -> Self {
        Self {
            handle: cfg.handle,
            events: cfg.events,
            paths: cfg.paths,
            signals: cfg.signals,
            admin: cfg.admin,
            monitor: cfg.monitor,
            autotune_hooks: cfg.autotune,
            model: None,
            params: DspParams::DEFAULT,
            endpoints: Vec::new(),
            badge: ApoBadge::NotInstalled,
            monitor_latency_ms: 0.0,
            visualizer: None,
            frames: Arc::new(Triple::new(UiFrame::default())),
            visible: false,
            autotune: None,
            autotune_ui: AutoTuneUi::default(),
            autotune_failure_phase: None,
            pending: None,
            messages: Vec::new(),
            monitor_endpoint: None,
        }
    }

    /// The last controller model (engine-authoritative).
    #[must_use]
    pub fn app_model(&self) -> Option<&Arc<AppModel>> {
        self.model.as_ref()
    }

    /// The last published parameter snapshot (`Event::ParamsResolved`).
    #[must_use]
    pub fn params(&self) -> &DspParams {
        &self.params
    }

    /// User-facing messages queued since the last call (error surfacing:
    /// each of them was also written to the log).
    #[must_use]
    pub fn take_messages(&mut self) -> Vec<String> {
        std::mem::take(&mut self.messages)
    }

    /// `true` while onboarding has not yet been completed.
    #[must_use]
    pub fn first_run(&self) -> bool {
        !persist::load_settings(&self.paths).into_value().onboarding_completed
    }

    /// Refreshes the endpoint list and the D-018 badge from the admin
    /// source; failures are logged and surfaced.
    pub fn refresh_endpoints(&mut self) {
        match self.admin.list() {
            Ok(list) => {
                self.endpoints = list;
                let id = self
                    .model
                    .as_ref()
                    .and_then(|m| m.endpoint.as_ref())
                    .map_or("", |e| e.as_str());
                self.badge = badge_from_health(&self.admin.health(id));
            }
            Err(e) => {
                let msg = format!("Cannot enumerate microphones: {e}");
                tracing::warn!(target: "tine::bridge", "{msg}");
                self.messages.push(msg);
            }
        }
    }

    /// Whether the 60 Hz visualizer pump runs; visibility drives
    /// start/stop so a hidden window costs nothing (spec: frames only
    /// while visible).
    pub fn set_visible(&mut self, visible: bool) {
        self.visible = visible;
        if visible && self.visualizer.is_none() {
            let v = Visualizer::start(SharedSignals(Arc::clone(&self.signals)));
            self.frames = v.frames();
            self.visualizer = Some(v);
            tracing::debug!(target: "tine::bridge", "visualizer started");
        } else if !visible && self.visualizer.is_some() {
            if let Some(mut v) = self.visualizer.take() {
                v.stop();
            }
            tracing::debug!(target: "tine::bridge", "visualizer stopped");
        }
    }

    /// The bridge's visual state, mirroring the spec's lifecycle language.
    #[must_use]
    pub fn visible(&self) -> bool {
        self.visible
    }

    /// `true` while the visualizer pump thread is alive.
    #[must_use]
    pub fn is_visualizing(&self) -> bool {
        self.visualizer.as_ref().is_some_and(Visualizer::is_running)
    }

    /// Pulls the latest visualizer frame, mapped to the UI's [`LiveFrame`]
    /// (latest-wins; the caller is the frames buffer's single reader).
    #[must_use]
    pub fn latest_live(&mut self) -> Option<LiveFrame> {
        let mut f = UiFrame::default();
        if self.frames.latest(&mut f) {
            Some(live_from_frame(&f))
        } else {
            None
        }
    }

    /// Drains controller and Auto-Tune events for up to `timeout` total,
    /// applying them to the bridge caches. Returns the event count.
    pub fn pump(&mut self, timeout: Duration) -> usize {
        let deadline = Instant::now() + timeout;
        let mut count = 0;
        loop {
            let remain = deadline.saturating_duration_since(Instant::now());
            match self.events.recv_timeout(remain) {
                Ok(event) => {
                    self.on_engine_event(event);
                    count += 1;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout | std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
            if Instant::now() >= deadline {
                break;
            }
        }
        while let Some(handle) = self.autotune.as_mut() {
            match handle.events().try_recv() {
                Ok(event) => {
                    self.on_autotune_event(event);
                    count += 1;
                }
                Err(std::sync::mpsc::TryRecvError::Empty | std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        count
    }

    /// Applies one controller event to the caches.
    fn on_engine_event(&mut self, event: Event) {
        match event {
            Event::ModelChanged(m) => {
                // T16 contract: a live monitor follows its endpoint.
                if m.monitor.enabled {
                    let endpoint = m.endpoint.clone();
                    if endpoint != self.monitor_endpoint {
                        self.monitor_endpoint.clone_from(&endpoint);
                        self.monitor_apply(true, m.monitor.output_device.as_deref());
                    }
                } else {
                    self.monitor_endpoint = None;
                }
                self.model = Some(m);
            }
            Event::ParamsResolved(p) => self.params = p,
            Event::ApoLink(link) => {
                tracing::debug!(target: "tine::bridge", link = ?link, "link state changed");
            }
            Event::Error(e) => {
                let msg = engine_error_message(&e);
                tracing::error!(target: "tine::bridge", error = ?e, "engine error");
                self.messages.push(msg);
            }
        }
    }

    /// Applies one Auto-Tune runner event to the wizard state (30 Hz
    /// progress; phase changes immediate; exactly one terminal event).
    fn on_autotune_event(&mut self, event: AutoTuneEvent) {
        match event {
            AutoTuneEvent::Started { rate_hz } => {
                tracing::info!(target: "tine::bridge", rate_hz, "auto-tune capture started");
            }
            AutoTuneEvent::Progress(p) => {
                if p.phase == CalPhase::Failed {
                    // The typed failure arrives with `Done`; hold the phase
                    // payload until the card can render it.
                    self.autotune_failure_phase = Some(p);
                } else {
                    self.autotune_ui = autotune_from_progress(&p, None);
                }
            }
            AutoTuneEvent::Done(outcome) => {
                self.autotune = None;
                self.autotune_failure_phase = None;
                match outcome {
                    AutoTuneOutcome::Applied(report) => {
                        // Held for the results card; the Apply / Discard
                        // intents decide its fate.
                        let done = CalProgress {
                            phase: CalPhase::Done,
                            phase_progress: 1.0,
                            live_level_db: -90.0,
                            voiced_ms: 0,
                        };
                        self.autotune_ui = autotune_from_progress(&done, None);
                        self.pending = Some(*report);
                    }
                    AutoTuneOutcome::Failed(failure) => {
                        let failed = self.autotune_failure_phase.take().unwrap_or(CalProgress {
                            phase: CalPhase::Failed,
                            phase_progress: 1.0,
                            live_level_db: -90.0,
                            voiced_ms: 0,
                        });
                        self.autotune_ui = autotune_from_progress(&failed, Some(failure));
                        tracing::info!(target: "tine::bridge", failure = ?failure, "auto-tune rejected the capture");
                    }
                    AutoTuneOutcome::Error(e) => {
                        self.autotune_ui = AutoTuneUi::default();
                        let msg = format!("Auto-Tune failed: {e}");
                        tracing::warn!(target: "tine::bridge", "{msg}");
                        self.messages.push(msg);
                    }
                }
                self.autotune_failure_phase = None;
            }
        }
    }

    /// Builds the full `tine-ui` view model from the engine caches
    /// (endpoints and badge need [`EngineBridge::refresh_endpoints`] to
    /// have run; the values are stale-but-safe before that).
    #[must_use]
    pub fn ui_model(&self) -> UiModel {
        let model = self.model.as_deref();
        let profile = model.map_or_else(|| EndpointProfile::new("", Archetype::default()), |m| m.profile.clone());
        let endpoint_id = model
            .and_then(|m| m.endpoint.as_ref())
            .map_or_else(String::new, |e| e.as_str().to_owned());
        let selected = self.endpoints.iter().position(|e| e.id == endpoint_id);
        let calibration = self.pending.as_ref().map(calibration_from_report);
        let pending_calibration = self.pending.as_ref().map(|r| r.calibration);
        let active_calibration = model.and_then(|m| m.profile.calibration);
        UiModel {
            endpoints: endpoint_rows(&self.endpoints),
            selected,
            apo: self.badge.clone(),
            archetype: profile.archetype,
            macros: profile.macros,
            pins: profile.pins,
            params: self.params,
            bypass: model.is_some_and(|m| m.bypass),
            muted: crate::services::is_muted(),
            denoise_engine: DenoiseEngine::default(),
            park_when_unused: true,
            monitor: MonitorUi {
                enabled: model.is_some_and(|m| m.monitor.enabled),
                device_name: model.and_then(|m| m.monitor.output_device.clone()),
                available_devices: Vec::new(),
                estimated_latency_ms: self.monitor_latency_ms,
            },
            calibration,
            pending_calibration,
            active_calibration,
            autotune: self.autotune_ui.clone(),
            mic_check: MicCheckStep::Idle,
            first_run: self.first_run(),
            status_banner: None,
            raw_mic_apps: Vec::new(),
        }
    }

    fn preview_params_for(
        archetype: Archetype,
        cal: &Calibration,
        style: tine_dsp::calibrator::VoiceStyle,
        keep_pins: bool,
        macros: MacroPositions,
        mut pins: tine_protocol::presets::PinSet,
        current: &DspParams,
    ) -> DspParams {
        let macros = tine_dsp::calibrator::recommend_macros(cal, archetype, style, macros);
        if !keep_pins {
            for id in tine_dsp::calibrator::CHANGED_PARAMS {
                pins.unpin(id);
            }
            if style != tine_dsp::calibrator::VoiceStyle::KeepCurrent {
                for m in tine_dsp::macros::Macro::ALL {
                    for &id in tine_dsp::macros::MacroMapper::drives(m) {
                        pins.unpin(id);
                    }
                }
            }
        }
        tine_dsp::macros::MacroMapper::apply(builtin_preset(archetype), Some(cal), macros, pins, current).clamped()
    }

    /// Dispatches one user intent (the in-process `IntentSink` mapping).
    pub fn dispatch(&mut self, intent: Intent) {
        match intent {
            Intent::SelectEndpoint(id) => {
                if self.endpoints.iter().any(|e| e.id == id) {
                    self.send(Command::SelectEndpoint(EndpointId::new(id)));
                } else {
                    tracing::warn!(target: "tine::bridge", endpoint_id = %id, "selected microphone not found in endpoints");
                    self.surface("Selected microphone disappeared before switching");
                }
            }
            Intent::SetArchetype(a) => self.send(Command::SetArchetype(a)),
            Intent::SetDenoiseEngine(engine) => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.denoise_engine = engine;
                if let Err(e) = persist::save_settings(&settings, &self.paths) {
                    self.surface(&format!("Failed to save denoiser engine: {e}"));
                }
            }
            Intent::SetParkWhenUnused(enabled) => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                settings.park_when_unused = enabled;
                if let Err(e) = persist::save_settings(&settings, &self.paths) {
                    self.surface(&format!("Failed to save keep-warm setting: {e}"));
                }
                crate::services::set_park_when_unused(enabled);
            }
            Intent::SetMacro(m, v) => self.send(Command::SetMacro(engine_macro(m), v)),
            Intent::SetParam(id, v) => self.send(Command::SetParam(id, v)),
            Intent::Unpin(id) => self.send(Command::Unpin(id)),
            Intent::UnpinAll => self.send(Command::UnpinAll),
            Intent::ToggleBypass => {
                let next = self.model.as_ref().is_none_or(|m| !m.bypass);
                self.send(Command::SetBypass(next));
            }
            Intent::SetMuted(muted) => {
                let _ = crate::services::set_muted(muted);
            }
            Intent::ToggleMonitor => {
                let on = self.model.as_ref().is_none_or(|m| !m.monitor.enabled);
                let device = self.model.as_ref().and_then(|m| m.monitor.output_device.clone());
                self.monitor_set(on, device);
            }
            Intent::PickMonitorDevice(name) => self.monitor_set(true, Some(name)),
            Intent::StartAutoTune => self.start_autotune(),
            Intent::CancelAutoTune => {
                if let Some(h) = self.autotune.as_mut() {
                    h.cancel();
                }
                if let Some(resolved) = self.model.as_deref().map(resolve) {
                    self.params = resolved;
                }
            }
            Intent::PreviewCalibration {
                style,
                keep_pins,
                audition_raw: _,
            } => {
                if let (Some(report), Some(model)) = (self.pending.as_ref(), self.model.as_deref()) {
                    let mut resolved = Self::preview_params_for(
                        model.profile.archetype,
                        &report.calibration,
                        style,
                        keep_pins,
                        model.profile.macros,
                        model.profile.pins,
                        &self.params,
                    );
                    resolved.bypass = model.bypass;
                    self.params = resolved;
                }
            }
            Intent::ApplyCalibration { keep_pins } => {
                if let Some(report) = self.pending.take() {
                    self.autotune_ui = AutoTuneUi::default();
                    let keep = if keep_pins { KeepPins::Keep } else { KeepPins::Clear };
                    self.send(Command::ApplyCalibration(report.calibration, keep));
                }
            }
            Intent::DiscardCalibration => {
                self.pending = None;
                self.autotune_ui = AutoTuneUi::default();
                if let Some(resolved) = self.model.as_deref().map(resolve) {
                    self.params = resolved;
                }
                tracing::info!(target: "tine::bridge", "auto-tune results discarded");
            }
            Intent::InstallApo | Intent::RepairApo => {
                // D-018: "install" = a present virtual cable; there is no
                // APO to register. The re-check refreshes the badge and
                // tells the user what is missing.
                self.refresh_endpoints();
                if matches!(self.badge, ApoBadge::Active) {
                    tracing::info!(target: "tine::bridge", "virtual cable healthy — nothing to repair");
                } else {
                    self.surface("Tine needs a virtual audio cable: install VB-Audio Virtual Cable, then use Repair.");
                }
            }
            Intent::SetProDrawerOpen(open) => {
                let mut settings = persist::load_settings(&self.paths).into_value();
                if settings.pro_drawer_open != open {
                    settings.pro_drawer_open = open;
                    if let Err(e) = persist::save_settings(&settings, &self.paths) {
                        self.surface(&format!("Failed to save Pro drawer state: {e}"));
                    }
                }
            }
            Intent::OpenSettings
            | Intent::CloseWindow
            | Intent::StartMicCheck
            | Intent::SetMicCheckMode(_)
            | Intent::StopMicCheck => {
                tracing::debug!(target: "tine::bridge", intent = ?intent, "local-only intent");
            }
        }
    }

    /// Queues a user-visible message (already logged by the caller).
    fn surface(&mut self, msg: &str) {
        tracing::warn!(target: "tine::bridge", "{msg}");
        self.messages.push(msg.to_owned());
    }

    /// Sends a command; a dead controller surfaces an error.
    fn send(&mut self, cmd: Command) {
        if let Err(e) = self.handle.send(cmd) {
            self.surface(&format!("Tine engine unreachable: {e}"));
        }
    }

    /// Applies monitor state: drives the [`MonitorHost`] and persists the
    /// setting through the controller (single source of truth).
    fn monitor_set(&mut self, enabled: bool, device: Option<String>) {
        self.monitor_apply(enabled, device.as_deref());
        self.monitor_endpoint = enabled
            .then(|| self.model.as_ref().and_then(|m| m.endpoint.clone()))
            .flatten();
        self.send(Command::SetMonitor(MonitorSettings {
            enabled,
            output_device: device,
        }));
    }

    /// Drives the [`MonitorHost`] and records the latency badge.
    fn monitor_apply(&mut self, enabled: bool, device: Option<&str>) {
        let mut host = match self.monitor.lock() {
            Ok(h) => h,
            Err(poisoned) => poisoned.into_inner(),
        };
        match host.apply(enabled, device) {
            Ok(latency) => self.monitor_latency_ms = if enabled { latency } else { 0.0 },
            Err(msg) => {
                tracing::error!(target: "tine::bridge", "{msg}");
                self.messages.push(msg);
                self.monitor_latency_ms = 0.0;
            }
        }
        drop(host);
        if !enabled {
            self.monitor_endpoint = None;
        }
    }

    /// Starts the 8-second wizard through the T16 runner.
    fn start_autotune(&mut self) {
        if self.autotune.is_some() {
            tracing::info!(target: "tine::bridge", "auto-tune already running");
            return;
        }
        let Some(client) = self.autotune_hooks.client.clone() else {
            self.surface(
                "Auto-Tune needs a live processing link: start Tine's audio routing first (virtual cable, D-018).",
            );
            return;
        };
        let Some(model) = self.model.clone() else {
            self.surface("Auto-Tune needs a selected microphone");
            return;
        };
        let cfg = AutoTuneConfig {
            client,
            controller: None, // the results card applies on the Apply intent
            capture: (self.autotune_hooks.capture)(),
            factory: Arc::clone(&self.autotune_hooks.factory),
            preset: *builtin_preset(model.profile.archetype),
            macros: model.profile.macros,
            pins: model.profile.pins,
            current: resolve(&model),
            keep_pins: KeepPins::Keep,
        };
        self.autotune_ui = AutoTuneUi::default();
        self.autotune = Some(AutoTuneRunner::start(cfg));
        tracing::info!(target: "tine::bridge", "auto-tune wizard started");
    }

    /// Whether an Auto-Tune run is currently in flight.
    #[must_use]
    pub fn autotune_running(&self) -> bool {
        self.autotune.is_some()
    }

    /// The pending report awaiting the Apply / Discard intents.
    #[must_use]
    pub fn pending_calibration(&self) -> Option<&CalibrationReport> {
        self.pending.as_ref()
    }
}

impl Drop for EngineBridge {
    fn drop(&mut self) {
        if let Some(mut v) = self.visualizer.take() {
            v.stop();
        }
        if let Some(mut h) = self.autotune.take() {
            h.cancel();
        }
        if let Ok(mut host) = self.monitor.lock() {
            host.stop();
        }
    }
}

// ---------------------------------------------------------------------------
// `--ui` child process host
// ---------------------------------------------------------------------------

/// Whether the window currently shows (drives the meters polling).
static WINDOW_VISIBLE: AtomicBool = AtomicBool::new(true);

/// Whether a meters update is still queued for the UI thread (coalescing:
/// an update in flight drops the older frame).
static METERS_QUEUED: AtomicBool = AtomicBool::new(false);

/// Whether onboarding completion/dismissal has already been persisted to
/// `settings.json` for this child session.
static ONBOARDING_PERSISTED: AtomicBool = AtomicBool::new(false);

/// Persists onboarding dismissal/completion to the parent so `settings.json`
/// exists on disk and `first_run` is `false` on future opens.
fn persist_onboarding_async() {
    if ONBOARDING_PERSISTED.swap(true, Ordering::AcqRel) {
        return;
    }
    if std::thread::Builder::new()
        .name("tine-ui-onboarding".to_owned())
        .spawn(|| {
            let pipe = single_instance::pipe_name_for_user();
            match single_instance::send_request(&pipe, &PipeRequest::OnboardingDone) {
                Ok(PipeResponse { ok: true, .. }) => {
                    tracing::debug!(target: "tine::ui", "onboarding dismissal persisted");
                }
                Ok(resp) => {
                    ONBOARDING_PERSISTED.store(false, Ordering::Release);
                    let msg = resp.error.unwrap_or_else(|| "onboarding persist failed".to_owned());
                    tracing::warn!(target: "tine::ui", "{msg}");
                }
                Err(e) => {
                    ONBOARDING_PERSISTED.store(false, Ordering::Release);
                    tracing::warn!(target: "tine::ui", "onboarding persist unreachable: {e}");
                }
            }
        })
        .is_err()
    {
        ONBOARDING_PERSISTED.store(false, Ordering::Release);
        tracing::error!(target: "tine::ui", "onboarding persist worker spawn failed");
    }
}

/// Rune visibility hook (minimized/covered/hidden ⇄ shown).
pub fn on_visibility(s: &mut AppState, visible: bool, _tx: &mut rune::Tx) {
    WINDOW_VISIBLE.store(visible, Ordering::Release);
    if !visible && s.model.first_run && s.ui.onboarding_dismissed {
        s.model.first_run = false;
        persist_onboarding_async();
    }
}

/// Window key handler: delegate to `tine-ui` (A/B `Space`), dismiss
/// onboarding on `Escape` when active, or close on `Escape` / `⌘W` / `⌘Q`
/// (tray app: closing hides, audio continues).
fn child_on_key(s: &mut AppState, ev: &rune::KeyEvent, tx: &mut rune::Tx) -> bool {
    if tine_ui::on_key(s, ev, tx) {
        return true;
    }
    match ev.key {
        rune::Key::Escape if s.model.first_run && !s.ui.onboarding_dismissed => {
            s.ui.onboarding_dismissed = true;
            s.model.first_run = false;
            persist_onboarding_async();
            true
        }
        rune::Key::Command('w' | 'q') | rune::Key::Escape => {
            tx.close_window();
            true
        }
        _ => false,
    }
}

/// Live-frame polling period (spec: ≤ 60 Hz).
const METERS_PERIOD: Duration = Duration::from_millis(16);

/// Model-refresh period (the pipe equivalent of the model-event stream;
/// the parent reacts to control changes well within this window).
const STATUS_PERIOD: Duration = Duration::from_millis(2_000);

/// Cooldown after a local UI edit before `child_refresh_model` overwrites
/// profile-derived fields from disk (exceeds the controller's 500 ms
/// trailing debounce so optimistic edits are never reverted).
const EDIT_COOLDOWN: Duration = Duration::from_millis(1_500);

/// Intent sink of the child process: forwards intents to the parent's
/// command pipe (the authoritative controller lives there, D-017).
pub struct ChildSink {
    tx: Sender<Intent>,
}

impl IntentSink for ChildSink {
    fn send(&self, i: Intent) {
        if self.tx.send(i.clone()).is_err() {
            tracing::error!(target: "tine::ui", intent = ?i, "bridge worker gone; intent dropped");
        }
    }
}

/// Cached status fields the child needs for toggle intents (the pipe
/// request needs the *next* state, which the optimistic UI flipped) plus
/// the endpoint ids the picker indexes into.
struct ChildModel {
    bypass: bool,
    muted: bool,
    denoise_engine: DenoiseEngine,
    park_when_unused: bool,
    monitor_on: bool,
    archetype: Archetype,
    endpoint_id: String,
    endpoints: Vec<String>,
    output_devices: Vec<String>,
    badge: ApoBadge,
    last_device_refresh: Option<Instant>,
    last_local_edit: Option<Instant>,
}

impl ChildModel {
    fn new() -> Self {
        Self {
            bypass: false,
            muted: false,
            denoise_engine: DenoiseEngine::default(),
            park_when_unused: true,
            monitor_on: false,
            archetype: Archetype::default(),
            endpoint_id: String::new(),
            endpoints: apo_admin::list_capture_endpoints()
                .map(|list| list.into_iter().map(|e| e.id).collect::<Vec<_>>())
                .unwrap_or_default(),
            output_devices: list_outputs()
                .map(|list| list.into_iter().map(|d| d.name).collect::<Vec<_>>())
                .unwrap_or_default(),
            badge: ApoBadge::Inactive,
            last_device_refresh: None,
            last_local_edit: None,
        }
    }
}

/// Loads one `status` snapshot from the running parent.
fn fetch_status() -> Result<Value, String> {
    let name = single_instance::pipe_name_for_user();
    match single_instance::send_request(&name, &PipeRequest::Status) {
        Ok(PipeResponse {
            ok: true,
            data: Some(data),
            ..
        }) => Ok(data),
        Ok(resp) => Err(resp.error.unwrap_or_else(|| "unexpected reply".to_owned())),
        Err(e) => Err(e),
    }
}

/// Builds the child's initial UI model: endpoint enumeration and D-018
/// badge through `apo_admin`, first-run detection from the settings file,
/// the persisted profile (Pro params + pins), and the parent's `status`
/// snapshot (macros, bypass, monitor).
fn initial_ui_model() -> UiModel {
    let rows = apo_admin::list_capture_endpoints()
        .map(|list| endpoint_rows(&list))
        .unwrap_or_default();
    let badge = badge_from_health(&apo_admin::health(""));
    let status = fetch_status().ok();

    let mut first_run = true;
    let mut profile = EndpointProfile::new("", Archetype::default());
    let mut monitor = MonitorUi::default();
    let mut endpoint_id = String::new();
    let mut denoise_engine = DenoiseEngine::default();
    let mut park_when_unused = true;
    if let Ok(paths) = EnginePaths::system() {
        let settings = persist::load_settings(&paths).into_value();
        first_run = !settings.onboarding_completed;
        endpoint_id = settings.selected_endpoint_id.unwrap_or_default();
        denoise_engine = settings.denoise_engine;
        park_when_unused = settings.park_when_unused;
        monitor.enabled = settings.monitor_enabled;
        monitor.device_name = settings.monitor_device_id;
        profile = persist::load_profile(&endpoint_id, &paths).into_value();
    }
    if let Some(s) = &status {
        if let Some(m) = s.get("macros").and_then(Value::as_object) {
            let get = |k: &str| m.get(k).and_then(Value::as_f64).map(|v| v as f32);
            profile.macros.purity = get("purity").unwrap_or(profile.macros.purity).clamp(0.0, 1.0);
            profile.macros.body = get("body").unwrap_or(profile.macros.body).clamp(0.0, 1.0);
            profile.macros.presence = get("presence").unwrap_or(profile.macros.presence).clamp(0.0, 1.0);
            profile.macros.leveler = get("leveler").unwrap_or(profile.macros.leveler).clamp(0.0, 1.0);
        }
        endpoint_id = s
            .get("endpoint_id")
            .and_then(Value::as_str)
            .filter(|t| !t.is_empty())
            .map(str::to_owned)
            .unwrap_or(endpoint_id);
        monitor.enabled = s.get("monitor").and_then(Value::as_bool).unwrap_or(monitor.enabled);
        if let Some(engine) = s
            .get("denoise_engine")
            .and_then(Value::as_str)
            .and_then(DenoiseEngine::from_name)
        {
            denoise_engine = engine;
        }
        park_when_unused = s
            .get("park_when_unused")
            .and_then(Value::as_bool)
            .unwrap_or(park_when_unused);
    }
    let bypass = status
        .as_ref()
        .and_then(|s| s.get("bypass"))
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let muted = status
        .as_ref()
        .and_then(|s| s.get("muted"))
        .and_then(Value::as_bool)
        .unwrap_or_default();
    let app_model = AppModel {
        endpoint: (!endpoint_id.is_empty()).then(|| EndpointId::new(endpoint_id.clone())),
        bypass,
        monitor: MonitorSettings {
            enabled: monitor.enabled,
            output_device: monitor.device_name.clone(),
        },
        profile: profile.clone(),
    };
    let params = resolve(&app_model);
    monitor.estimated_latency_ms = if monitor.enabled {
        nominal_monitor_latency_ms()
    } else {
        0.0
    };
    let selected = rows
        .iter()
        .position(|e| e.id == endpoint_id)
        .or((!rows.is_empty()).then_some(0));
    UiModel {
        endpoints: rows,
        selected,
        apo: badge,
        archetype: profile.archetype,
        macros: profile.macros,
        pins: profile.pins,
        params,
        bypass,
        muted,
        denoise_engine,
        park_when_unused,
        monitor,
        calibration: None,
        pending_calibration: None,
        active_calibration: profile.calibration,
        autotune: AutoTuneUi::default(),
        mic_check: MicCheckStep::Idle,
        first_run,
        status_banner: None,
        raw_mic_apps: status
            .as_ref()
            .and_then(|s| s.get("raw_mic_apps"))
            .and_then(Value::as_array)
            .map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect())
            .unwrap_or_default(),
    }
}

/// Resource id of the app's icon group (`assets/brand/tine.ico`, linked by `build.rs`).
pub const APP_ICON_ID: u16 = 1;

/// Runs the `--ui` child (called from `main` for `tine --ui`): the real
/// `tine-ui` window wired to the parent's control plane through the
/// command pipe, with local endpoint watching, the D-018 badge, and
/// first-run onboarding detection.
pub fn run_ui_child() {
    if let Ok(paths) = EnginePaths::system() {
        let _ = crate::logging::init_child_log(&paths.logs_dir(), "tine-ui.log");
    }
    tracing::info!(target: "tine::ui", "UI child starting");

    let model = initial_ui_model();
    let initial_seed = ChildInit {
        bypass: model.bypass,
        muted: model.muted,
        denoise_engine: model.denoise_engine,
        park_when_unused: model.park_when_unused,
        monitor_on: model.monitor.enabled,
        archetype: model.archetype,
        endpoint_id: model
            .selected
            .and_then(|idx| model.endpoints.get(idx))
            .map(|e| e.id.clone())
            .unwrap_or_default(),
    };
    ONBOARDING_PERSISTED.store(!model.first_run, Ordering::Release);
    if model.first_run {
        tracing::info!(target: "tine::ui", "first run detected: onboarding overlay shown");
    }
    let mut state = AppState::from_model(model);
    if let Ok(paths) = EnginePaths::system() {
        state.ui.pro_drawer_open = persist::load_settings(&paths).into_value().pro_drawer_open;
    }

    let (tx_intents, rx_intents) = channel::<Intent>();
    let spec = {
        let mut spec = tine_ui::app_spec(ChildSink { tx: tx_intents }, state);
        spec.on_key = Some(child_on_key);
        spec
    };
    // Title bar, taskbar and Alt+Tab icon from the exe's own icon group (linked by build.rs).
    #[cfg(windows)]
    rune::set_app_icon_resource(APP_ICON_ID);
    let app = rune::App::new(spec)
        .args(std::env::args_os().skip(2).map(|a| a.to_string_lossy().into_owned()))
        .on_visibility(on_visibility)
        .on_window(|_| tracing::debug!(target: "tine::ui", "window opened"));
    let handle = app.handle();

    let worker_handle = handle.clone();
    if std::thread::Builder::new()
        .name("tine-ui-bridge".to_owned())
        .spawn(move || {
            child_worker(&worker_handle, &rx_intents, initial_seed);
        })
        .is_err()
    {
        tracing::error!(target: "tine::ui", "bridge worker spawn failed");
    }
    spawn_device_watcher(&handle);

    app.run();
}

/// Drains all currently queued intents and coalesces consecutive slider drags
/// (`SetMacro` / `SetParam`) to their latest value so dragging a control never
/// builds a multi-second pipe backlog.
fn coalesce_intents(first: Intent, rx: &Receiver<Intent>) -> Vec<Intent> {
    let mut batch = vec![first];
    while let Ok(next) = rx.try_recv() {
        match next {
            Intent::SetMacro(m, v) => {
                if let Some(existing) = batch
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Intent::SetMacro(em, _) if *em == m))
                {
                    *existing = Intent::SetMacro(m, v);
                    continue;
                }
            }
            Intent::SetParam(id, v) => {
                if let Some(existing) = batch
                    .iter_mut()
                    .rev()
                    .find(|i| matches!(i, Intent::SetParam(eid, _) if *eid == id))
                {
                    *existing = Intent::SetParam(id, v);
                    continue;
                }
            }
            _ => {}
        }
        batch.push(next);
    }
    batch
}

fn mic_check_step_from_wire(data: &Value) -> Option<(MicCheckStep, bool)> {
    match data.get("flow").and_then(Value::as_str)? {
        "idle" => Some((MicCheckStep::Idle, false)),
        "recording" => Some((
            MicCheckStep::Recording {
                progress: data.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                level_db: data.get("level_db").and_then(Value::as_f64).unwrap_or(-60.0) as f32,
            },
            true,
        )),
        "playing" => Some((
            MicCheckStep::Playing {
                raw: data.get("raw").and_then(Value::as_bool).unwrap_or(false),
                progress: data.get("progress").and_then(Value::as_f64).unwrap_or(0.0) as f32,
                duration_s: data.get("duration_s").and_then(Value::as_f64).unwrap_or(5.0) as f32,
            },
            true,
        )),
        "failed" => Some((
            MicCheckStep::Failed(
                data.get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("Mic Check failed.")
                    .to_owned(),
            ),
            false,
        )),
        _ => None,
    }
}

struct ChildInit {
    bypass: bool,
    muted: bool,
    denoise_engine: DenoiseEngine,
    park_when_unused: bool,
    monitor_on: bool,
    archetype: Archetype,
    endpoint_id: String,
}

/// The child's bridge worker: intent dispatch to the pipe (admin requests
/// run here, on this worker thread), the Auto-Tune wizard's ~30 Hz
/// progress poll while a run is in flight, plus timed `status`/`meters`
/// polls that post coalesced, visibility-gated updates to the Rune state.
fn child_worker(handle: &rune::AppHandle<AppState>, rx_intents: &Receiver<Intent>, init: ChildInit) {
    let pipe = single_instance::pipe_name_for_user();
    let mut model = ChildModel::new();
    model.bypass = init.bypass;
    model.muted = init.muted;
    model.denoise_engine = init.denoise_engine;
    model.park_when_unused = init.park_when_unused;
    model.monitor_on = init.monitor_on;
    model.archetype = init.archetype;
    model.endpoint_id = init.endpoint_id;
    let mut autotune_active = false;
    let mut mic_check_active = false;
    let mut wizard_smooth_db: Option<f32> = None;
    let epoch = Instant::now();
    let mut last_status = epoch.checked_sub(STATUS_PERIOD).unwrap_or(epoch);
    let mut last_meters = epoch.checked_sub(METERS_PERIOD).unwrap_or(epoch);
    let mut last_wizard = epoch.checked_sub(WIZARD_PERIOD).unwrap_or(epoch);
    let mut last_mic_check = epoch.checked_sub(Duration::from_millis(50)).unwrap_or(epoch);
    loop {
        // 1. Pending intents take priority (coalesced so rapid slider drags stay instant).
        let (woke, started_mc, stopped_mc) = match rx_intents.recv_timeout(METERS_PERIOD) {
            Ok(first) => {
                let mut started = false;
                let mut mc_start = false;
                let mut mc_stop = false;
                for intent in coalesce_intents(first, rx_intents) {
                    match intent {
                        Intent::StartMicCheck => mc_start = true,
                        Intent::StopMicCheck => {
                            mc_start = false;
                            mc_stop = true;
                        }
                        _ => {}
                    }
                    if child_dispatch(handle, &pipe, &mut model, &intent) {
                        started = true;
                    }
                }
                (started, mc_start, mc_stop)
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => (false, false, false),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return,
        };
        if woke {
            autotune_active = true;
            wizard_smooth_db = None;
            last_wizard = epoch.checked_sub(WIZARD_PERIOD).unwrap_or(epoch);
        }
        if started_mc {
            mic_check_active = true;
            last_mic_check = epoch.checked_sub(Duration::from_millis(50)).unwrap_or(epoch);
        } else if stopped_mc {
            mic_check_active = false;
        }
        let now = Instant::now();
        if mic_check_active && now.duration_since(last_mic_check) >= Duration::from_millis(50) {
            last_mic_check = now;
            if let Ok(PipeResponse {
                ok: true,
                data: Some(data),
                ..
            }) = single_instance::send_request(&pipe, &PipeRequest::MicCheckQuery)
            {
                if let Some((step, keep_polling)) = mic_check_step_from_wire(&data) {
                    mic_check_active = keep_polling;
                    handle.update_if(move |s, _| {
                        if s.model.mic_check == step {
                            return false;
                        }
                        s.model.mic_check = step;
                        true
                    });
                }
            } else {
                mic_check_active = false;
            }
        }
        // 2. Wizard progress at ~30 Hz while the parent calibrates (the
        //    poll stops on the first terminal frame).
        if autotune_active && now.duration_since(last_wizard) >= WIZARD_PERIOD {
            last_wizard = now;
            if let Ok(PipeResponse {
                ok: true,
                data: Some(data),
                ..
            }) = single_instance::send_request(&pipe, &PipeRequest::CalibrateQuery)
            {
                if let Some((mut ui, calibration, terminal)) = autotune_ui_from_state(&data) {
                    let pending_cal = pending_calibration_from_state(&data, model.archetype);
                    if matches!(
                        ui.step,
                        AutoTuneStep::Prepare { .. } | AutoTuneStep::Silence { .. } | AutoTuneStep::Speak { .. }
                    ) {
                        let raw_db = ui.level_db;
                        let smoothed = match wizard_smooth_db {
                            None => raw_db,
                            Some(prev) => {
                                let alpha = if raw_db > prev { 0.45 } else { 0.16 };
                                prev + alpha * (raw_db - prev)
                            }
                        };
                        wizard_smooth_db = Some(smoothed);
                        ui.level_db = smoothed;
                    }
                    if terminal {
                        autotune_active = false;
                        wizard_smooth_db = None;
                    }
                    handle.update_if(move |s, tx| {
                        s.model.autotune = ui;
                        s.model.calibration = calibration;
                        s.model.pending_calibration = pending_cal;
                        if let Some(cal) = pending_cal {
                            let mut resolved = EngineBridge::preview_params_for(
                                s.model.archetype,
                                &cal,
                                s.ui.autotune_style,
                                s.ui.autotune_keep_pins,
                                s.model.macros,
                                s.model.pins,
                                &s.model.params,
                            );
                            resolved.bypass = s.model.bypass;
                            s.model.params = resolved;
                        }
                        tx.animated();
                        true
                    });
                }
            } else {
                // Parent unreachable or refused: stop the wizard, the
                // last rendered step stays on screen.
                tracing::warn!(target: "tine::ui", "auto-tune progress unavailable; wizard poll stopped");
                autotune_active = false;
                wizard_smooth_db = None;
            }
        }
        // 3. Meters at ≤ 60 Hz while the window is visible, coalesced: a
        //    queued-but-undrained update drops the older frame.
        if WINDOW_VISIBLE.load(Ordering::Acquire)
            && now.duration_since(last_meters) >= METERS_PERIOD
            && !METERS_QUEUED.load(Ordering::Acquire)
        {
            last_meters = now;
            if let Ok(resp) = single_instance::send_request(&pipe, &PipeRequest::Meters)
                && let Some(live) = resp.data.as_ref().and_then(live_from_meters)
                && !METERS_QUEUED.swap(true, Ordering::AcqRel)
            {
                handle.update_if(move |s, _| {
                    METERS_QUEUED.store(false, Ordering::Release);
                    if s.live == live {
                        return false;
                    }
                    s.live = live;
                    true
                });
            }
        }
        // 4. Model refresh at a slow snapshot cadence (only while visible).
        if WINDOW_VISIBLE.load(Ordering::Acquire) && now.duration_since(last_status) >= STATUS_PERIOD {
            last_status = now;
            child_refresh_model(handle, &mut model);
        }
    }
}

/// Resets the wizard and results card in the Rune state (the optimistic
/// close after Apply / Discard; the render thread applies it when idle).
fn close_wizard(handle: &rune::AppHandle<AppState>) {
    let ui = AutoTuneUi::default();
    handle.update_if(move |s, tx| {
        s.model.autotune = ui;
        s.model.calibration = None;
        s.model.pending_calibration = None;
        tx.animated();
        true
    });
}

/// Forwards one intent to the parent's pipe, or handles it locally.
/// Returns `true` when the Auto-Tune wizard poll should start (the
/// parent accepted a calibration start).
fn child_dispatch(handle: &rune::AppHandle<AppState>, pipe: &str, model: &mut ChildModel, intent: &Intent) -> bool {
    // Admin intents run on their own worker thread (never the UI thread)
    // and post the refreshed badge back through `AppHandle::update`.
    if let Intent::InstallApo | Intent::RepairApo = intent {
        spawn_admin_check(handle.clone());
        return false;
    }
    if let Intent::ApplyCalibration { keep_pins } = intent {
        // Close the wizard + results card locally on the optimistic path;
        // the parent's apply lands in the next model refresh.
        tracing::debug!(target: "tine::ui", keep_pins, "auto-tune results applied via parent");
        close_wizard(handle);
    } else if let Intent::DiscardCalibration = intent {
        tracing::info!(target: "tine::ui", "auto-tune results discarded");
        close_wizard(handle);
    } else if let Intent::CancelAutoTune = intent {
        tracing::info!(target: "tine::ui", "auto-tune cancelled / closed");
        close_wizard(handle);
    }
    let req = match intent {
        Intent::SelectEndpoint(id) => {
            if model.endpoints.iter().any(|e| e == id) {
                Some(PipeRequest::Endpoint { id: id.clone() })
            } else {
                tracing::warn!(target: "tine::ui", endpoint_id = %id, "endpoint id not in picker rows; not routed");
                None
            }
        }
        Intent::SetArchetype(a) => Some(PipeRequest::Archetype {
            name: a.id().to_owned(),
        }),
        Intent::SetDenoiseEngine(engine) => Some(PipeRequest::SetDenoiseEngine {
            engine: engine.as_str().to_owned(),
        }),
        Intent::SetParkWhenUnused(enabled) => Some(PipeRequest::SetParkWhenUnused { enabled: *enabled }),
        Intent::SetMacro(m, v) => Some(PipeRequest::Macro {
            name: m.label().to_lowercase(),
            value: *v,
        }),
        Intent::SetParam(id, v) => param_key(*id).map(|key| PipeRequest::Set {
            key: key.to_owned(),
            value: *v,
        }),
        Intent::Unpin(id) => param_key(*id).map(|key| PipeRequest::Unpin { key: key.to_owned() }),
        Intent::UnpinAll => Some(PipeRequest::UnpinAll),
        Intent::ToggleBypass => Some(PipeRequest::Bypass { on: !model.bypass }),
        Intent::SetMuted(muted) => Some(PipeRequest::Mute { on: Some(*muted) }),
        Intent::ToggleMonitor => Some(PipeRequest::Monitor {
            on: !model.monitor_on,
            device: None,
        }),
        Intent::PickMonitorDevice(name) => Some(PipeRequest::Monitor {
            on: true,
            device: Some(name.clone()),
        }),
        Intent::StartAutoTune => Some(PipeRequest::Calibrate),
        Intent::CancelAutoTune => Some(PipeRequest::CalibrateCancel),
        Intent::PreviewCalibration {
            style,
            keep_pins,
            audition_raw,
        } => Some(PipeRequest::CalibratePreview {
            style: style.as_str().to_owned(),
            keep_pins: *keep_pins,
            audition_raw: *audition_raw,
        }),
        Intent::ApplyCalibration { keep_pins } => Some(PipeRequest::CalibrateApply { keep_pins: *keep_pins }),
        Intent::DiscardCalibration => Some(PipeRequest::CalibrateDiscard),
        Intent::SetProDrawerOpen(open) => Some(PipeRequest::SetProDrawerOpen { open: *open }),
        Intent::StartMicCheck => Some(PipeRequest::MicCheckStart),
        Intent::SetMicCheckMode(raw) => Some(PipeRequest::MicCheckSetMode { raw: *raw }),
        Intent::StopMicCheck => Some(PipeRequest::MicCheckStop),
        Intent::OpenSettings | Intent::CloseWindow | Intent::InstallApo | Intent::RepairApo => None,
    };
    if matches!(
        intent,
        Intent::SelectEndpoint(_)
            | Intent::SetArchetype(_)
            | Intent::SetDenoiseEngine(_)
            | Intent::SetParkWhenUnused(_)
            | Intent::SetMacro(..)
            | Intent::SetParam(..)
            | Intent::Unpin(_)
            | Intent::UnpinAll
            | Intent::ToggleBypass
            | Intent::SetMuted(_)
            | Intent::ToggleMonitor
            | Intent::PickMonitorDevice(_)
            | Intent::PreviewCalibration { .. }
            | Intent::ApplyCalibration { .. }
    ) {
        model.last_local_edit = Some(Instant::now());
    }
    let started_calibrate = matches!(req, Some(PipeRequest::Calibrate));
    let Some(req) = req else {
        tracing::debug!(target: "tine::ui", intent = ?intent, "local-only intent (no pipe request)");
        return false;
    };
    match single_instance::send_request(pipe, &req) {
        Ok(PipeResponse { ok: true, data, .. }) => {
            tracing::debug!(target: "tine::ui", intent = ?intent, "intent applied");
            if let PipeRequest::Bypass { on } = req {
                model.bypass = on;
            }
            if let PipeRequest::Mute { on } = req
                && let Some(m) = on
            {
                model.muted = m;
            }
            if let PipeRequest::Monitor { on, .. } = req {
                model.monitor_on = on;
            }
            if let PipeRequest::Endpoint { id } = &req {
                model.endpoint_id.clone_from(id);
            }
            if let PipeRequest::Archetype { name } = &req
                && let Some(arch) = crate::cli::archetype_from_name(name)
            {
                model.archetype = arch;
            }
            if let PipeRequest::SetDenoiseEngine { engine } = &req
                && let Some(parsed) = DenoiseEngine::from_name(engine)
            {
                model.denoise_engine = parsed;
            }
            if let PipeRequest::SetParkWhenUnused { enabled } = req {
                model.park_when_unused = enabled;
            }
            // The wizard poll starts only when the parent actually
            // began a run (a missing routing link answers otherwise).
            let running = started_calibrate
                && data
                    .as_ref()
                    .and_then(|d| d.get("calibrate"))
                    .and_then(|c| c.get("flow"))
                    .and_then(Value::as_str)
                    == Some("running");
            if running {
                handle.update_if(move |s, tx| {
                    if matches!(
                        s.model.autotune.step,
                        AutoTuneStep::Idle | AutoTuneStep::Results | AutoTuneStep::Failed(_)
                    ) {
                        s.ui.autotune_audition_raw = None;
                        s.ui.autotune_style = tine_dsp::calibrator::VoiceStyle::AutoBalanced;
                        s.model.autotune = AutoTuneUi {
                            step: AutoTuneStep::Prepare {
                                remaining_secs: tine_dsp::calibrator::LIVE_COUNTDOWN_S,
                            },
                            progress: 0.0,
                            level_db: s.live.telemetry.input_rms_db.max(-60.0),
                            prompt_text: String::new(),
                            voiced_ms: 0,
                        };
                        tx.animated();
                        true
                    } else {
                        false
                    }
                });
            } else if started_calibrate {
                let msg = data
                    .as_ref()
                    .and_then(|d| d.get("notes"))
                    .and_then(Value::as_array)
                    .and_then(|arr| arr.iter().find_map(Value::as_str))
                    .map_or_else(
                        || {
                            String::from(
                                "Auto-Tune could not start — check your microphone and Virtual Cable status above.",
                            )
                        },
                        str::to_owned,
                    );
                handle.update_if(move |s, _| {
                    s.model.status_banner = Some(msg);
                    true
                });
            }
            running
        }
        Ok(resp) => {
            let msg = resp.error.unwrap_or_else(|| "request failed".to_owned());
            tracing::error!(target: "tine::ui", intent = ?intent, "{msg}");
            if started_calibrate {
                handle.update_if(move |s, _| {
                    s.model.status_banner = Some(msg);
                    true
                });
            }
            false
        }
        Err(e) => {
            tracing::error!(target: "tine::ui", intent = ?intent, "parent unreachable: {e}");
            if started_calibrate {
                let msg = format!("Auto-Tune could not start: {e}");
                handle.update_if(move |s, _| {
                    s.model.status_banner = Some(msg);
                    true
                });
            }
            false
        }
    }
}

/// Spawns the admin worker (D-018 "install"/"repair" = a virtual-cable
/// presence re-check + refreshed badge + router follow + onboarding remedy).
fn spawn_admin_check(handle: rune::AppHandle<AppState>) {
    let fallback = handle.clone();
    if std::thread::Builder::new()
        .name("tine-ui-admin".to_owned())
        .spawn(move || {
            DEVICES_DIRTY.store(true, Ordering::Release);
            let badge = match badge_from_health(&apo_admin::health("")) {
                ApoBadge::Active => ApoBadge::Active,
                ApoBadge::Inactive => {
                    ApoBadge::Error("Virtual cable is disabled in Windows Sound Settings (mmsys.cpl)".to_owned())
                }
                ApoBadge::NotInstalled => ApoBadge::Error(
                    "No virtual audio cable detected — install VB-Cable (or run setup-virtual-cable.ps1 -Install)"
                        .to_owned(),
                ),
                other @ ApoBadge::Error(_) => other,
            };
            if matches!(badge, ApoBadge::Active) {
                tracing::info!(target: "tine::ui", "virtual cable healthy — starting audio routing");
                let pipe = single_instance::pipe_name_for_user();
                match single_instance::send_request(&pipe, &PipeRequest::RouterFollow) {
                    Ok(PipeResponse { ok: true, .. }) => {}
                    Ok(resp) => {
                        let msg = resp.error.unwrap_or_else(|| "router follow failed".to_owned());
                        tracing::warn!(target: "tine::ui", "{msg}");
                    }
                    Err(e) => {
                        tracing::warn!(target: "tine::ui", "router follow unreachable: {e}");
                    }
                }
            } else {
                tracing::warn!(target: "tine::ui",
                    "Tine needs a virtual audio cable: install VB-Audio Virtual Cable, then use Repair");
            }
            handle.update_if(move |s, _| {
                s.model.apo = badge;
                s.ui.apo_installing = false;
                true
            });
        })
        .is_err()
    {
        tracing::error!(target: "tine::ui", "admin worker spawn failed");
        fallback.update_if(move |s, _| {
            s.ui.apo_installing = false;
            true
        });
    }
}

/// One status snapshot into the Rune state: macros, bypass, monitor, and
/// profile-derived Pro params / pins (the profile file is the parent
/// controller's single-writer output, read here outside the edit cooldown).
fn child_refresh_model(handle: &rune::AppHandle<AppState>, model: &mut ChildModel) {
    if !WINDOW_VISIBLE.load(Ordering::Acquire) {
        return;
    }
    let Ok(status) = fetch_status() else {
        tracing::debug!(target: "tine::ui", "status unavailable (parent starting?)");
        return;
    };
    // Re-enumerate COM audio devices only when DeviceWatcher signaled a change,
    // on initial load, or as a 30s fallback (avoids 3 COM enumerations every 2s).
    let device_stale = model
        .last_device_refresh
        .is_none_or(|t| t.elapsed() >= Duration::from_secs(30));
    if DEVICES_DIRTY.swap(false, Ordering::AcqRel) || device_stale {
        model.last_device_refresh = Some(Instant::now());
        model.endpoints = apo_admin::list_capture_endpoints().map_or_else(
            |e| {
                tracing::warn!(target: "tine::ui", "endpoint re-enumeration failed: {e}");
                model.endpoints.clone()
            },
            |list| list.into_iter().map(|e| e.id).collect::<Vec<_>>(),
        );
        model.output_devices = list_outputs().map_or_else(
            |_| model.output_devices.clone(),
            |list| list.into_iter().map(|d| d.name).collect::<Vec<_>>(),
        );
        model.badge = badge_from_health(&apo_admin::health(&model.endpoint_id));
    }
    let mut note_banner: Option<String> = None;
    if let Some(notes) = status.get("notes").and_then(Value::as_array)
        && !notes.is_empty()
    {
        for note in notes {
            if let Some(text) = note.as_str() {
                tracing::warn!(target: "tine::ui", "{text}");
                if note_banner.is_none() && !text.is_empty() {
                    note_banner = Some(text.to_owned());
                }
            } else {
                tracing::warn!(target: "tine::ui", "{note}");
            }
        }
    }
    let in_edit_cooldown = model.last_local_edit.is_some_and(|t| t.elapsed() < EDIT_COOLDOWN);
    if !in_edit_cooldown {
        model.bypass = status.get("bypass").and_then(Value::as_bool).unwrap_or(model.bypass);
        model.muted = status.get("muted").and_then(Value::as_bool).unwrap_or(model.muted);
        if let Some(engine) = status
            .get("denoise_engine")
            .and_then(Value::as_str)
            .and_then(DenoiseEngine::from_name)
        {
            model.denoise_engine = engine;
        }
        model.park_when_unused = status
            .get("park_when_unused")
            .and_then(Value::as_bool)
            .unwrap_or(model.park_when_unused);
        model.monitor_on = status
            .get("monitor")
            .and_then(Value::as_bool)
            .unwrap_or(model.monitor_on);
        let next_id = status
            .get("endpoint_id")
            .and_then(Value::as_str)
            .filter(|id| !id.is_empty())
            .map(str::to_owned);
        if let Some(next_id) = next_id {
            model.endpoint_id = next_id;
        }
    }
    let bypass = model.bypass;
    let muted = model.muted;
    let denoise_engine = model.denoise_engine;
    let park_when_unused = model.park_when_unused;
    let monitor_on = model.monitor_on;
    let monitor_device = status.get("monitor_device").and_then(Value::as_str).map(str::to_owned);
    let latency = if monitor_on { nominal_monitor_latency_ms() } else { 0.0 };
    let devices = model.output_devices.clone();
    let profile_update = if in_edit_cooldown {
        None
    } else {
        let mut macros = model_default_macros();
        if let Some(m) = status.get("macros").and_then(Value::as_object) {
            let get = |k: &str| m.get(k).and_then(Value::as_f64).map(|v| v as f32);
            macros = MacroPositions {
                purity: get("purity").unwrap_or(macros.purity).clamp(0.0, 1.0),
                body: get("body").unwrap_or(macros.body).clamp(0.0, 1.0),
                presence: get("presence").unwrap_or(macros.presence).clamp(0.0, 1.0),
                leveler: get("leveler").unwrap_or(macros.leveler).clamp(0.0, 1.0),
            };
        }
        // Pro rack state: resolve the persisted profile (single writer is the
        // parent controller, 500 ms trailing debounce + flush-on-exit).
        let (params, pins, archetype, active_calibration) = match EnginePaths::system() {
            Ok(paths) => {
                let profile = persist::load_profile(&model.endpoint_id, &paths).into_value();
                let cal = profile.calibration;
                let app_model = AppModel {
                    endpoint: (!model.endpoint_id.is_empty()).then(|| EndpointId::new(model.endpoint_id.clone())),
                    bypass,
                    monitor: MonitorSettings {
                        enabled: monitor_on,
                        output_device: monitor_device.clone(),
                    },
                    profile,
                };
                (
                    resolve(&app_model),
                    app_model.profile.pins,
                    app_model.profile.archetype,
                    cal,
                )
            }
            Err(e) => {
                tracing::warn!(target: "tine::ui", "paths unavailable: {e}");
                (
                    DspParams::DEFAULT,
                    tine_protocol::presets::PinSet::default(),
                    Archetype::default(),
                    None,
                )
            }
        };
        model.archetype = archetype;
        Some((macros, params, pins, archetype, active_calibration))
    };
    let raw_mic_apps: Vec<String> = status
        .get("raw_mic_apps")
        .and_then(Value::as_array)
        .map(|arr| arr.iter().filter_map(Value::as_str).map(str::to_owned).collect())
        .unwrap_or_default();
    let selected_id = model.endpoint_id.clone();
    let badge = model.badge.clone();
    handle.update_if(move |s, tx| {
        if s.model.first_run && s.ui.onboarding_dismissed {
            s.model.first_run = false;
            persist_onboarding_async();
        }
        if s.model.raw_mic_apps != raw_mic_apps {
            s.model.raw_mic_apps = raw_mic_apps;
            s.ui.raw_mic_banner_dismissed = false;
        }
        if let Some((macros, params, pins, archetype, active_calibration)) = profile_update {
            let changed = s.model.params != params
                || s.model.macros != macros
                || s.model.active_calibration != active_calibration;
            s.model.macros = macros;
            s.model.bypass = bypass;
            s.model.muted = muted;
            s.model.denoise_engine = denoise_engine;
            s.model.park_when_unused = park_when_unused;
            s.model.archetype = archetype;
            s.model.params = params;
            s.model.pins = pins;
            s.model.active_calibration = active_calibration;
            s.model.monitor.enabled = monitor_on;
            s.model.monitor.device_name = monitor_device;
            s.model.monitor.estimated_latency_ms = latency;
            if let Some(idx) = s.model.endpoints.iter().position(|e| e.id == selected_id) {
                s.model.selected = Some(idx);
            }
            if changed {
                tx.animated();
            }
        }
        s.model.monitor.available_devices = devices;
        if matches!(badge, ApoBadge::Active) || !matches!(s.model.apo, ApoBadge::Error(_)) {
            s.model.apo = badge;
        }
        if let Some(msg) = note_banner
            && s.model.status_banner.is_none()
        {
            s.model.status_banner = Some(msg);
        }
        true
    });
}

/// Macro placeholder when the parent has not published yet.
fn model_default_macros() -> MacroPositions {
    MacroPositions::default()
}

/// Watches device events (hot-plug, removal, default change) and refreshes
/// the endpoint list and badge into the Rune state.
fn spawn_device_watcher(handle: &rune::AppHandle<AppState>) {
    let Ok((watcher, rx)) = apo_admin::DeviceWatcher::new() else {
        tracing::warn!(target: "tine::ui", "device watcher unavailable");
        return;
    };
    let handle = handle.clone();
    if std::thread::Builder::new()
        .name("tine-ui-watcher".to_owned())
        .spawn(move || {
            let watcher = watcher;
            while let Ok(_event) = rx.recv() {
                DEVICES_DIRTY.store(true, Ordering::Release);
                match apo_admin::list_capture_endpoints() {
                    Ok(list) => {
                        let rows = endpoint_rows(&list);
                        let health = list.first().map_or_else(|| ApoState::NotInstalled, |e| e.state.clone());
                        let badge = badge_from_health(&health);
                        handle.update_if(move |s, _| {
                            s.model.endpoints = rows;
                            s.model.apo = badge;
                            true
                        });
                    }
                    Err(e) => tracing::warn!(target: "tine::ui", "re-enumeration failed: {e}"),
                }
            }
            drop(watcher);
        })
        .is_err()
    {
        tracing::error!(target: "tine::ui", "watcher thread spawn failed");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn band_level_maps_the_full_range() {
        assert_eq!(band_level(-90.0), 0.0);
        assert!((band_level(-45.0) - 0.5).abs() < 1e-6);
        assert_eq!(band_level(0.0), 1.0);
        assert_eq!(band_level(6.0), 1.0);
        assert_eq!(band_level(f32::NAN), 0.0);
    }

    #[test]
    fn live_from_frame_normalizes_each_band() {
        let mut f = UiFrame::default();
        // An analyzer frame: silence sits on the floor, tones read dBFS.
        f.spectrum.fill(SPECTRUM_FLOOR_DB);
        f.spectrum[3] = -30.0;
        f.spectrum[4] = 0.0; // full scale reads 1.0
        f.telemetry.sequence = 7;
        let live = live_from_frame(&f);
        assert_eq!(live.telemetry.sequence, 7);
        assert!((live.spectrum[3] - (2.0 / 3.0)).abs() < 1e-6);
        assert_eq!(live.spectrum[0], 0.0);
        assert_eq!(live.spectrum[4], 1.0);
    }

    #[test]
    fn live_from_meters_parses_the_pipe_payload() {
        let data = serde_json::json!({
            "telemetry": {
                "sequence": 9,
                "input_rms_db": -34.5,
                "input_peak_db": -12.0,
                "output_rms_db": -18.0,
                "output_peak_db": -6.0
            },
            "spectrum": [-90.0, -30.0, 0.0]
        });
        let live = live_from_meters(&data).expect("payload");
        assert_eq!(live.telemetry.sequence, 9);
        assert_eq!(live.telemetry.input_rms_db, -34.5);
        assert_eq!(live.spectrum[1], 2.0 / 3.0);
        assert_eq!(live.spectrum[2], 1.0);
        assert!(live_from_meters(&serde_json::json!({ "spectrum": [] })).is_none());
    }

    #[test]
    fn badge_maps_the_d018_cable_states() {
        assert!(matches!(
            badge_from_health(&ApoState::Active { heartbeat_age_ms: 0 }),
            ApoBadge::Active
        ));
        assert!(matches!(
            badge_from_health(&ApoState::NotInstalled),
            ApoBadge::NotInstalled
        ));
        assert!(matches!(
            badge_from_health(&ApoState::InstalledNotLoaded),
            ApoBadge::Inactive
        ));
        assert!(matches!(
            badge_from_health(&ApoState::RemovedBySystem),
            ApoBadge::Inactive
        ));
        let win_err = tine_engine::win::WinError::Com {
            context: "test",
            source: windows::core::Error::from(windows::core::HRESULT(-2_147_418_113_i32)),
        };
        assert!(matches!(
            badge_from_health(&ApoState::Error(apo_admin::ApoError::Win(win_err))),
            ApoBadge::Error(_)
        ));
    }

    #[test]
    fn param_key_round_trips_the_cli_parser() {
        for spec in &SPECS {
            let key = param_key(spec.id).expect("every param has a key");
            assert!(crate::cli::param_id_by_key(key).is_some(), "{key} must parse");
            assert_eq!(crate::cli::param_id_by_key(key), Some(spec.id));
        }
    }

    #[test]
    fn nominal_monitor_latency_matches_the_d026_formula() {
        assert!((nominal_monitor_latency_ms() - (10.0 + 10.0 + 21.6)).abs() < 0.01);
    }

    #[test]
    fn wizard_state_maps_each_flow_of_the_wire_payload() {
        // Running: phase + level + live duration overrides drive the wizard's step + meter.
        let (ui, cal, terminal) = autotune_ui_from_state(&serde_json::json!({
            "flow": "running", "phase": "Noise", "progress": 0.5,
            "remaining_secs": 1.75, "overall_progress": 0.184,
            "level_db": -40.0, "voiced_ms": 0
        }))
        .expect("running frame");
        assert!(
            matches!(ui.step, AutoTuneStep::Silence { remaining_secs } if (remaining_secs - 1.75).abs() < 0.01),
            "{:?}",
            ui.step
        );
        assert!((ui.progress - 0.184).abs() < 0.001);
        assert!((ui.level_db - (-40.0)).abs() < 0.01);
        assert!(!terminal && cal.is_none());

        // Report-pending: the results card carries the measured numbers
        // and SPECS-keyed diffs become labeled rows.
        let pending_payload = serde_json::json!({
            "flow": "report-pending", "summary": "measured",
            "report": {
                "noise_rms_db": -50.0, "noise_peak_db": -38.0, "speech_rms_db": -20.0,
                "crest_db": 14.5, "tilt_db": 4.2,
                "simulated_peak_db": -4.5, "simulated_rms_db": -18.0,
                "changes": [ { "key": "input_gain_db", "before": 0.0, "after": 6.5 } ]
            }
        });
        let (ui, cal, terminal) = autotune_ui_from_state(&pending_payload).expect("results frame");
        assert!(matches!(ui.step, AutoTuneStep::Results));
        let cal = cal.expect("results card");
        assert!((cal.simulated_peak_db - (-4.5)).abs() < 0.01);
        assert_eq!(cal.changes.len(), 1);
        assert_eq!(cal.changes[0].param, ParamId::InputGainDb);
        assert_eq!(cal.changes[0].unit, "dB");
        assert!(terminal);
        let full_cal = pending_calibration_from_state(&pending_payload, Archetype::Dynamic).expect("full cal");
        assert!((full_cal.tilt_db - 4.2).abs() < 0.01);
        assert!((full_cal.crest_db - 14.5).abs() < 0.01);
        assert!((full_cal.derived.input_gain_db - 6.5).abs() < 0.01);

        // Failed: the typed rejection maps the failure card.
        let (ui, _, terminal) = autotune_ui_from_state(&serde_json::json!({
            "flow": "failed",
            "failure": { "type": "TooNoisy", "snr_db": 4.0 },
            "message": "too noisy"
        }))
        .expect("failure frame");
        assert!(matches!(
            ui.step,
            AutoTuneStep::Failed(CalFailureUi::TooNoisy { snr_db }) if (snr_db - 4.0).abs() < 0.01
        ));
        assert!(terminal);

        // Idle closes the wizard; an unknown flow is skipped, not mapped.
        let (ui, _, terminal) = autotune_ui_from_state(&serde_json::json!({ "flow": "idle" })).expect("idle frame");
        assert!(matches!(ui.step, AutoTuneStep::Idle));
        assert!(terminal);
        assert!(autotune_ui_from_state(&serde_json::json!({ "flow": "from-the-future" })).is_none());
    }
}
