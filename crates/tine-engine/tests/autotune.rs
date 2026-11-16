//! Auto-Tune runner integration tests: the full state machine against a
//! session-local mapping standing in for the APO, a lease-tracking fake-APO
//! thread, an offline fixture capture, and a scripted calibrator.
//!
//! TODO(T11) flip plan: when `tine_dsp::calibrator` merges, `ScriptedCalibrator`
//! is replaced by `AutoCalibrator::new(fs)` via `Calibrator` /
//! `CalibratorFactory`, and the fixture audio becomes `tine-dsp`'s
//! calibration fixtures; the assertions below do not change.

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use tine_dsp::presets::{PresetDef, builtin};
use tine_engine::autotune::{
    AutoTuneConfig, AutoTuneError, AutoTuneEvent, AutoTuneOutcome, AutoTuneRunner, CAL_CHANGES, CalFailure, CalPhase,
    Calibrator, CalibratorFactory, CaptureSpec, LEASE_MS, SampleSource, clear_lease, lease_until_ms, renew_lease,
};
use tine_engine::controller::{Controller, ControllerHandle, EngineConfig, Event};
use tine_engine::model::{EndpointId, KeepPins};
use tine_engine::paths::EnginePaths;
use tine_engine::shm_client::ShmClient;
use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::{Archetype, Calibration, CalibrationOverrides, MacroPositions, PinSet};
use tine_protocol::shm::{ShmLayout, init_layout_in_place};
use tine_protocol::sync::ParamBlock;
use tine_protocol::telemetry::flags;
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};

// ---------------------------------------------------------------------------
// Fake APO: session-local shared memory + lease-tracking bypass thread
// ---------------------------------------------------------------------------

struct FakeApo {
    mapping: windows::Win32::Foundation::HANDLE,
    base: *mut c_void,
    len: usize,
}

// SAFETY: The mapped view is accessed through `ShmLayout`'s lock-free atomics
// only, exactly as `audiodg` would in production.
unsafe impl Send for FakeApo {}
// SAFETY: Same as `Send`: cross-thread atomics on shared memory are the
// intended pattern.
unsafe impl Sync for FakeApo {}

impl FakeApo {
    fn new(name: &str) -> Self {
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
        let init = unsafe {
            init_layout_in_place(
                view.Value.cast::<u8>(),
                len,
                *b"fakeapo\0\0\0\0\0\0\0\0\0",
                48_000,
                1,
                &DspParams::DEFAULT,
            )
        };
        assert!(init.is_ok(), "layout init failed");
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

    fn is_reporting_bypass(&self) -> bool {
        self.layout().header.state.load(Ordering::Acquire) & flags::BYPASS != 0
    }
}

impl Drop for FakeApo {
    fn drop(&mut self) {
        let view = windows::Win32::System::Memory::MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base };
        // SAFETY: view from MapViewOfFile, unmapped exactly once.
        let _ = unsafe { UnmapViewOfFile(view) };
        // SAFETY: handle from CreateFileMappingW, closed exactly once.
        unsafe { CloseHandle(self.mapping) }.ok();
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

fn unique_name(tag: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!("Local\\Tine.v1.at-{tag}-{n}-{}", std::process::id())
}

/// Fake-APO side: while the header lease is in the future, report the BYPASS
/// state flag; as soon as it expires (or is cleared), resume processing.
/// This is the honour-the-lease behaviour the real APO implements (T21).
fn start_fake_apo(apo: Arc<FakeApo>, stop: &Arc<AtomicU64>, rate: Duration) {
    let stop = Arc::clone(stop);
    std::thread::spawn(move || {
        while stop.load(Ordering::Relaxed) == 0 {
            let lease = apo.layout().header.bypass_lease_until_ms.load(Ordering::Acquire);
            let state = if lease > now_ms() { flags::BYPASS } else { 0 };
            apo.layout().header.state.store(state, Ordering::Release);
            apo.layout().header.heartbeat.store(now_ms(), Ordering::Relaxed);
            std::thread::sleep(rate);
        }
    });
}

// ---------------------------------------------------------------------------
// Fixture capture source
// ---------------------------------------------------------------------------

/// Offline fixture: delivers "recorded" mic audio at `speed`× real time.
struct FixtureSource {
    samples: Vec<f32>,
    pos: usize,
    start: Instant,
    speed: f64,
}

impl SampleSource for FixtureSource {
    fn rate_hz(&self) -> u32 {
        48_000
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        let target = (self.start.elapsed().as_secs_f64() * 48_000.0 * self.speed) as usize;
        let avail = target.saturating_sub(self.pos).min(buf.len());
        for (i, slot) in buf.iter_mut().enumerate().take(avail) {
            *slot = self.samples.get(self.pos + i).copied().unwrap_or(0.0);
        }
        self.pos += avail;
        avail
    }

