//! T21 end-to-end integration: the full control plane driven headlessly.
//!
//! The harness spawns the real engine `Controller` against an in-process
//! [`ParamBlock`] target, feeds a fake audio source through the T16
//! services (a session-local mapping standing in for the processing
//! side), and drives the **real app bridge** (`EngineBridge`) with
//! `tine-ui` intents — asserting that intents land in the published
//! parameters and in the persisted files (`settings.json`, the endpoint
//! profile, the boot-params file), exactly the flows `tine.exe` runs
//! live (D-017 splits them between the tray parent and the `--ui` child;
//! the bridge is the shared mapping both hosts use).

use std::ffi::c_void;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use tine_app::services;
use tine_app::ui_bridge::{
    AutotuneHooks, BridgeCfg, ComEndpointAdmin, EndpointAdmin, EngineBridge, MonitorHost, nominal_monitor_latency_ms,
};
use tine_engine::apo_admin::{ApoState, CaptureEndpoint, FormFactor};
use tine_engine::autotune::{AutoCalibratorFactory, CaptureSpec, SampleSource};
use tine_engine::controller::{Controller, EngineConfig};
use tine_engine::model::{EndpointId, Macro as EngineMacro, resolve};
use tine_engine::paths::EnginePaths;
use tine_engine::persist;
use tine_engine::shm_client::ShmClient;
use tine_engine::visualizer::InProcessSignals;
use tine_protocol::params::{DspParams, ParamId};
use tine_protocol::presets::Archetype;
use tine_protocol::shm::{ShmLayout, init_layout_in_place};
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::{AudioTelemetryFrame, flags};
use tine_ui::{AutoTuneStep, Intent, Macro};
use windows::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows::Win32::System::Memory::{
    CreateFileMappingW, FILE_MAP_ALL_ACCESS, MEMORY_MAPPED_VIEW_ADDRESS, MapViewOfFile, PAGE_READWRITE, UnmapViewOfFile,
};

const RATE: u32 = 48_000;
const MIC_A: &str = "{mic-a}";
const MIC_B: &str = "{mic-b}";

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

/// Temporary persistence roots for one test.
///
/// The tempdir guard is intentionally leaked: `EnginePaths` files must stay
/// readable until the process exits (nextest isolates every test process,
/// so the OS reclaims the folder afterwards).
fn temp_paths() -> EnginePaths {
    let dir = tempfile::tempdir().expect("tempdir");
    let paths = EnginePaths::from_roots(
        dir.path().join("app"),
        dir.path().join("prog"),
        dir.path().join("local"),
    );
    std::mem::forget(dir);
    paths
}

/// A session-local shared memory standing in for the processing side
/// (fake APO): the mapping the Auto-Tune runner leases, plus a
/// lease-honoring bypass thread like a real APO runs.
struct FakeApo {
    mapping: windows::Win32::Foundation::HANDLE,
    base: *mut c_void,
    len: usize,
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

// SAFETY: the view is only accessed through `ShmLayout`'s lock-free
// atomics, exactly as a real APO would across processes.
unsafe impl Send for FakeApo {}
// SAFETY: same lock-free-atomics-only discipline.
unsafe impl Sync for FakeApo {}

impl FakeApo {
    fn new(tag: &str) -> (Self, String) {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, Ordering::Relaxed);
        let name = format!("Local\\Tine.v1.e2e-{tag}-{n}-{}", std::process::id());
        let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
        let len = std::mem::size_of::<ShmLayout>();
        // SAFETY: `wide` is NUL-terminated; the page file backs the
        // mapping; the size matches `ShmLayout` exactly.
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
        // SAFETY: handle from CreateFileMappingW with a matching size.
        let view = unsafe { MapViewOfFile(mapping, FILE_MAP_ALL_ACCESS, 0, 0, len) };
        assert!(!view.Value.is_null(), "MapViewOfFile returned null");
        // SAFETY: fresh RW view of exactly `len` bytes.
        let init = unsafe {
            init_layout_in_place(
                view.Value.cast::<u8>(),
                len,
                *b"fakeapo\0\0\0\0\0\0\0\0\0",
                RATE,
                1,
                &DspParams::DEFAULT,
            )
        };
        assert!(init.is_ok(), "layout init failed");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        // SAFETY: `view.Value` is a valid mapping kept alive by the
        // owning `FakeApo` for the thread's whole life.
        let layout = unsafe { &*(view.Value as *const ShmLayout) };
        let header = &layout.header;
        let join = std::thread::Builder::new()
            .name("fake-apo-bypass".to_owned())
            .spawn(move || {
                loop {
                    if stop_thread.load(Ordering::Acquire) {
                        break;
                    }
                    let now_ms = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map_or(0, |d| d.as_millis() as u64);
                    let lease = header.bypass_lease_until_ms.load(Ordering::Acquire);
                    let state = header.state.load(Ordering::Relaxed);
                    let wanted = if lease > now_ms { flags::BYPASS } else { 0 };
                    header.state.store((state & !flags::BYPASS) | wanted, Ordering::Release);
                    std::thread::sleep(Duration::from_millis(5));
                }
            })
            .ok();
        (
            Self {
                mapping,
                base: view.Value,
                len,
                stop,
                join,
            },
            name,
        )
    }
}

