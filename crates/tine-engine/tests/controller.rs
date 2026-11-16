//! Controller integration tests: command matrix, pin semantics (D-007),
//! persistence debounce, boot-params continuity, and the full
//! controller → shared-memory path against a local (session) mapping that
//! stands in for the APO.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tine_engine::controller::{Command, Controller, ControllerHandle, EngineConfig, Event};
use tine_engine::model::{AppModel, EndpointId, KeepPins, Macro, MonitorSettings};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;
use tine_engine::shm_client::{ShmTarget, Target};
use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, MacroPositions};
use tine_protocol::shm::{ShmLayout, init_layout_in_place};
use tine_protocol::sync::ParamBlock;
use tine_protocol::telemetry::AudioTelemetryFrame;
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};

fn temp_paths(tag: &str) -> EnginePaths {
    let base = tempfile::tempdir().ok().map_or_else(
        || std::env::temp_dir().join(format!("tine-ctl-{tag}")),
        tempfile::TempDir::keep,
    );
    let root = base.join(tag);
    EnginePaths::from_roots(root.join("app"), root.join("prog"), root.join("local"))
}

fn calibration_fixture() -> Calibration {
    Calibration {
        noise_rms_db: -61.0,
        noise_peak_db: -55.5,
        speech_rms_db: -27.5,
        crest_db: 11.0,
        tilt_db: 2.5,
        derived: CalibrationOverrides {
            input_gain_db: 9.5,
            gate_threshold_db: -55.0,
            denoise_mix: 0.85,
            comp_thresh_db: -16.0,
            comp_makeup_db: 2.5,
        },
    }
}

const BUDGET: Duration = Duration::from_secs(5);

/// Waits until an event matching `pred` arrives (discarding others), or `None`
/// if the budget expires / channel closes.
fn wait_event(rx: &Receiver<Event>, pred: impl Fn(&Event) -> bool, budget: Duration) -> Option<Event> {
    let start = Instant::now();
    while start.elapsed() < budget {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(e) if pred(&e) => return Some(e),
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
    None
}

/// Waits for a `ModelChanged` snapshot whose model satisfies `pred`.
fn wait_model(rx: &Receiver<Event>, pred: impl Fn(&AppModel) -> bool, what: &str) -> Option<Arc<AppModel>> {
    let outcome = wait_event(
        rx,
        |e| match e {
            Event::ModelChanged(m) => pred(m),
            _ => false,
        },
        BUDGET,
    );
    match outcome {
        Some(Event::ModelChanged(m)) => Some(m),
        Some(_) => None,
        None => {
            let _ = what;
            None
        }
    }
}

/// Waits for a `ParamsResolved` snapshot satisfying `pred` (panics with
/// `what` on timeout).
fn wait_params(rx: &Receiver<Event>, pred: impl Fn(&DspParams) -> bool, what: &str) -> DspParams {
    match wait_event(
        rx,
        |e| match e {
            Event::ParamsResolved(p) => pred(p),
            _ => false,
        },
        BUDGET,
    ) {
        Some(Event::ParamsResolved(p)) => p,
        _ => panic!("timed out waiting for params: {what}"),
    }
}

fn spawn_block(endpoint: &str, tag: &str) -> (ControllerHandle, Receiver<Event>, EnginePaths) {
    let paths = temp_paths(tag);
    let cfg = EngineConfig::block(EndpointId::new(endpoint), Arc::new(ParamBlock::new()), paths.clone());
    let (handle, rx) = Controller::spawn(cfg);
    (handle, rx, paths)
}

#[test]
fn every_command_reaches_the_model_and_the_param_target() {
    let (handle, rx, _paths) = spawn_block("mic-int", "commands");
    // Startup events.
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some(), "startup ModelChanged");

    let commands = [
        Command::SetArchetype(Archetype::Headset),
        Command::SetMacro(Macro::Purity, 0.9),
        Command::SetMacro(Macro::Body, 0.1),
        Command::SetMacro(Macro::Presence, 0.8),
        Command::SetMacro(Macro::Leveler, 0.75),
        Command::SetParam(ParamId::HpfFreqHz, 95.0),
        Command::Unpin(ParamId::HpfFreqHz),
        Command::SetParam(ParamId::CompRatio, 6.0),
        Command::SetBypass(true),
        Command::SetBypass(false),
        Command::UnpinAll,
        Command::SetMonitor(MonitorSettings {
            output_device: Some("hp-1".to_owned()),
            enabled: true,
        }),
        Command::ApplyCalibration(calibration_fixture(), KeepPins::Clear),
    ];
    let mut last_params = DspParams::DEFAULT;
    for cmd in &commands {
        let sent = handle.send(cmd.clone());
        assert!(sent.is_ok(), "send {cmd:?}");
        // Every command resolves + publishes params, then emits the model.
        last_params = wait_params(&rx, |_| true, "ParamsResolved per command");
        let model = wait_event(&rx, |e| matches!(e, Event::ModelChanged(_)), BUDGET);
        assert!(model.is_some(), "ModelChanged after {cmd:?}");
    }
    let p = last_params;
    // Calibration drove the Auto-Tune params (`KeepPins::Clear`).
    assert_eq!(p.input_gain_db, 9.5);
    assert_eq!(p.comp_makeup_db, 2.5);
    assert!(!p.bypass);
    // D-021: the calibrated denoise_mix 0.85 is now the anchor the Purity
    // macro (left at 0.9) swings around: 0.85 + 0.4 = 1.25, clamped by SPECS
    // to 1.0 — the calibrated value is reproduced exactly only at m = 0.5.
    assert!((p.denoise_mix - 1.0).abs() < 1e-6);
    assert!(
        (p.gate_threshold_db - (-43.0)).abs() < 1e-3,
        "calibrated -55 dB + 0.8 * 15 dB span"
    );

    drop(handle);
    drop(rx);
}

