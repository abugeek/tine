//! Engine controller synchronizing `AppModel` and shared memory.
//!
//! The controller is the user-side brain: a single owner thread driven by
//! [`Command`]s (never called into from RT callbacks; `std::sync::mpsc` is
//! fine here). It mutates the [`AppModel`], resolves [`DspParams`] (see
//! [`crate::model::resolve`]), publishes them through a [`Target`] (the APO's
//! shared memory, or an in-process `ParamBlock` for tests and `devhost`),
//! streams back [`Event`]s, and persists state with a 500 ms trailing
//! debounce (flushed on exit).
//!
//! Idle cost is zero while linked: the thread blocks on the command channel
//! and never polls (reconnect attempts run every second **only while not
//! linked**).

use std::sync::Arc;
use std::sync::Mutex;
use std::sync::mpsc::{Receiver, RecvTimeoutError, SendError, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::{Archetype, Calibration, EndpointProfile, MacroPositions, PresetParams};
use tine_protocol::settings::AppSettings;
use tine_protocol::sync::ParamBlock;
use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::model::{
    AppModel, EndpointId, EngineError, KeepPins, Macro, MonitorSettings, apply_override, clear_override, resolve,
};
use crate::paths::EnginePaths;
use crate::persist;
use crate::shm_client::{LinkState, ShmClient, Target};

/// Persistence debounce: writes land 500 ms after the last change.
pub const PERSIST_DEBOUNCE: Duration = Duration::from_millis(500);

/// Command from the UI/CLI/pipe to the controller.
#[derive(Clone, Debug, PartialEq)]
pub enum Command {
    /// Select the capture endpoint (loads or creates its profile).
    SelectEndpoint(EndpointId),
    /// Switch the hardware archetype. Pins are kept; the UI asks the user
    /// whether to keep pins (D-007) and sends [`Command::UnpinAll`] if not.
    SetArchetype(Archetype),
    /// Move a Golden Line macro (`0.0..=1.0`; pinned parameters do not move).
    SetMacro(Macro, f32),
    /// Set one parameter in Pro mode (pins it per D-007).
    SetParam(ParamId, f32),
    /// Unpin a parameter and return it to macro control.
    Unpin(ParamId),
    /// Unpin everything.
    UnpinAll,
    /// Global A/B bypass.
    SetBypass(bool),
    /// Adopt an Auto-Tune result (T16's runner hands over the
    /// [`tine_protocol::presets::Calibration`]); [`KeepPins`] decides what
    /// happens to the pins of the parameters calibration now drives.
    ///
    /// Note: T11/T16 produce a richer `CalibrationReport`; the engine only
    /// needs its `calibration` payload, so the reporter extracts it.
    ApplyCalibration(Calibration, KeepPins),
    /// Reset the profile to fresh archetype defaults (macros, overrides,
    /// calibration, and pins are all cleared).
    ResetToArchetype,
    /// Update "Hear Myself" monitor settings (controller state, D-008).
    SetMonitor(MonitorSettings),
}

/// Event from the controller to its listeners (UI, tray, CLI pipe).
#[derive(Debug)]
pub enum Event {
    /// The model changed (Arc for cheap sharing with the UI thread).
    ModelChanged(Arc<AppModel>),
    /// Parameters were re-resolved and published to the target.
    ParamsResolved(DspParams),
    /// The APO link state changed (connected, lost, version mismatch).
    ApoLink(LinkState),
    /// A non-fatal engine error surfaced while running.
    Error(EngineError),
}

/// Startup configuration for the controller.
#[derive(Debug)]
pub struct EngineConfig {
    /// Endpoint to select on startup (`None` keeps the persisted selection).
    pub endpoint: Option<EndpointId>,
    /// Where resolved parameters are published.
    pub target: Target,
    /// Root directories for persistence.
    pub paths: EnginePaths,
}

impl EngineConfig {
    /// Production configuration: the APO's shared memory for `endpoint`.
    #[must_use]
    pub fn shm(endpoint: &EndpointId, paths: EnginePaths) -> Self {
        Self {
            endpoint: Some(endpoint.clone()),
            target: Target::shm(endpoint),
            paths,
        }
    }

    /// Test/`devhost` configuration: an in-process parameter block.
    #[must_use]
    pub fn block(endpoint: EndpointId, block: Arc<ParamBlock>, paths: EnginePaths) -> Self {
        Self {
            endpoint: Some(endpoint),
            target: Target::block(block),
            paths,
        }
    }
}

/// Shared slot holding the live shared-memory client (if any) so the handle
/// can serve telemetry/tap reads without touching the controller thread.
///
/// Control-plane metadata only: the lock is never held across I/O, and no
/// audio thread ever takes it (the UI/visualizer thread reads a clone out in
/// O(1)).
type SharedClient = Arc<Mutex<Option<Arc<ShmClient>>>>;

/// Handle for sending [`Command`]s and reading telemetry from other threads.
///
/// `Clone + Send + Sync`. Telemetry and tap reads go straight to the
/// shared-memory lock-free primitives ([`ShmClient`]); by convention
/// [`Self::telemetry`] and [`Self::drain_tap`] are called from exactly one
/// reader thread (triple-buffer precondition), typically the visualizer.
#[derive(Debug)]
pub struct ControllerHandle {
    tx: Arc<Sender<Command>>,
    client: SharedClient,
}

impl Clone for ControllerHandle {
    fn clone(&self) -> Self {
        Self {
            tx: Arc::clone(&self.tx),
            client: Arc::clone(&self.client),
        }
    }
}

impl ControllerHandle {
    /// Sends a command to the controller.
    ///
    /// Fails only when the controller thread has exited (app shutdown or
    /// spawn failure).
    pub fn send(&self, cmd: Command) -> Result<(), SendError<Command>> {
        self.tx.send(cmd)
    }

    /// Latest telemetry frame (defaults when not linked over shared memory).
    #[must_use]
    pub fn telemetry(&self) -> AudioTelemetryFrame {
        match self.client.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(client) => client.telemetry(),
                None => AudioTelemetryFrame::default(),
            },
            Err(_) => AudioTelemetryFrame::default(),
        }
    }

    /// Drains up to `buf.len()` post-DSP tap samples (0 when not linked).
    pub fn drain_tap(&self, buf: &mut [f32]) -> usize {
        match self.client.lock() {
            Ok(guard) => match guard.as_ref() {
                Some(client) => client.drain_tap(buf),
                None => 0,
            },
            Err(_) => 0,
        }
    }

    /// The live shared-memory client, if linked (visualizer services,
    /// diagnostics). The snapshot is only current until the next
    /// [`Event::ApoLink`] transition.
    #[must_use]
    pub fn shm_client(&self) -> Option<Arc<ShmClient>> {
        match self.client.lock() {
            Ok(guard) => guard.as_ref().map(Arc::clone),
            Err(_) => None,
        }
    }
}