impl Drop for FakeApo {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
        // SAFETY: view from MapViewOfFile, unmapped exactly once.
        let _ = unsafe { UnmapViewOfFile(MEMORY_MAPPED_VIEW_ADDRESS { Value: self.base }) };
        // SAFETY: mapping handle created by CreateFileMappingW, closed once.
        unsafe { CloseHandle(self.mapping) }.ok();
        let _ = self.len;
    }
}

/// Offline capture feeding the real `AutoCalibrator`: 1 s discard, 3 s of
/// ~−51 dBFS LCG room noise, 5 s of a 150 Hz voice-band sine at −23 dBFS
/// (levels the T11 failure gates accept: SNR ≈ 28 dB, gain in range, no
/// clipping, ≥ 1.5 s voiced).
struct ScriptedSource {
    samples: Vec<f32>,
    pos: usize,
}

impl ScriptedSource {
    #[must_use]
    fn build() -> Self {
        let mut samples = Vec::with_capacity(9 * RATE as usize);
        samples.extend(std::iter::repeat_n(0.0, RATE as usize));
        let noise_rms = 10.0_f32.powf(-51.0 / 20.0);
        let amp = noise_rms * 3.0_f32.sqrt(); // U(-a, a) has rms a/sqrt(3)
        let mut seed = 0x2545_F491_u32;
        for _ in 0..(3 * RATE as usize) {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let unit = (seed >> 8) as f32 / 16_777_216.0 - 1.0;
            samples.push(unit * amp);
        }
        let speech_rms = 10.0_f32.powf(-23.0 / 20.0);
        let peak = speech_rms * 2.0_f32.sqrt();
        for i in 0..(5 * RATE as usize) {
            let t = i as f32 / RATE as f32;
            samples.push(peak * (2.0 * std::f32::consts::PI * 150.0 * t).sin());
        }
        Self { samples, pos: 0 }
    }
}

impl SampleSource for ScriptedSource {
    fn rate_hz(&self) -> u32 {
        RATE
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        let remain = self.samples.len() - self.pos;
        let taken = buf.len().min(remain);
        let src = &self.samples[self.pos..self.pos + taken];
        buf[..taken].copy_from_slice(src);
        self.pos += taken;
        taken
    }

    fn has_error(&self) -> bool {
        false
    }
}

/// Fake endpoint admin: two microphones, an active virtual cable, and a
/// watcher that never fires (tests drive refreshes explicitly).
struct FakeAdmin {
    endpoints: Vec<CaptureEndpoint>,
}

impl EndpointAdmin for FakeAdmin {
    fn list(&self) -> Result<Vec<CaptureEndpoint>, String> {
        Ok(self.endpoints.clone())
    }

    fn health(&self, _endpoint: &str) -> ApoState {
        ApoState::Active { heartbeat_age_ms: 0 }
    }

    fn watch(&self) -> Result<Receiver<tine_engine::apo_admin::DeviceEvent>, String> {
        let (_, rx) = channel();
        Ok(rx)
    }
}

/// Recorder for the "Hear Myself" service.
struct RecordingMonitorHost {
    calls: Mutex<Vec<(bool, Option<String>)>>,
}