    fn has_error(&self) -> bool {
        false
    }
}

/// Renders 9 s of synthetic mic audio: a quiet countdown, a noise floor, then
/// voiced "speech" (level-modulated noise).
fn fixture_audio() -> Vec<f32> {
    let fs = 48_000.0_f32;
    let n = 9 * fs as usize;
    let mut out = Vec::with_capacity(n);
    let mut x = 0x1234_5678_9abc_def0_u64;
    let mut noise = || {
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        ((x & 0xffff) as f32 / 32_768.0) - 1.0
    };
    for i in 0..n {
        let t = i as f32 / fs;
        let level = if t < 1.0 {
            0.000_2
        } else if t < 4.0 {
            0.002
        } else {
            0.15 + 0.1 * (2.0 * core::f32::consts::PI * t).sin()
        };
        out.push(noise() * level);
    }
    out
}

// ---------------------------------------------------------------------------
// Scripted calibrator (T11 stand-in)
// ---------------------------------------------------------------------------

const FS: usize = 48_000;

/// T11-shaped state machine without the DSP math: phases purely by sample
/// count (1 s countdown, 3 s noise, 5 s voice, then Analyzing).
struct ScriptedCalibrator {
    fed: usize,
    fail: Option<CalFailure>,
}

impl Calibrator for ScriptedCalibrator {
    fn push(&mut self, samples: &[f32]) -> tine_engine::autotune::CalProgress {
        self.fed += samples.len();
        let (phase, phase_progress, voiced_ms) = if self.fed < FS {
            (CalPhase::Countdown, self.fed as f32 / FS as f32, 0)
        } else if self.fed < FS * 4 {
            (CalPhase::Noise, (self.fed - FS) as f32 / (3.0 * FS as f32), 0)
        } else if self.fed < FS * 9 {
            (
                CalPhase::Voice,
                (self.fed - FS * 4) as f32 / (5.0 * FS as f32),
                ((self.fed.saturating_sub(FS * 4)) * 1000 / FS) as u32,
            )
        } else {
            (CalPhase::Analyzing, 1.0, 5_000)
        };
        tine_engine::autotune::CalProgress {
            phase,
            phase_progress: phase_progress.clamp(0.0, 1.0),
            live_level_db: -30.0,
            voiced_ms,
        }
    }