#[test]
fn set_macro_pins_and_unpin_restores_macro_control() {
    let (handle, rx, _paths) = spawn_block("mic-pins", "pins");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    // Pin denoise_mix via a Pro-mode edit (D-007).
    let sent = handle.send(Command::SetParam(ParamId::DenoiseMix, 0.25));
    assert!(sent.is_ok());
    wait_params(&rx, |p| (p.denoise_mix - 0.25).abs() < 1e-6, "pinned denoise_mix");

    // Moving Purity must move its other param (gate threshold) while the
    // pinned parameter stays put.
    let sent = handle.send(Command::SetMacro(Macro::Purity, 1.0));
    assert!(sent.is_ok());
    // The assertion runs inside `wait_params`: with Purity at 1.0 the gate
    // threshold (Dynamic base -50 dB + half of D-021's 30 dB span) reaches
    // ~-35 dB while the pinned denoise_mix stays at 0.25.
    wait_params(
        &rx,
        |p| (p.denoise_mix - 0.25).abs() < 1e-6 && p.gate_threshold_db > -36.5 && p.gate_threshold_db < -33.5,
        "purity at 1.0, denoise still pinned",
    );

    // Unpin returns control to the macro (purity 1.0 => denoise 1.0).
    let sent = handle.send(Command::Unpin(ParamId::DenoiseMix));
    assert!(sent.is_ok());
    wait_params(&rx, |p| (p.denoise_mix - 1.0).abs() < 1e-6, "macro drives again");

    drop(handle);
    drop(rx);
}

#[test]
fn macro_midpoint_reproduces_the_archetype_base() {
    let (handle, rx, _paths) = spawn_block("mic-mid", "mid");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    // Rest state (all macros at their 0.5 default) resolves to the preset
    // base curve exactly (resolve at rest == base is also proven by the
    // `model::tests::midpoint_macros_reproduce_the_preset_base` unit test),
    // so the controller publishes the base after its first macro round-trip.
    let base = tine_dsp::presets::builtin(Archetype::Dynamic).base;

    // Moving Purity away from the centre moves its params (D-021 spans)...
    let sent = handle.send(Command::SetMacro(Macro::Purity, 0.0));
    assert!(sent.is_ok());
    let low = wait_params(
        &rx,
        |p| (p.denoise_mix - 0.0).abs() < 1e-6 && (p.gate_threshold_db - (-65.0)).abs() < 1e-3,
        "purity 0.0 lands base - span/2",
    );
    assert_ne!(low, base);

    // ...and centreing all macros again reproduces the base *exactly*.
    let sent = handle.send(Command::SetMacro(Macro::Purity, 0.5));
    assert!(sent.is_ok());
    wait_params(&rx, |p| *p == base, "m back to 0.5 reproduces the base");

    drop(handle);
    drop(rx);
}