/// Namespace for the controller's entry point; all state lives on its thread.
pub struct Controller;

impl Controller {
    /// Spawns the controller thread and returns the handle and the event
    /// receiver.
    ///
    /// Dropping (or cloning away) every `ControllerHandle` flushes persistence
    /// and stops the thread. If the OS refuses to spawn the thread (OOM-level
    /// failure), every `send` errors and the event receiver is immediately
    /// disconnected.
    #[must_use]
    pub fn spawn(cfg: EngineConfig) -> (ControllerHandle, Receiver<Event>) {
        let (tx_cmd, rx_cmd) = channel::<Command>();
        let (tx_evt, rx_evt) = channel::<Event>();
        let client_slot: SharedClient = Arc::new(Mutex::new(None));

        let handle = ControllerHandle {
            tx: Arc::new(tx_cmd),
            client: Arc::clone(&client_slot),
        };

        let slot = Arc::clone(&client_slot);
        // On spawn failure the closure (holding both channel senders/most
        // receivers) is dropped, which disconnects both channels — callers
        // observe a dead controller instead of a half-alive one.
        let _spawned = thread::Builder::new()
            .name("tine-controller".to_owned())
            .spawn(move || run(cfg, rx_cmd, tx_evt, slot));
        (handle, rx_evt)
    }
}

/// The controller thread's loop state.
struct Loop {
    model: AppModel,
    settings: AppSettings,
    target: Target,
    current: DspParams,
    persist_at: Option<Instant>,
    poll_next: Option<Instant>,
}