impl RecordingMonitorHost {
    fn new() -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
        }
    }

    fn last(&self) -> Option<(bool, Option<String>)> {
        self.calls.lock().ok().and_then(|c| c.last().cloned())
    }
}

impl MonitorHost for RecordingMonitorHost {
    fn apply(&mut self, enabled: bool, output: Option<&str>) -> Result<f32, String> {
        match self.calls.lock() {
            Ok(mut c) => {
                c.push((enabled, output.map(str::to_owned)));
                Ok(41.6)
            }
            Err(_) => Err("poisoned".to_owned()),
        }
    }

    fn stop(&mut self) {}

    fn has_error(&self) -> bool {
        false
    }
}

/// The fake audio the visualizer pumps: a 1 kHz sine at −20 dBFS pushed
/// at real-time cadence plus telemetry frames with a sequence counter.
struct FakeAudio {
    stop: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}

impl FakeAudio {
    fn start() -> (Self, InProcessSignals<16384>) {
        let tap = Arc::new(SpscRing::<16384>::new());
        let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let tap_thread = Arc::clone(&tap);
        let tel_thread = Arc::clone(&telemetry);
        let join = std::thread::Builder::new()
            .name("fake-audio".to_owned())
            .spawn(move || {
                let amplitude = 10.0_f32.powf(-20.0 / 20.0);
                let step = 2.0 * std::f32::consts::PI * 1000.0 / RATE as f32;
                let mut seq = 1_u64;
                let mut phase = 0.0_f32;
                while !stop_thread.load(Ordering::Acquire) {
                    let mut block = [0.0_f32; 480];
                    for s in &mut block {
                        phase = (phase + step) % (2.0 * std::f32::consts::PI);
                        *s = amplitude * phase.sin();
                    }
                    let _ = tap_thread.push_slice(&block);
                    tel_thread.publish(&AudioTelemetryFrame {
                        sequence: seq,
                        input_rms_db: -20.0,
                        output_peak_db: -20.0,
                        ..AudioTelemetryFrame::default()
                    });
                    seq += 1;
                    std::thread::sleep(Duration::from_millis(10));
                }
            })
            .ok();
        let signals = InProcessSignals {
            telemetry,
            tap,
            rate_hz: RATE,
        };
        (Self { stop, join }, signals)
    }
}

impl Drop for FakeAudio {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

/// The whole headless app: controller + bridge + services.
struct Harness {
    bridge: EngineBridge,
    block: Arc<ParamBlock>,
    paths: EnginePaths,
    monitor: Arc<Mutex<RecordingMonitorHost>>,
    _audio: FakeAudio,
}

impl Harness {
    fn new(tag: &str, paths: &EnginePaths, endpoint: &str, client: Option<&Arc<ShmClient>>) -> Self {
        let block = Arc::new(ParamBlock::new());
        let (handle, events) = Controller::spawn(EngineConfig::block(
            EndpointId::new(endpoint.to_owned()),
            Arc::clone(&block),
            paths.clone(),
        ));
        let (audio, signals) = FakeAudio::start();
        let monitor = Arc::new(Mutex::new(RecordingMonitorHost::new()));
        let bridge = EngineBridge::new(BridgeCfg {
            handle,
            events,
            paths: paths.clone(),
            signals: Arc::new(signals),
            admin: Arc::new(FakeAdmin {
                endpoints: vec![
                    CaptureEndpoint {
                        id: MIC_A.to_owned(),
                        name: String::from("Dynamic USB"),
                        form_factor: FormFactor::Microphone,
                        suggested: Archetype::Dynamic,
                        state: ApoState::Active { heartbeat_age_ms: 0 },
                    },
                    CaptureEndpoint {
                        id: MIC_B.to_owned(),
                        name: String::from("Studio Condenser"),
                        form_factor: FormFactor::Microphone,
                        suggested: Archetype::Condenser,
                        state: ApoState::Active { heartbeat_age_ms: 0 },
                    },
                ],
            }),
            monitor: monitor.clone(),
            autotune: AutotuneHooks {
                client: client.cloned(),
                capture: Arc::new(|| CaptureSpec::Source(Box::new(ScriptedSource::build()))),
                factory: Arc::new(AutoCalibratorFactory),
            },
        });
        let _ = tag;
        Self {
            bridge,
            block,
            paths: paths.clone(),
            monitor,
            _audio: audio,
        }
    }