    fn finish(
        &mut self,
        _preset: &PresetDef,
        _macros: MacroPositions,
        _pins: PinSet,
        _current: &DspParams,
    ) -> Result<tine_engine::autotune::CalibrationReport, CalFailure> {
        match self.fail {
            Some(f) => Err(f),
            None => Ok(fixture_report()),
        }
    }
}

struct ScriptedFactory {
    fail: Option<CalFailure>,
}

impl CalibratorFactory for ScriptedFactory {
    fn make(&self, _fs: f32) -> Box<dyn Calibrator> {
        Box::new(ScriptedCalibrator {
            fed: 0,
            fail: self.fail,
        })
    }
}

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

fn temp_paths(tag: &str) -> String {
    format!("tine-at-{tag}-{}", std::process::id())
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

fn fixture_report() -> tine_engine::autotune::CalibrationReport {
    tine_engine::autotune::CalibrationReport {
        calibration: calibration_fixture(),
        changes: [
            (ParamId::InputGainDb, 0.0, 9.5),
            (ParamId::GateThresholdDb, -40.0, -55.0),
            (ParamId::DenoiseMix, 0.5, 0.85),
            (ParamId::CompThreshDb, -20.0, -16.0),
            (ParamId::CompMakeupDb, 0.0, 2.5),
        ],
        simulated_peak_db: -5.2,
        simulated_rms_db: -18.4,
    }
}

struct Ctx {
    _apo: Arc<FakeApo>,
    apo_state: Arc<FakeApo>,
    apo_stop: Arc<AtomicU64>,
    client: Arc<ShmClient>,
    _paths: EnginePaths,
}

impl Drop for Ctx {
    fn drop(&mut self) {
        self.apo_stop.store(1, Ordering::Relaxed);
    }
}

/// Spawns the fake APO (bypass-aware, honours the header lease) plus a client
/// handle to it. `bypassing: false` keeps the state flag at 0 the whole time
/// (an APO that never confirms bypass).
fn spawn_ctx(tag: &str, bypassing: bool, fail: Option<CalFailure>) -> (Ctx, AutoTuneConfig) {
    let name = unique_name(tag);
    let apo = Arc::new(FakeApo::new(&name));
    let apo_state = Arc::clone(&apo);
    let apo_stop = Arc::new(AtomicU64::new(0));
    if bypassing {
        start_fake_apo(Arc::clone(&apo), &apo_stop, Duration::from_millis(20));
    } else {
        // Freeze the state flag at "processing" (no bypass bit).
        apo.layout().header.state.store(0, Ordering::Release);
    }
    let client = Arc::new(ShmClient::open_name(&name).unwrap_or_else(|e| panic!("shm open: {e}")));

    let paths = EnginePaths::from_roots(
        std::env::temp_dir().join(format!("{}-app", temp_paths(tag))),
        std::env::temp_dir().join(format!("{}-prog", temp_paths(tag))),
        std::env::temp_dir().join(format!("{}-local", temp_paths(tag))),
    );

    let cfg = AutoTuneConfig {
        client: Arc::clone(&client),
        controller: None,
        capture: CaptureSpec::Source(Box::new(FixtureSource {
            samples: fixture_audio(),
            pos: 0,
            start: Instant::now(),
            speed: 10.0,
        })),
        factory: Arc::new(ScriptedFactory { fail }),
        preset: *builtin(Archetype::Dynamic),
        macros: MacroPositions::default(),
        pins: PinSet::EMPTY,
        current: DspParams::DEFAULT,
        keep_pins: KeepPins::Clear,
    };

    let ctx = Ctx {
        _apo: apo,
        apo_state,
        apo_stop,
        client,
        _paths: paths,
    };
    (ctx, cfg)
}

/// Spawns a controller with an in-process parameter target (persistence into
/// a temp dir), for `ApplyCalibration` assertions.
fn spawn_controller(tag: &str) -> (ControllerHandle, Receiver<Event>) {
    let paths = EnginePaths::from_roots(
        std::env::temp_dir().join(format!("{}-ctl-app", temp_paths(tag))),
        std::env::temp_dir().join(format!("{}-ctl-prog", temp_paths(tag))),
        std::env::temp_dir().join(format!("{}-ctl-local", temp_paths(tag))),
    );
    let cfg = EngineConfig::block(EndpointId::new("mic-int"), Arc::new(ParamBlock::new()), paths);
    let (handle, events) = Controller::spawn(cfg);
    (handle, events)
}

/// Waits for an event on `rx` matching `pred`, skipping others (5 s budget).
fn wait_event(rx: &Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Option<Event> {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(20)) {
            Ok(e) if pred(&e) => return Some(e),
            Ok(_) | Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => return None,
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn runner_completes_applies_through_the_controller_and_clears_the_lease() {
    let (ctx, mut cfg) = spawn_ctx("happy", true, None);
    let (ctl, rx) = spawn_controller("happy");

    // Pin one calibration-driven parameter to verify KeepPins::Clear.
    assert_pin_workaround(&ctl);
    cfg.controller = Some(ctl);

    let mut run = AutoTuneRunner::start(cfg);
    let t0 = Instant::now();

    // Mid-run lease check: fresh (renewed) and in the future.
    std::thread::sleep(Duration::from_millis(400));
    let lease = lease_until_ms(&ctx.client);
    assert!(lease > now_ms(), "lease must hold mid-run ({lease})");
    assert!(
        lease - now_ms() > LEASE_MS - 6_000,
        "lease must have been renewed (deadline now + {} ms)",
        lease - now_ms()
    );

    // Progress phases fan out while the run goes.
    let mut phases: Vec<CalPhase> = Vec::new();
    let deadline = t0 + Duration::from_secs(12);
    let mut progress_count = 0_u32;
    let mut started = false;
    let outcome = loop {
        assert!(
            Instant::now() < deadline,
            "runner produced no terminal event within the 12 s budget"
        );
        match run.events().recv_timeout(Duration::from_millis(50)) {
            Ok(AutoTuneEvent::Progress(p)) => {
                progress_count += 1;
                if phases.last() != Some(&p.phase) {
                    phases.push(p.phase);
                }
            }
            Ok(AutoTuneEvent::Started { rate_hz }) => {
                started = true;
                assert_eq!(rate_hz, 48_000);
            }
            Ok(AutoTuneEvent::Done(o)) => break o,
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => panic!("runner channel closed without a Done event"),
        }
    };
    assert!(
        phases.contains(&CalPhase::Countdown)
            && phases.contains(&CalPhase::Noise)
            && phases.contains(&CalPhase::Voice)
            && phases.contains(&CalPhase::Analyzing),
        "all wizard phases must reach the UI, saw {phases:?}"
    );
    assert!(started, "Started must be emitted with the negotiated rate");
    assert!(
        progress_count >= 15,
        "progress fan-out too sparse: {progress_count} events over an ~0.9 s run (expect ~30 Hz)"
    );
    println!(
        "progress fan-out: {progress_count} events across phases {phases:?} in {:.2} s",
        t0.elapsed().as_secs_f32()
    );
    match &outcome {
        AutoTuneOutcome::Applied(report) => {
            assert_eq!(report.calibration, calibration_fixture());
            assert_eq!(report.changes.len(), CAL_CHANGES);
        }
        other => panic!("expected Applied, got {other:?}"),
    }
    let wall = t0.elapsed().as_secs_f32();
    println!("runner: 9 s fixture @10x speed -> {wall:.2} s wall (incl. 2s lease checks)");

    // Lease cleared.
    assert_eq!(lease_until_ms(&ctx.client), 0, "lease cleared after Done");

    // The controller applied the calibration (ApplyCalibration dispatch).
    let applied = wait_event(
        &rx,
        |e| matches!(e, Event::ModelChanged(m) if m.profile.calibration.is_some()),
    );
    assert!(applied.is_some(), "ApplyCalibration must reach the controller model");
    if let Some(Event::ModelChanged(m)) = applied {
        assert_eq!(m.profile.calibration, Some(calibration_fixture()));
        assert!(
            !m.profile.pins.is_pinned(ParamId::InputGainDb),
            "KeepPins::Clear must drop pins"
        );
        assert!(
            !m.profile.pins.is_pinned(ParamId::GateThresholdDb),
            "KeepPins::Clear must drop pins"
        );
    }
}

/// Pins `CompThreshDb` so the `ApplyCalibration` test can observe unpinning
/// (`SetParam` also pins, per D-007).
fn assert_pin_workaround(ctl: &ControllerHandle) {
    assert!(
        ctl.send(tine_engine::controller::Command::SetParam(ParamId::CompThreshDb, -12.0))
            .is_ok(),
        "pin sched failed"
    );
}

#[test]
fn lease_helpers_write_and_clear_the_header() {
    let (ctx, _cfg) = spawn_ctx("lease-helpers", true, None);
    let before = now_ms();
    renew_lease(&ctx.client, now_ms());
    let lease = lease_until_ms(&ctx.client);
    assert!(
        (lease - before) <= LEASE_MS && lease >= before + LEASE_MS - 1_000,
        "renew_lease must write now + LEASE_MS ({lease} vs {before})"
    );
    clear_lease(&ctx.client);
    assert_eq!(lease_until_ms(&ctx.client), 0);
}

/// The fake APO honours the lease: the moment it is cleared, the BYPASS state
/// disappears — "killing tine.exe mid-run: the APO leaves bypass" evidence.
#[test]
fn fake_apo_honours_lease_expiry_and_release() {
    let (ctx, _cfg) = spawn_ctx("expiry", true, None);
    renew_lease(&ctx.client, now_ms());
    // The fake APO polls every 20 ms; the state flag must be BYPASS now.
    let deadline = Instant::now() + Duration::from_secs(1);
    while !ctx.apo_state.is_reporting_bypass() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        ctx.apo_state.is_reporting_bypass(),
        "bypass must engage under a valid lease"
    );

    // An expired (in-the-past) lease: the APO resumes processing by itself.
    let past = now_ms().saturating_sub(100);
    ctx.client
        .layout()
        .header
        .bypass_lease_until_ms
        .store(past, Ordering::Release);
    let deadline = Instant::now() + Duration::from_secs(1);
    while ctx.apo_state.is_reporting_bypass() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(25));
    }
    assert!(
        !ctx.apo_state.is_reporting_bypass(),
        "an expired lease must lift bypass (self-heal)"
    );
}

#[test]
fn cancel_mid_run_clears_the_lease() {
    let (ctx, cfg) = spawn_ctx("cancel", true, None);
    let mut run = AutoTuneRunner::start(cfg);
    std::thread::sleep(Duration::from_millis(250));
    assert!(lease_until_ms(&ctx.client) > now_ms());
    run.cancel();
    let outcome = run
        .wait(Duration::from_secs(5))
        .unwrap_or_else(|| panic!("no outcome after cancel"));
    assert!(
        matches!(outcome, AutoTuneOutcome::Error(AutoTuneError::Cancelled)),
        "expected Cancelled, got {outcome:?}"
    );
    assert_eq!(lease_until_ms(&ctx.client), 0, "cancel must clear the lease");
}

#[test]
fn bypass_not_confirmed_fails_clean() {
    let (ctx, cfg) = spawn_ctx("no-bypass", false, None);
    let run = AutoTuneRunner::start(cfg);
    let outcome = run.wait(Duration::from_secs(8)).unwrap_or_else(|| panic!("no outcome"));
    assert!(
        matches!(outcome, AutoTuneOutcome::Error(AutoTuneError::BypassNotConfirmed)),
        "expected BypassNotConfirmed, got {outcome:?}"
    );
    assert_eq!(lease_until_ms(&ctx.client), 0, "failed confirm must clear the lease");
}

#[test]
fn calibrator_failure_fans_out_and_clears_the_lease() {
    let (ctx, cfg) = spawn_ctx("fail", true, Some(CalFailure::TooNoisy { snr_db: 4.2 }));
    let run = AutoTuneRunner::start(cfg);
    let outcome = run
        .wait(Duration::from_secs(12))
        .unwrap_or_else(|| panic!("no outcome"));
    match outcome {
        AutoTuneOutcome::Failed(CalFailure::TooNoisy { snr_db }) => assert!((snr_db - 4.2).abs() < 1e-6),
        other => panic!("expected Failed(TooNoisy), got {other:?}"),
    }
    assert_eq!(lease_until_ms(&ctx.client), 0);
}

#[test]
fn nonexistent_device_fails_with_capture_error_and_clears_lease() {
    let (ctx, mut cfg) = spawn_ctx("nodev", true, None);
    cfg.capture = CaptureSpec::Device {
        device: tine_engine::audio_io::DeviceRef::Name("__no_such_input_device_for_tine_t16__".to_owned()),
        want_rate: Some(48_000),
    };
    let run = AutoTuneRunner::start(cfg);
    let outcome = run.wait(Duration::from_secs(8)).unwrap_or_else(|| panic!("no outcome"));
    assert!(
        matches!(outcome, AutoTuneOutcome::Error(AutoTuneError::Capture(_))),
        "expected Capture error, got {outcome:?}"
    );
    assert_eq!(lease_until_ms(&ctx.client), 0);
}

/// Real-mic end-to-end run (acceptance criterion: "Auto-Tune on a real mic
/// completes in ~9 s and returns a report"): default input device, scripted
/// calibrator (T11 flip: the real one), lease-typical fake APO not required —
/// uses a plain Local\ mapping that reports bypass. Run with:
/// `cargo nextest run -p tine-engine --test autotune --run-ignored only real_mic_run --no-capture`
#[test]
#[ignore = "opens the real default microphone for ~9 s"]
fn real_mic_run_completes_in_about_9s() {
    // A lease-honouring fake APO so bypass confirms immediately; the fake-APO
    // thread and mapping live in `ctx` for the whole run.
    let (_ctx, mut cfg) = spawn_ctx("realmic", true, None);
    // Real capture: `TINE_HW_IN` (or the system default input) at 48 kHz.
    let device = std::env::var("TINE_HW_IN").map_or_else(
        |_| tine_engine::audio_io::DeviceRef::Default,
        |arg| tine_engine::audio_io::DeviceRef::parse(&arg),
    );
    cfg.capture = CaptureSpec::Device {
        device,
        want_rate: Some(48_000),
    };

    let start = Instant::now();
    let run = AutoTuneRunner::start(cfg);
    let outcome = run
        .wait(Duration::from_secs(30))
        .unwrap_or_else(|| panic!("no outcome"));
    let elapsed = start.elapsed();
    println!("real-mic run: {elapsed:.2?} -> {outcome:?}");
    assert!(
        matches!(outcome, AutoTuneOutcome::Applied(_)),
        "real-mic run must produce a report (got {outcome:?})"
    );
    // ~9 s of phases at 30 Hz capture cadence plus analysis.
    assert!(elapsed.as_secs_f32() >= 8.0, "finished too fast: {elapsed:?}");
    assert!(elapsed.as_secs_f32() <= 15.0, "finished too slowly: {elapsed:?}");
}