#[test]
fn archetype_switch_keeps_pins_and_overrides() {
    let (handle, rx, _paths) = spawn_block("mic-arch", "archetype");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    let sent = handle.send(Command::SetParam(ParamId::InputGainDb, 6.0));
    assert!(sent.is_ok());
    let pinned = wait_model(&rx, |m| m.profile.pins.is_pinned(ParamId::InputGainDb), "pin");
    assert!(pinned.is_some());

    let sent = handle.send(Command::SetArchetype(Archetype::Condenser));
    assert!(sent.is_ok());
    let model = wait_model(&rx, |m| m.profile.archetype == Archetype::Condenser, "archetype");
    assert!(model.is_some(), "archetype switch emits");
    let Some(model) = model else {
        panic!("expected ModelChanged");
    };
    assert!(model.profile.pins.is_pinned(ParamId::InputGainDb), "pins survive D-007");
    assert_eq!(model.profile.overrides.input_gain_db, Some(6.0));

    drop(handle);
    drop(rx);
}

#[test]
fn apply_calibration_respects_keep_pins() {
    let (handle, rx, _paths) = spawn_block("mic-cal", "calibration");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    // Pin a param calibration will drive; then apply with KeepPins::Clear.
    let sent = handle.send(Command::SetParam(ParamId::InputGainDb, 12.0));
    assert!(sent.is_ok());
    let pinned = wait_model(&rx, |m| m.profile.pins.is_pinned(ParamId::InputGainDb), "pin");
    assert!(pinned.is_some());

    let sent = handle.send(Command::ApplyCalibration(calibration_fixture(), KeepPins::Clear));
    assert!(sent.is_ok());
    // `ParamsResolved` is emitted before `ModelChanged` for each command: read
    // the calibrated value first, then check the pin release in the model.
    wait_params(&rx, |p| (p.input_gain_db - 9.5).abs() < f32::EPSILON, "calibrated gain");
    let cleared = wait_model(
        &rx,
        |m| m.profile.calibration.is_some() && !m.profile.pins.is_pinned(ParamId::InputGainDb),
        "calibration cleared the pin",
    );
    assert!(cleared.is_some(), "calibration must release the pin it now drives");

    // KeepPins::Keep leaves pins alone.
    let sent = handle.send(Command::SetParam(ParamId::CompRatio, 7.0));
    assert!(sent.is_ok());
    let pinned = wait_model(&rx, |m| m.profile.pins.is_pinned(ParamId::CompRatio), "pin ratio");
    assert!(pinned.is_some());
    let sent = handle.send(Command::ApplyCalibration(calibration_fixture(), KeepPins::Keep));
    assert!(sent.is_ok());
    let kept = wait_model(
        &rx,
        |m| m.profile.calibration.is_some() && m.profile.pins.is_pinned(ParamId::CompRatio),
        "keep",
    );
    assert!(kept.is_some(), "`Keep` must keep pins");

    drop(handle);
    drop(rx);
}