    /// Pumps until `cond` holds or the deadline expires.
    fn pump_until(&mut self, timeout: Duration, mut cond: impl FnMut(&mut Harness) -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        while !cond(self) {
            if Instant::now() >= deadline {
                return false;
            }
            if self.bridge.pump(Duration::from_millis(20)) == 0 {
                std::thread::sleep(Duration::from_millis(10));
            }
        }
        true
    }

    /// Reads the latest published parameter snapshot from the block target.
    fn published(&self) -> DspParams {
        let mut out = DspParams::DEFAULT;
        let mut last_gen = 0;
        assert!(
            self.block.load_if_changed(&mut last_gen, &mut out),
            "the block must hold a published generation"
        );
        out
    }

    /// Waits out the controller's 500 ms persistence debounce and drains.
    fn settle(&mut self) {
        std::thread::sleep(Duration::from_millis(800));
        self.bridge.pump(Duration::from_millis(300));
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn intents_flow_to_params_and_files_and_frames_flow_to_live_state() {
    let paths = temp_paths();
    let mut app = Harness::new("params", &paths, MIC_A, None);
    assert!(app.bridge.first_run(), "fresh profile: first run detected");
    app.bridge.refresh_endpoints();
    app.bridge.set_visible(true);
    assert!(app.bridge.is_visualizing(), "the 60 Hz pump runs while visible");

    // Visualizer frames reach the UI mapping from the fake audio source
    // (`latest_live` is a consume: the newest frame wins inside the pump).
    let mut seen: Option<tine_ui::LiveFrame> = None;
    let voiced = app.pump_until(Duration::from_secs(2), |app| {
        if let Some(live) = app.bridge.latest_live() {
            seen = Some(live);
        }
        seen.as_ref()
            .is_some_and(|live| live.spectrum.iter().copied().fold(0.0_f32, f32::max) > 0.6)
    });
    assert!(voiced, "a -20 dBFS 1 kHz tone must light up a band");
    let live = seen.expect("a coalesced frame");
    let peak_band = live
        .spectrum
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.total_cmp(b))
        .map(|(i, v)| (i, *v))
        .expect("bands");
    assert!(
        (12..=24).contains(&peak_band.0),
        "1 kHz sits mid spectrum (peak band {} at {:.2})",
        peak_band.0,
        peak_band.1
    );
    assert!(live.telemetry.sequence > 0, "the frame carries the live telemetry");
    assert_eq!(tine_app::ui_bridge::band_level(-90.0), 0.0);

    // Macros: intent → model → resolved params on the in-process target.
    app.bridge.dispatch(Intent::SetMacro(Macro::Purity, 0.1));
    assert!(
        app.pump_until(Duration::from_secs(2), |app| app.published().denoise_mix < 0.5),
        "purity 0.1 lowers the denoiser mix"
    );
    let expected_base = app.bridge.app_model().expect("model");
    let mut expected_model = tine_engine::model::AppModel::clone(&**expected_base);
    EngineMacro::Purity.set(&mut expected_model.profile.macros, 0.1);
    assert_eq!(
        app.published(),
        resolve(&expected_model),
        "pipeline snapshot equals resolve()"
    );

    // Pro edit: pins the parameter (D-007) and lands in the profile.
    app.bridge.dispatch(Intent::SetParam(ParamId::DenoiseMix, 0.9));
    assert!(
        app.pump_until(Duration::from_secs(2), |app| app.published().denoise_mix == 0.9),
        "the pinned value stops moving with macros"
    );
    app.settle();
    assert!(app.paths.settings_file().exists(), "settings.json flushed");
    assert!(app.paths.profile_file(MIC_A).exists(), "profile flushed");
    let profile = persist::load_profile(MIC_A, &app.paths).into_value();
    assert!(
        profile.pins.is_pinned(ParamId::DenoiseMix),
        "the Pro edit pinned denoise_mix"
    );
    assert_eq!(profile.overrides.denoise_mix, Some(0.9));
    assert_eq!(profile.macros.purity, 0.1);

    // A/B bypass: the toggle intent flips the engine model.
    app.bridge.dispatch(Intent::ToggleBypass);
    assert!(
        app.pump_until(Duration::from_secs(2), |app| {
            app.bridge.app_model().is_some_and(|m| m.bypass) && app.published().bypass
        }),
        "A/B toggle reaches the published snapshot"
    );

    // Saving settings before OnboardingDone keeps first_run == true.
    assert!(app.bridge.first_run());
    let mut settings = persist::load_settings(&app.paths).into_value();
    settings.onboarding_completed = true;
    persist::save_settings(&settings, &app.paths).expect("save onboarding_completed");
    assert!(!app.bridge.first_run());
}

#[test]
fn monitor_intents_drive_the_service_and_persist_settings() {
    let paths = temp_paths();
    let mut app = Harness::new("monitor", &paths, MIC_A, None);
    app.bridge.set_visible(true);
    assert!(
        app.pump_until(Duration::from_secs(2), |app| app.bridge.app_model().is_some()),
        "controller pushes its first model"
    );

    app.bridge.dispatch(Intent::ToggleMonitor);
    assert!(app.pump_until(Duration::from_secs(2), |app| {
        app.bridge.app_model().is_some_and(|m| m.monitor.enabled)
    }));
    assert!(
        app.monitor
            .lock()
            .ok()
            .and_then(|m| m.last().map(|(on, _)| on))
            .unwrap_or(false),
        "the T16 monitor service was asked to start"
    );
    app.settle();
    assert!(
        persist::load_settings(&app.paths).into_value().monitor_enabled,
        "monitor flag persisted"
    );

    // Latency badge reaches the UI model (the measured 41.6 ms service value).
    let ui = app.bridge.ui_model();
    assert!(ui.monitor.enabled);
    assert_eq!(ui.monitor.estimated_latency_ms, 41.6);

    app.bridge.dispatch(Intent::PickMonitorDevice("Headphones".to_owned()));
    assert!(
        app.pump_until(Duration::from_secs(2), |app| {
            app.monitor
                .lock()
                .ok()
                .and_then(|m| m.last())
                .is_some_and(|(_, d)| d.as_deref() == Some("Headphones"))
        }),
        "the device pick reached the service"
    );
    assert!(app.pump_until(Duration::from_secs(2), |app| {
        app.bridge
            .app_model()
            .is_some_and(|m| m.monitor.output_device.as_deref() == Some("Headphones"))
    }));

    app.bridge.dispatch(Intent::ToggleMonitor);
    assert!(app.pump_until(Duration::from_secs(2), |app| {
        app.bridge.app_model().is_some_and(|m| !m.monitor.enabled)
    }));
    app.settle();
    assert!(
        !persist::load_settings(&app.paths).into_value().monitor_enabled,
        "monitor flag persisted off"
    );
    assert!((nominal_monitor_latency_ms() - 41.6).abs() < 1e-4);
}

#[test]
fn endpoint_selection_creates_and_persists_a_new_profile() {
    let paths = temp_paths();
    let mut app = Harness::new("select", &paths, MIC_A, None);
    app.bridge.refresh_endpoints();
    assert!(
        app.pump_until(Duration::from_secs(2), |app| app.bridge.app_model().is_some()),
        "controller pushes its first model"
    );
    // Unknown endpoint ID: surfaced user message (no silent failure).
    app.bridge
        .dispatch(Intent::SelectEndpoint("nonexistent-mic".to_owned()));
    assert!(
        app.bridge.take_messages().iter().any(|m| m.contains("disappeared")),
        "an unknown endpoint pick surfaces a message, not a panic"
    );

    app.bridge.dispatch(Intent::SelectEndpoint(MIC_B.to_owned()));
    assert!(app.pump_until(Duration::from_secs(2), |app| {
        app.bridge
            .app_model()
            .and_then(|m| m.endpoint.as_ref())
            .is_some_and(|e| e.as_str() == MIC_B)
    }));
    assert_eq!(
        app.bridge.ui_model().selected,
        Some(1),
        "the header picker follows the engine"
    );
    // Parameters re-resolved for the fresh profile (midpoint macros).
    let model = app.bridge.app_model().cloned().expect("model");
    assert_eq!(app.published(), resolve(&model));

    app.settle();
    let profile = persist::load_profile(MIC_B, &app.paths).into_value();
    assert_eq!(profile.endpoint_id, MIC_B, "profile written for the new endpoint");
    assert_eq!(
        persist::load_settings(&app.paths)
            .into_value()
            .selected_endpoint_id
            .as_deref(),
        Some(MIC_B)
    );
}

#[test]
fn autotune_wizard_applies_calibration_through_apply_intent() {
    // Session-local mapping: the lease target for the runner + a
    // lease-honoring fake APO confirming bypass.
    let (apo, name) = FakeApo::new("auto");
    let paths = temp_paths();
    let client = Arc::new(ShmClient::open_name(&name).expect("open mapping"));
    let mut app = Harness::new("auto", &paths, MIC_A, Some(&client));
    app.bridge.set_visible(true);
    assert!(
        app.pump_until(Duration::from_secs(2), |app| app.bridge.app_model().is_some()),
        "controller pushes its first model"
    );
    let uncalibrated = app.bridge.params().gate_threshold_db;

    // Without the results card, Apply is a no-op (nothing to adopt).
    app.bridge.dispatch(Intent::ApplyCalibration { keep_pins: true });
    assert!(app.bridge.pending_calibration().is_none());

    // Full run: the real calibrator consumes the scripted capture.
    app.bridge.dispatch(Intent::StartAutoTune);
    let started = Instant::now();
    assert!(
        app.pump_until(Duration::from_secs(60), |app| {
            matches!(app.bridge.ui_model().autotune.step, AutoTuneStep::Results)
        }),
        "wizard reaches the results card (took {:?})",
        started.elapsed()
    );
    let report = *app.bridge.pending_calibration().expect("report held for the card");
    // Auto-Tune acceptance: −9..−6 dBFS simulated peak; speech +
    // input gain anchored at −20 dBFS; the noisy room drives a high AI mix.
    assert!(
        (-9.0..=-6.0).contains(&report.simulated_peak_db),
        "verified peak {} dBFS",
        report.simulated_peak_db
    );
    let cal = &report.calibration;
    assert!(
        (cal.speech_rms_db + cal.derived.input_gain_db - -20.0).abs() < 1.0,
        "speech landed at -20 dBFS (rms {} + gain {})",
        cal.speech_rms_db,
        cal.derived.input_gain_db
    );
    assert!(cal.derived.denoise_mix >= 0.5, "noisy room drives a high AI mix");

    // PreviewCalibration updates bridge.params() for different VoiceStyles before Apply.
    app.bridge.dispatch(Intent::PreviewCalibration {
        style: tine_dsp::calibrator::VoiceStyle::Broadcast,
        keep_pins: true,
        audition_raw: None,
    });
    let preview_broadcast = *app.bridge.params();
    app.bridge.dispatch(Intent::PreviewCalibration {
        style: tine_dsp::calibrator::VoiceStyle::Crisp,
        keep_pins: true,
        audition_raw: None,
    });
    let preview_crisp = *app.bridge.params();
    assert_ne!(
        preview_broadcast, preview_crisp,
        "Broadcast and Crisp calibration previews must produce distinct DSP parameters"
    );

    // Apply { keep_pins: false }: calibration lands in the profile, pins
    // the calibration now drives are cleared.
    app.bridge.dispatch(Intent::ApplyCalibration { keep_pins: false });
    assert!(
        app.pump_until(Duration::from_secs(2), |app| {
            app.bridge.app_model().is_some_and(|m| m.profile.calibration.is_some())
        }),
        "calibration adopted by the controller"
    );
    let model = app.bridge.app_model().cloned().expect("model");
    assert!(
        !model.profile.pins.is_pinned(ParamId::GateThresholdDb),
        "keep_pins=false released the pin"
    );
    assert_ne!(
        app.bridge.params().gate_threshold_db,
        uncalibrated,
        "the gate moved to noise+6"
    );
    assert!(
        matches!(app.bridge.ui_model().autotune.step, AutoTuneStep::Idle),
        "card closed after Apply"
    );

    // And it persisted for the next boot.
    app.settle();
    assert!(
        persist::load_profile(MIC_A, &app.paths)
            .into_value()
            .calibration
            .is_some(),
        "calibration persisted"
    );
    drop(apo);
}

#[test]
fn state_survives_reboot() {
    let paths = temp_paths();
    {
        let mut app = Harness::new("reboot", &paths, MIC_A, None);
        app.bridge.set_visible(true);
        assert!(app.bridge.first_run());
        app.bridge.dispatch(Intent::SetMacro(Macro::Presence, 0.9));
        assert!(app.pump_until(Duration::from_secs(2), |app| app.bridge.app_model().is_some()));
        app.bridge.dispatch(Intent::SetParam(ParamId::HpfFreqHz, 123.0));
        assert!(
            app.pump_until(Duration::from_secs(2), |app| app.published().hpf_freq_hz == 123.0),
            "pro edit published"
        );
        app.settle();
        assert!(
            app.bridge.first_run(),
            "saving settings before OnboardingDone keeps first_run == true"
        );
        let mut settings = persist::load_settings(&paths).into_value();
        settings.onboarding_completed = true;
        persist::save_settings(&settings, &paths).expect("save onboarding_completed");
        // Scope drop: every `ControllerHandle` goes away → the controller
        // thread flushes and exits (a tine.exe reboot).
    }

    let mut rebooted = Harness::new("reboot", &paths, MIC_A, None);
    assert!(
        !rebooted.bridge.first_run(),
        "onboarding_completed survived: not the first run anymore"
    );
    assert!(rebooted.pump_until(Duration::from_secs(2), |app| app.bridge.app_model().is_some()));
    rebooted.bridge.refresh_endpoints();
    let model = rebooted.bridge.app_model().cloned().expect("model");
    assert_eq!(model.profile.macros.presence, 0.9, "macro position survived");
    assert!(model.profile.pins.is_pinned(ParamId::HpfFreqHz), "pin survived");
    assert_eq!(model.profile.overrides.hpf_freq_hz, Some(123.0), "pro edit survived");
    assert_eq!(
        rebooted.published(),
        resolve(&model),
        "processing continues where it left off"
    );
    assert_eq!(
        persist::load_settings(&paths)
            .into_value()
            .selected_endpoint_id
            .as_deref(),
        Some(MIC_A),
        "endpoint selection survived"
    );
}

#[test]
fn failures_surface_and_the_visualizer_follows_visibility() {
    let paths = temp_paths();
    let mut app = Harness::new("errors", &paths, MIC_A, None);
    app.bridge.refresh_endpoints();

    // Hidden: no pump (closing the window stops the visualizer).
    app.bridge.set_visible(false);
    assert!(!app.bridge.is_visualizing());

    // Auto-Tune without a processing link: user message, no panic.
    app.bridge.dispatch(Intent::StartAutoTune);
    assert!(!app.bridge.autotune_running(), "no runner without a lease target");
    assert!(
        app.bridge
            .take_messages()
            .iter()
            .any(|m| m.contains("Auto-Tune needs a live processing link")),
        "the missing link is surfaced to the user"
    );

    // Unknown endpoint ID: surfaced.
    app.bridge
        .dispatch(Intent::SelectEndpoint("nonexistent-mic".to_owned()));
    assert!(app.bridge.take_messages().iter().any(|m| m.contains("disappeared")));

    // Parent-side service seams behave identically (no mapping/router here).
    services::shutdown();
    services::autotune_start_on(&app.paths);
    assert!(
        services::take_messages()
            .iter()
            .any(|m| m.contains("Auto-Tune needs a live processing link"))
    );
    assert_eq!(services::autotune_state()["flow"], "idle");
    services::monitor_apply_on(&app.paths, true, Some("__nonexistent_output__"));
    assert!(
        services::take_messages()
            .iter()
            .any(|m| m.contains("Hear Myself will start once audio routing is active"))
    );
    services::monitor_apply(false, None);

    // Visible → pump runs again; hidden → torn down (acceptance criteria).
    app.bridge.set_visible(true);
    assert!(app.bridge.is_visualizing());
    app.bridge.set_visible(false);
    assert!(!app.bridge.is_visualizing());
    let _ = ComEndpointAdmin.list().is_ok();
}