/// Controller-thread main loop (message driven, blocking receive).
// The thread entry point owns its channel endpoints and config by design.
#[allow(clippy::needless_pass_by_value)]
fn run(cfg: EngineConfig, rx: Receiver<Command>, tx: Sender<Event>, slot: SharedClient) {
    let paths = cfg.paths;

    // ---- Startup: load persisted state --------------------------------
    let settings = persist::load_settings(&paths).into_value();
    let endpoint: Option<EndpointId> = cfg
        .endpoint
        .or_else(|| settings.selected_endpoint_id.clone().map(EndpointId::new));

    let model = AppModel {
        endpoint: endpoint.clone(),
        profile: match &endpoint {
            Some(id) => persist::load_profile(id.as_str(), &paths).into_value(),
            None => EndpointProfile::new("", Archetype::default()),
        },
        bypass: false,
        monitor: MonitorSettings {
            output_device: settings.monitor_device_id.clone(),
            enabled: settings.monitor_enabled,
        },
    };

    let mut target = cfg.target;
    target.set_endpoint(model.endpoint.clone());

    // Resolve and publish the initial snapshot.
    let current = resolve(&model);
    target.write(&current);
    if let Some(client) = target_to_client(&target) {
        update_slot(&slot, Some(client));
    }
    let _ = tx.send(Event::ParamsResolved(current));
    let _ = tx.send(Event::ModelChanged(Arc::new(model.clone())));

    let mut state = Loop {
        model,
        settings,
        target,
        current,
        persist_at: None,
        poll_next: None,
    };
    // First link poll (also emits the initial `ApoLink` event).
    state.poll_next = poll_and_update(&mut state, &tx, &slot);

    loop {
        // Wake at the earliest deadline; block indefinitely while linked with
        // no pending write (0 % idle CPU).
        let now = Instant::now();
        let persist_wait = state.persist_at.map(|t| t.saturating_duration_since(now));
        let poll_wait = state.poll_next.map(|t| t.saturating_duration_since(now));
        let wait = [persist_wait, poll_wait].into_iter().flatten().min();

        let received: Result<Option<Command>, ()> = match wait {
            None => match rx.recv() {
                Ok(cmd) => Ok(Some(cmd)),
                Err(_) => Err(()),
            },
            Some(d) => match rx.recv_timeout(d) {
                Ok(cmd) => Ok(Some(cmd)),
                Err(RecvTimeoutError::Timeout) => Ok(None),
                Err(RecvTimeoutError::Disconnected) => Err(()),
            },
        };

        match received {
            Ok(Some(cmd)) => {
                apply_command(&mut state, &cmd, &tx, &slot, &paths);
                state.persist_at = Some(Instant::now() + PERSIST_DEBOUNCE);
                state.poll_next = poll_and_update(&mut state, &tx, &slot);
            }
            Ok(None) => {
                if let Some(deadline) = state.persist_at
                    && deadline <= Instant::now()
                {
                    state.persist_at = None;
                    flush_persistence(&mut state, &tx, &paths);
                }
                state.poll_next = poll_and_update(&mut state, &tx, &slot);
            }
            Err(()) => {
                // All handles dropped: flush and exit.
                flush_persistence(&mut state, &tx, &paths);
                break;
            }
        }
    }
}