#[test]
fn reset_to_archetype_clears_everything() {
    let (handle, rx, _paths) = spawn_block("mic-reset", "reset");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    let sent = handle.send(Command::SetParam(ParamId::HpfFreqHz, 120.0));
    assert!(sent.is_ok());
    let sent = handle.send(Command::SetMacro(Macro::Purity, 0.1));
    assert!(sent.is_ok());
    let sent = handle.send(Command::ApplyCalibration(calibration_fixture(), KeepPins::Keep));
    assert!(sent.is_ok());
    let settled = wait_model(
        &rx,
        |m| (m.profile.macros.purity - 0.1).abs() < 1e-6 && m.profile.calibration.is_some(),
        "dirty state",
    );
    assert!(settled.is_some());

    let sent = handle.send(Command::ResetToArchetype);
    assert!(sent.is_ok());
    let reset = wait_model(
        &rx,
        |m| {
            m.profile.macros == MacroPositions::default()
                && m.profile.calibration.is_none()
                && m.profile.pins.is_empty()
                && m.profile.overrides.eq.is_empty()
        },
        "reset state",
    );
    assert!(
        reset.is_some(),
        "ResetToArchetype clears macros, calibration, pins, and overrides"
    );

    drop(handle);
    drop(rx);
}

#[test]
fn persistence_is_debounced_and_flushed_on_exit() {
    let (handle, rx, paths) = spawn_block("mic-persist", "persist-debounce");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    let file = paths.profile_file("mic-persist");
    assert!(!file.exists(), "no profile write before any command");

    // Burst of slider-drag commands: one coalesced write expected.
    for i in 0..20_u32 {
        let sent = handle.send(Command::SetMacro(Macro::Purity, i as f32 * 0.05));
        assert!(sent.is_ok());
    }
    std::thread::sleep(Duration::from_millis(150));
    // 500 ms *trailing* debounce: still nothing on disk...
    assert!(!file.exists(), "no write until the 500 ms debounce expires");
    // ...then the coalesced write lands.
    let mut appeared = false;
    let start = Instant::now();
    while start.elapsed() < BUDGET {
        if file.exists() {
            appeared = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(appeared, "debounced profile write landed");

    // A fresh command re-schedules; exiting inside the debounce window must
    // still flush it.
    let sent = handle.send(Command::SetArchetype(Archetype::Laptop));
    assert!(sent.is_ok());
    let seen = wait_model(&rx, |m| m.profile.archetype == Archetype::Laptop, "laptop");
    assert!(seen.is_some());
    std::thread::sleep(Duration::from_millis(100));
    drop(handle);
    drop(rx);

    let flushed = wait_flushed(&file, |f| f.contains("laptop"));
    assert!(flushed, "flush-on-exit persisted the pending archetype change");
}

/// Polls for `file` to exist and satisfy `pred` on its text (controller flush
/// is asynchronous w.r.t. the dropping test).
fn wait_flushed(file: &std::path::Path, pred: impl Fn(&str) -> bool) -> bool {
    let start = Instant::now();
    while start.elapsed() < BUDGET {
        if let Ok(text) = std::fs::read_to_string(file)
            && pred(&text)
        {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
fn boot_params_survive_controller_exit_for_the_apo() {
    let (handle, rx, paths) = spawn_block("mic-boot", "boot");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    let sent = handle.send(Command::SetMacro(Macro::Purity, 0.7));
    assert!(sent.is_ok());
    let sent = handle.send(Command::SetParam(ParamId::InputGainDb, 3.5));
    assert!(sent.is_ok());
    wait_params(
        &rx,
        |p| (p.denoise_mix - 0.7).abs() < 1e-6 && (p.input_gain_db - 3.5).abs() < 1e-6,
        "state",
    );

    // "tine.exe dies" — the APO later reads the boot params at LockForProcess.
    drop(handle);
    drop(rx);

    let written = wait_flushed(&paths.boot_params_file("mic-boot"), |_| true);
    assert!(written, "boot params flushed on exit");
    match persist::load_boot_params("mic-boot", &paths) {
        persist::Loaded::Fresh(p) => {
            assert!((p.denoise_mix - 0.7).abs() < f32::EPSILON, "macro survives restart");
            assert_eq!(p.input_gain_db, 3.5, "pinned override survives restart");
        }
        other => panic!("expected Fresh boot params, got {other:?}"),
    }
}

#[test]
fn select_endpoint_loads_or_creates_the_profile_and_retargets() {
    let (handle, rx, paths) = spawn_block("mic-first", "select");
    let ready = wait_model(&rx, |_| true, "startup ModelChanged");
    assert!(ready.is_some());

    // Pre-seed a profile for a second endpoint; selecting it must load it.
    let mut seeded = tine_protocol::presets::EndpointProfile::new("mic-second", Archetype::Laptop);
    seeded.macros.purity = 0.2;
    let saved = persist::save_profile(&seeded, &paths);
    assert!(saved.is_ok());

    let sent = handle.send(Command::SelectEndpoint(EndpointId::new("mic-second")));
    assert!(sent.is_ok());
    let model = wait_model(
        &rx,
        |m| m.endpoint.as_ref().is_some_and(|e| e.as_str() == "mic-second"),
        "selected endpoint",
    );
    assert!(model.is_some(), "endpoint switch emits");
    let Some(model) = model else {
        panic!("expected ModelChanged");
    };
    assert_eq!(model.profile.archetype, Archetype::Laptop, "seeded profile loaded");
    assert!((model.profile.macros.purity - 0.2).abs() < f32::EPSILON);

    drop(handle);
    drop(rx);
}

// ---------------------------------------------------------------------------
// Full controller → shared-memory path with a local "APO" mapping
// ---------------------------------------------------------------------------

/// Session-local mapping that stands in for the APO's `Global\Tine.v1.<id>`
/// mapping (creating `Global\` objects requires `SeCreateGlobalPrivilege`).
struct FakeApo {
    mapping: windows::Win32::Foundation::HANDLE,
    base: *mut c_void,
    len: usize,
}

// SAFETY: `FakeApo` owns page-file-backed mapping handles; all access goes
// through `ShmLayout`'s lock-free atomics (single producer for telemetry and
// the tap, generation-stamped params). Handles stay valid until `Drop`, which
// runs exactly once.
unsafe impl Send for FakeApo {}
// SAFETY: See `Send`: cross-thread use is the shm access pattern itself.
unsafe impl Sync for FakeApo {}

impl FakeApo {
    fn create(name: &str) -> Self {
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let len = std::mem::size_of::<ShmLayout>();
        // SAFETY: `wide` is NUL-terminated; INVALID_HANDLE_VALUE selects the
        // system page file; size matches the layout exactly.
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
            Err(e) => panic!("CreateFileMappingW failed: {e}"),
        };
        // SAFETY: handle from CreateFileMappingW with matching size.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) };
        assert!(!view.Value.is_null(), "MapViewOfFile returned null");
        // SAFETY: fresh RW view of exactly `len` bytes, exclusively owned.
        let initialized = unsafe {
            init_layout_in_place(
                view.Value.cast::<u8>(),
                len,
                *b"fakeapo\0\0\0\0\0\0\0\0\0",
                48_000,
                1,
                &DspParams::DEFAULT,
            )
        };
        assert!(initialized.is_ok(), "layout init failed");
        Self {
            mapping,
            base: view.Value,
            len,
        }
    }

    fn layout(&self) -> &ShmLayout {
        // SAFETY: `base`/`len` describe a valid mapping owned by `self`.
        match unsafe { tine_protocol::shm::layout_from_ptr(self.base.cast::<u8>(), self.len) } {
            Ok(l) => l,
            Err(e) => panic!("fake APO layout failed validation: {e}"),
        }
    }

    fn beat_and_publish(&self, seq: u64) {
        let layout = self.layout();
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_millis() as u64);
        layout.header.heartbeat.store(now, Ordering::Relaxed);
        let frame = AudioTelemetryFrame {
            sequence: seq,
            output_rms_db: -18.0,
            ..AudioTelemetryFrame::default()
        };
        layout.telemetry.publish(&frame);
    }

    fn push_tap(&self, samples: &[f32]) -> usize {
        self.layout().tap.push_slice(samples)
    }
}

impl Drop for FakeApo {
    fn drop(&mut self) {
        let view = windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base };
        // SAFETY: view from `MapViewOfFile`, unmapped exactly once.
        let _ = unsafe { UnmapViewOfFile(view) };
        if !self.mapping.is_invalid() {
            // SAFETY: handle from `CreateFileMappingW`; closed exactly once.
            let _ = unsafe { CloseHandle(self.mapping) };
        }
    }
}

fn unique_mapping_name(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("Local\\Tine.v1.ctl-{tag}-{n}-{}", std::process::id())
}

#[test]
fn controller_links_reconnects_and_serves_telemetry_over_shm() {
    let name = unique_mapping_name("link");
    let paths = temp_paths("shm-link");
    // Controller starts before the APO exists: unlinked, retrying at 1 Hz.
    let target = Target::Shm(ShmTarget::new(Some(EndpointId::new("apo-ep"))).with_name(&name));
    let cfg = EngineConfig {
        endpoint: Some(EndpointId::new("apo-ep")),
        target,
        paths: paths.clone(),
    };
    let (handle, rx) = Controller::spawn(cfg);

    // The APO (re)creates the mapping after ~300 ms.
    std::thread::sleep(Duration::from_millis(300));
    let apo = Arc::new(FakeApo::create(&name));
    let beater_apo = Arc::clone(&apo);
    let beater = std::thread::spawn(move || {
        for seq in 1..=80_u64 {
            beater_apo.beat_and_publish(seq);
            std::thread::sleep(Duration::from_millis(50));
        }
    });
    let linked = wait_event(
        &rx,
        |e| matches!(e, Event::ApoLink(tine_engine::shm_client::LinkState::Linked { .. })),
        BUDGET,
    );
    assert!(
        linked.is_some(),
        "controller must link once the APO creates the mapping"
    );
    let _ = beater.join();

    // A SetMacro command must be published into the mapping — audible in
    // every app using the mic well within one frame (50 ms budget).
    apo.beat_and_publish(100);
    let t0 = Instant::now();
    let sent = handle.send(Command::SetMacro(Macro::Purity, 0.8));
    assert!(sent.is_ok());
    let mut last = 0u64;
    let params_ptr: *const ParamBlock = &raw const apo.layout().params;
    let mut gained = false;
    while t0.elapsed() < BUDGET {
        // SAFETY: `apo` (an Arc alive for the whole test) keeps the mapping
        // mapped; the ParamBlock is read here exactly like the APO's own
        // reader would (generation Acquire), from this thread only.
        let shared = unsafe { &*params_ptr };
        let mut probe = DspParams::DEFAULT;
        if shared.load_if_changed(&mut last, &mut probe) && (probe.denoise_mix - 0.8).abs() < f32::EPSILON {
            gained = true;
            break;
        }
        std::thread::sleep(Duration::from_micros(50));
    }
    assert!(gained, "SetMacro must reach the shared ParamBlock");
    let total_ms = t0.elapsed().as_millis();
    println!("shm param propagation: command -> shared ParamBlock visible in ~{total_ms} ms (budget 50 ms)");
    assert!(
        total_ms < 50,
        "param round-trip must be well inside the 50 ms audibility budget (was {total_ms} ms)"
    );

    // Telemetry + tap served through the handle (lock-free paths).
    let frame = telemetry_with_sequence(&handle);
    assert!(frame > 0, "telemetry flows from shm through the handle");
    let taps = [0.5_f32, 0.25];
    apo.push_tap(&taps);
    let mut drained = [0.0_f32; 16];
    let mut got = 0usize;
    let start = Instant::now();
    while got == 0 && start.elapsed() < BUDGET {
        got = handle.drain_tap(&mut drained);
        if got == 0 {
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    assert_eq!(got, 2, "tap samples drain through the handle");
    assert!((drained[0] - 0.5).abs() < f32::EPSILON);

    assert!(
        handle.shm_client().is_some(),
        "handle exposes the live client for the visualizer"
    );

    drop(handle);
    drop(rx);
    drop(apo);
    let _ = paths;
}

fn telemetry_with_sequence(handle: &ControllerHandle) -> u64 {
    let start = Instant::now();
    while start.elapsed() < BUDGET {
        let frame = handle.telemetry();
        if frame.sequence > 0 {
            return frame.sequence;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    0
}