/// Applies one command to the loop state; emits events; re-resolves and
/// re-publishes parameters.
fn apply_command(state: &mut Loop, cmd: &Command, tx: &Sender<Event>, slot: &SharedClient, paths: &EnginePaths) {
    let model = &mut state.model;
    let mut endpoint_changed = false;
    match cmd {
        Command::SelectEndpoint(id) => {
            model.endpoint = Some(id.clone());
            state.settings.selected_endpoint_id = Some(id.as_str().to_owned());
            let loaded = persist::load_profile(id.as_str(), paths);
            if let persist::Loaded::Restored(_) = loaded {
                tracing::warn!(target: "tine::controller", endpoint = %id, "profile corrupt; using defaults");
            }
            model.profile = loaded.into_value();
            endpoint_changed = true;
        }
        Command::SetArchetype(a) => {
            model.profile.archetype = *a;
            // Pins are deliberately kept here (D-007); the UI asks the user
            // and sends `UnpinAll` when they choose to release them.
        }
        Command::SetMacro(m, v) => {
            m.set(&mut model.profile.macros, *v);
        }
        Command::SetParam(id, v) => {
            let clamped = crate::model::clamp_param(*id, *v);
            model.profile.pins.pin(*id);
            apply_override(&mut model.profile.overrides, *id, clamped);
        }
        Command::Unpin(id) => {
            model.profile.pins.unpin(*id);
            clear_override(&mut model.profile.overrides, *id);
        }
        Command::UnpinAll => {
            model.profile.pins.clear();
            model.profile.overrides = PresetParams::default();
        }
        Command::SetBypass(b) => {
            model.bypass = *b;
        }
        Command::ApplyCalibration(cal, keep_pins) => {
            model.profile.calibration = Some(*cal);
            if *keep_pins == KeepPins::Clear {
                for id in [
                    ParamId::InputGainDb,
                    ParamId::GateThresholdDb,
                    ParamId::DenoiseMix,
                    ParamId::CompThreshDb,
                    ParamId::CompMakeupDb,
                ] {
                    model.profile.pins.unpin(id);
                    clear_override(&mut model.profile.overrides, id);
                }
            }
        }
        Command::ResetToArchetype => {
            // Full reset to the fresh-profile defaults (documented choice:
            // a reset also releases pins; a selective reset belongs to the UI).
            model.profile.macros = MacroPositions::default();
            model.profile.overrides = PresetParams::default();
            model.profile.calibration = None;
            model.profile.pins.clear();
        }
        Command::SetMonitor(monitor) => {
            model.monitor = monitor.clone();
        }
    }

    if endpoint_changed {
        state.target.set_endpoint(state.model.endpoint.clone());
        update_slot(slot, None);
    }

    state.current = resolve(&state.model);
    state.target.write(&state.current);
    let _ = tx.send(Event::ParamsResolved(state.current));
    let _ = tx.send(Event::ModelChanged(Arc::new(state.model.clone())));
}

/// Runs one link poll; emits `ApoLink` on transitions and writes the current
/// params on a fresh connection. Returns the next poll deadline, or `None`
/// while linked (no polling at all).
fn poll_and_update(state: &mut Loop, tx: &Sender<Event>, slot: &SharedClient) -> Option<Instant> {
    if let Some((link_state, client)) = state.target.poll() {
        if let Some(client) = client {
            client.write_params(&state.current);
        }
        update_slot(slot, state.target.client());
        let _ = tx.send(Event::ApoLink(link_state));
    }
    if state.target.state().is_linked() {
        None
    } else {
        Some(Instant::now() + state.target.poll_period())
    }
}

/// Stores the live client into the shared slot (metadata only; the mutex is
/// never held across I/O).
fn update_slot(slot: &SharedClient, client: Option<Arc<ShmClient>>) {
    if let Ok(mut guard) = slot.lock() {
        *guard = client;
    }
}

fn target_to_client(target: &Target) -> Option<Arc<ShmClient>> {
    target.client()
}

/// Writes all persistence layers for the current state (debounce deadline or
/// exit).
fn flush_persistence(state: &mut Loop, tx: &Sender<Event>, paths: &EnginePaths) {
    state.settings.selected_endpoint_id = state.model.endpoint.as_ref().map(|e| e.as_str().to_owned());
    state.settings.monitor_device_id = state.model.monitor.output_device.clone();
    state.settings.monitor_enabled = state.model.monitor.enabled;

    if state.model.endpoint.is_some()
        && let Err(e) = persist::save_profile(&state.model.profile, paths)
    {
        let _ = tx.send(Event::Error(EngineError::from(e)));
    }
    if let Err(e) = persist::save_settings(&state.settings, paths) {
        let _ = tx.send(Event::Error(EngineError::from(e)));
    }
}

#[cfg(test)]
mod tests {
    use super::{Command, Controller, EngineConfig, Event, PERSIST_DEBOUNCE};
    use crate::model::{EndpointId, KeepPins, Macro, MonitorSettings};
    use crate::paths::EnginePaths;
    use std::sync::Arc;
    use std::time::{Duration, Instant};
    use tine_protocol::params::DspParams;
    use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides};
    use tine_protocol::sync::ParamBlock;
    use tine_protocol::telemetry::AudioTelemetryFrame;

    fn temp_paths(tag: &str) -> EnginePaths {
        let dir = std::env::temp_dir().join(format!("tine-ctl-{tag}-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        EnginePaths::from_roots(dir.join("app"), dir.join("prog"), dir.join("local"))
    }

    fn wait_event(
        rx: &std::sync::mpsc::Receiver<Event>,
        pred: impl Fn(&Event) -> bool,
        budget: Duration,
    ) -> Option<Event> {
        let start = Instant::now();
        while start.elapsed() < budget {
            match rx.recv_timeout(Duration::from_millis(50)) {
                Ok(e) if pred(&e) => return Some(e),
                Ok(_) | Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return None,
            }
        }
        None
    }

    #[test]
    fn controller_publishes_params_and_flushes_on_drop() {
        let paths = temp_paths("publish");
        let block = Arc::new(ParamBlock::new());
        let cfg = EngineConfig::block(EndpointId::new("mic-a"), Arc::clone(&block), paths.clone());
        let (handle, rx) = Controller::spawn(cfg);
        // Startup emits ParamsResolved + ModelChanged; wait for both.
        let first = wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), Duration::from_secs(5));
        assert!(first.is_some(), "initial ModelChanged must arrive");

        let sent = handle.send(Command::SetMacro(Macro::Purity, 0.9));
        assert!(sent.is_ok(), "send must succeed while the handle is alive");
        let resolved = wait_event(&rx, |e| matches!(e, Event::ParamsResolved(_)), Duration::from_secs(5));
        assert!(resolved.is_some(), "expect a ParamsResolved event");
        let mut loaded = DspParams::DEFAULT;
        let mut last_gen = 0u64;
        assert!(block.load_if_changed(&mut last_gen, &mut loaded));
        assert!((loaded.denoise_mix - 0.9).abs() < f32::EPSILON);

        // Persistence flushes when all handles drop.
        let t0 = Instant::now();
        drop(handle);
        drop(rx);
        let file = paths.profile_file("mic-a");
        let mut appeared = false;
        while t0.elapsed() < Duration::from_secs(5) {
            if file.exists() {
                appeared = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(appeared, "profile must be flushed on exit");
    }

    fn test_calibration() -> Calibration {
        Calibration {
            noise_rms_db: -60.0,
            noise_peak_db: -55.0,
            speech_rms_db: -28.0,
            crest_db: 12.0,
            tilt_db: 3.0,
            derived: CalibrationOverrides {
                input_gain_db: 9.5,
                gate_threshold_db: -54.0,
                denoise_mix: 0.5,
                comp_thresh_db: -16.0,
                comp_makeup_db: 2.0,
            },
        }
    }

    #[test]
    fn every_command_round_trips_through_the_model() {
        let paths = temp_paths("commands");
        let block = Arc::new(ParamBlock::new());
        let (handle, rx) = Controller::spawn(EngineConfig::block(EndpointId::new("mic-b"), Arc::clone(&block), paths));
        let rt = Duration::from_secs(5);
        wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), rt);

        let cases: Vec<Command> = vec![
            Command::SetArchetype(Archetype::Headset),
            Command::SetMacro(Macro::Body, 0.2),
            Command::SetParam(tine_protocol::params::ParamId::HpfFreqHz, 95.0),
            Command::SetBypass(true),
            Command::SetMonitor(MonitorSettings {
                output_device: Some("out-1".into()),
                enabled: true,
            }),
            Command::Unpin(tine_protocol::params::ParamId::HpfFreqHz),
            Command::UnpinAll,
            Command::ApplyCalibration(test_calibration(), KeepPins::Clear),
            Command::ResetToArchetype,
        ];
        for cmd in cases {
            assert!(handle.send(cmd.clone()).is_ok(), "send {cmd:?}");
            let got = wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), rt);
            assert!(got.is_some(), "ModelChanged after {cmd:?}");
        }
        assert!(handle.send(Command::SelectEndpoint(EndpointId::new("mic-c"))).is_ok());
        let switched = wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), rt);
        assert!(switched.is_some(), "endpoint switch emits");
        drop(handle);
        drop(rx);
    }

    /// Idle CPU evidence: with no pending work (linked Block target, no
    /// pending persistence) the controller blocks; process CPU time over an
    /// idle window stays ~0 (a poll loop would burn one core).
    #[test]
    fn controller_idle_costs_no_cpu() {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::GetProcessTimes;

        fn cpu_ms() -> u64 {
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            // SAFETY: GetCurrentProcess returns a pseudo-handle, valid for
            // this process; all pointers point to valid FILETIMEs.
            unsafe {
                let proc = windows::Win32::System::Threading::GetCurrentProcess();
                if GetProcessTimes(proc, &raw mut creation, &raw mut exit, &raw mut kernel, &raw mut user).is_err() {
                    return u64::MAX;
                }
            }
            let to_ms = |t: FILETIME| (u64::from(t.dwHighDateTime) << 32 | u64::from(t.dwLowDateTime)) / 10_000;
            to_ms(kernel) + to_ms(user)
        }

        let paths = temp_paths("idle");
        let block = Arc::new(ParamBlock::new());
        let (handle, rx) = Controller::spawn(EngineConfig::block(EndpointId::new("mic-idle"), block, paths));
        // Let the startup burst settle and the debounce flush land.
        wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), Duration::from_secs(2));
        std::thread::sleep(PERSIST_DEBOUNCE + Duration::from_millis(200));
        std::thread::sleep(Duration::from_millis(100));
        let before = cpu_ms();
        std::thread::sleep(Duration::from_millis(1_000));
        let after = cpu_ms();
        let delta = after.saturating_sub(before);
        println!("controller idle: {delta} ms process CPU time over 1_000 ms (budget < 100)");
        assert!(delta < 100, "controller idle burned {delta} ms CPU in 1 s (busy loop?)");
        let frame = handle.telemetry();
        assert_eq!(
            frame,
            AudioTelemetryFrame::default(),
            "no shm client -> default telemetry"
        );
        drop(handle);
        drop(rx);
    }

    /// Trend evidence for "audible in Discord within 50 ms": time from sending
    /// a `SetMacro` command to the new generation being visible in the
    /// parameter block.
    #[test]
    fn set_macro_is_visible_immediately() {
        let paths = temp_paths("latency");
        let block = Arc::new(ParamBlock::new());
        let (handle, rx) = Controller::spawn(EngineConfig::block(
            EndpointId::new("mic-lat"),
            Arc::clone(&block),
            paths,
        ));
        // Wait for the controller's startup snapshot to be published so the
        // next ParamBlock change is the command's, not the initial publish
        // (ParamBlock::new() starts at generation 2 before the thread runs).
        let startup = wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), Duration::from_secs(2));
        assert!(startup.is_some(), "startup ModelChanged must arrive");
        let mut warmup = DspParams::DEFAULT;
        let mut warm_gen = 0u64;
        assert!(
            block.load_if_changed(&mut warm_gen, &mut warmup),
            "startup snapshot must be visible in ParamBlock"
        );
        assert!(
            (warmup.denoise_mix - 0.5).abs() < f32::EPSILON,
            "startup snapshot is neutral (macro 0.5)"
        );
        let t0 = Instant::now();
        let sent = handle.send(Command::SetMacro(Macro::Purity, 0.1));
        assert!(sent.is_ok(), "send must succeed while the handle is alive");
        let mut visible = None;
        let mut params = DspParams::DEFAULT;
        let mut last_gen = warm_gen;
        while t0.elapsed() < Duration::from_secs(2) {
            let mut probe = DspParams::DEFAULT;
            if block.load_if_changed(&mut last_gen, &mut probe) {
                params = probe;
                visible = Some(t0.elapsed());
                break;
            }
            std::thread::sleep(Duration::from_micros(50));
        }
        assert!(visible.is_some(), "SetMacro must reach the ParamBlock");
        assert!(
            (params.denoise_mix - 0.1).abs() < f32::EPSILON,
            "expected denoise_mix ~= 0.1, got {} (warm_gen={warm_gen}, last_gen={last_gen})",
            params.denoise_mix
        );
        drop(handle);
        drop(rx);
    }
}
