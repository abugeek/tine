//! 9-second Auto-Tune capture runner (the engine side of the calibration
//! wizard): bypass lease, raw-mic capture, calibration feeding, progress
//! fan-out, result dispatch.
//!
//! The runner is the glue between three worlds: the APO's shared memory (a
//! time-boxed **bypass lease** so a dying `tine.exe` self-heals within 15 s),
//! `cpal` capture (the raw microphone — captured *after* the APO entered
//! bypass, so the stream carries the unprocessed signal), and the
//! [`Calibrator`] state machine (T11) which turns 9 s of mono audio into a
//! report.
//!
//! ```text
//! lease = now+15 s ─► wait bypass confirmed ─► open_capture ─► feed ──┐
//!    ▲ renew every 5 s                                              │ 30 Hz progress
//!    └────────────────────────────────┐                             ▼
//!                          clear lease ◄─ Analyzing ─► finish ─► ApplyCalibration via
//!                                                                    the Controller
//! ```
//!
//! # Calibrator wiring
//! The runner drives the real [`tine_dsp::calibrator::AutoCalibrator`] (T11,
//! merged as 08c4914) through the thin [`Calibrator`] /
//! [`CalibratorFactory`] seam — the seam exists so offline tests can script
//! capture outcomes without a microphone. [`AutoCalibratorFactory`] is the
//! production factory, and the T11 data types are re-exported here for
//! T17/T20 (their shape is frozen by the T11 contract).

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use thiserror::Error;
use tine_dsp::calibrator::{AutoCalibrator, CHANGED_PARAM_COUNT, LIVE_COUNTDOWN_S, LIVE_NOISE_S, LIVE_VOICE_S};
use tine_dsp::presets::PresetDef;
use tine_protocol::params::DspParams;
use tine_protocol::presets::{MacroPositions, PinSet};
use tine_protocol::sync::SpscRing;
use tine_protocol::telemetry::flags;

pub use tine_dsp::calibrator::{
    CalFailure, CalPhase, CalProgress, CalibrationReport, RoomQuality, VoiceDiagnosis, VoiceDynamics, VoiceStyle,
    VoiceTone, diagnose_voice, recommend_macros,
};

use crate::audio_io::{AudioIoError, DeviceRef, open_capture};
use crate::controller::{Command, ControllerHandle};
use crate::model::KeepPins;
use crate::shm_client::ShmClient;

/// Bypass-lease duration: the APO stays bypassed this far into the future;
/// renewed throughout the run, so a crashed `tine.exe` self-heals within this
/// bound (see the task's acceptance criteria).
pub const LEASE_MS: u64 = 15_000;

/// Lease renewal period.
pub const RENEW_MS: u64 = 5_000;

/// Progress-event rate to the UI (fan-out channel).
pub const PROGRESS_HZ: u32 = 30;

/// How long to wait for a telemetry/state frame confirming bypass before
/// giving up (the APO publishes state at ~60 Hz while processing).
pub const BYPASS_CONFIRM_TIMEOUT_MS: u64 = 2_000;

/// Overall hard wall-clock deadline for a complete Auto-Tune run (20 s).
pub const HARD_DEADLINE_MS: u64 = 20_000;

/// Maximum time without receiving any capture samples before aborting (2.5 s).
pub const NO_PROGRESS_TIMEOUT_MS: u64 = 2_500;

/// Inter-stream ring between the capture callback (producer) and the runner
/// thread (consumer): 16,384 samples ≈ 341 ms at 48 kHz.
pub const CAPTURE_RING: usize = 16_384;

/// Number of calibration-driven parameter changes in a [`CalibrationReport`]
/// (tracks T11's [`CHANGED_PARAM_COUNT`] — the five derived parameters).
pub const CAL_CHANGES: usize = CHANGED_PARAM_COUNT;

const _: () = assert!(CAL_CHANGES == 5);

/// The two operations the runner needs from the calibration state machine:
/// `push` (streaming audio in, live state out) and `finish` (analysis
/// inputs, report or failure out) — exactly the frozen T11 surface.
///
/// Implemented by [`AutoCalibrator`] (production) and by scripted fakes in
/// the integration tests.
pub trait Calibrator: Send {
    /// Streams `samples` into the state machine; allocation-free by contract.
    fn push(&mut self, samples: &[f32]) -> CalProgress;

    /// Analyzes the captured audio; returns the report or a typed failure.
    fn finish(
        &mut self,
        preset: &PresetDef,
        macros: MacroPositions,
        pins: PinSet,
        current: &DspParams,
    ) -> Result<CalibrationReport, CalFailure>;

    /// Returns the captured voice-phase mono samples (`fs` Hz), when available.
    fn captured_voice(&self) -> &[f32] {
        &[]
    }
}

/// Constructs a [`Calibrator`] once the capture's actual sample rate is known
/// (after bypass confirmation, inside the runner thread).
pub trait CalibratorFactory: Send + Sync {
    /// Builds a calibrator for a capture running at `fs` Hz.
    fn make(&self, fs: f32) -> Box<dyn Calibrator>;
}

/// The production factory: builds the real
/// [`tine_dsp::calibrator::AutoCalibrator`] for the negotiated capture rate (9 s durations).
#[derive(Debug, Default)]
pub struct AutoCalibratorFactory;

impl CalibratorFactory for AutoCalibratorFactory {
    fn make(&self, fs: f32) -> Box<dyn Calibrator> {
        Box::new(AutoCalibrator::new(fs))
    }
}

/// Interactive live factory: uses human-paced durations ([`LIVE_COUNTDOWN_S`],
/// [`LIVE_NOISE_S`], [`LIVE_VOICE_S`]) so real users have time to read instructions
/// and take a breath before speaking.
#[derive(Debug, Default)]
pub struct LiveAutoCalibratorFactory;

impl CalibratorFactory for LiveAutoCalibratorFactory {
    fn make(&self, fs: f32) -> Box<dyn Calibrator> {
        Box::new(AutoCalibrator::with_durations(
            fs,
            LIVE_COUNTDOWN_S,
            LIVE_NOISE_S,
            LIVE_VOICE_S,
        ))
    }
}

impl Calibrator for AutoCalibrator {
    fn push(&mut self, samples: &[f32]) -> CalProgress {
        AutoCalibrator::push(self, samples)
    }

    fn finish(
        &mut self,
        preset: &PresetDef,
        macros: MacroPositions,
        pins: PinSet,
        current: &DspParams,
    ) -> Result<CalibrationReport, CalFailure> {
        AutoCalibrator::finish(self, preset, macros, pins, current)
    }

    fn captured_voice(&self) -> &[f32] {
        AutoCalibrator::captured_voice(self)
    }
}

// ---------------------------------------------------------------------------
// Capture sources
// ---------------------------------------------------------------------------

/// Non-RT mono capture feeding the runner thread.
///
/// One `read` returns whatever is currently available (0 is allowed); the
/// runner polls with short sleeps. Real-time code lives behind the source,
/// never in the runner.
pub trait SampleSource: Send {
    /// The capture's actual sample rate in Hz.
    fn rate_hz(&self) -> u32;

    /// Reads up to `buf.len()` samples; returns the count read.
    fn read(&mut self, buf: &mut [f32]) -> usize;

    /// `true` when the underlying stream died (device unplugged).
    fn has_error(&self) -> bool;
}

/// Raw-mic capture through [`crate::audio_io::open_capture`] (shared mode, so
/// while the APO holds the bypass lease the stream is the *raw* signal).
pub struct DeviceCapture {
    stream: crate::audio_io::CaptureStream,
    ring: Arc<SpscRing<CAPTURE_RING>>,
}

impl DeviceCapture {
    /// Opens the capture on `device`, preferring `want_rate`.
    pub fn open(device: &DeviceRef, want_rate: Option<u32>) -> Result<Self, AudioIoError> {
        let ring = Arc::new(SpscRing::<CAPTURE_RING>::new());
        // `on_block` is a no-op: the RT path only pushes the downmixed mono
        // block into the SPSC ring.
        let stream = open_capture(device, want_rate, Arc::clone(&ring), |_| {})?;
        Ok(Self { stream, ring })
    }

    /// Drops the stream ( stops the capture and releases the device.
    pub fn stop(self) {
        let _ = self.stream.pause();
        drop(self.stream);
        drop(self.ring);
    }
}

impl SampleSource for DeviceCapture {
    fn rate_hz(&self) -> u32 {
        self.stream.sample_rate()
    }

    fn read(&mut self, buf: &mut [f32]) -> usize {
        self.ring.pop_slice(buf)
    }

    fn has_error(&self) -> bool {
        self.stream.has_error()
    }
}

/// Which capture to open once the bypass lease is confirmed.
pub enum CaptureSpec {
    /// A real input device (shared mode; the APO is bypassed, so this is the
    /// raw microphone).
    Device {
        /// Device selector (defaults to the system input).
        device: DeviceRef,
        /// Preferred sample rate (native 48 kHz when `None`).
        want_rate: Option<u32>,
    },
    /// A caller-supplied offline source (tests, `devhost` fixtures).
    Source(Box<dyn SampleSource>),
}

impl core::fmt::Debug for CaptureSpec {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Device { device, want_rate } => f
                .debug_struct("CaptureSpec::Device")
                .field("device", device)
                .field("want_rate", want_rate)
                .finish(),
            Self::Source(_) => f.write_str("CaptureSpec::Source(offline)"),
        }
    }
}

// ---------------------------------------------------------------------------
// Runner
// ---------------------------------------------------------------------------

/// Runner startup configuration.
pub struct AutoTuneConfig {
    /// Shared-memory client of the endpoint being calibrated (lease writer,
    /// bypass-state reader).
    pub client: Arc<ShmClient>,
    /// Where the result is dispatched through (`Command::ApplyCalibration`);
    /// `None` leaves applying to the caller (wizard preview).
    pub controller: Option<ControllerHandle>,
    /// What to capture once the APO confirms bypass.
    pub capture: CaptureSpec,
    /// Calibrator factory invoked at the negotiated sample rate.
    pub factory: Arc<dyn CalibratorFactory>,
    /// Analysis inputs forwarded to `Calibrator::finish`.
    pub preset: PresetDef,
    /// Analysis inputs forwarded to `Calibrator::finish`.
    pub macros: MacroPositions,
    /// Analysis inputs forwarded to `Calibrator::finish`.
    pub pins: PinSet,
    /// Analysis inputs forwarded to `Calibrator::finish`.
    pub current: DspParams,
    /// What happens to Pro-mode pins of calibration-driven parameters when
    /// the result is applied.
    pub keep_pins: KeepPins,
}

/// Terminal state of an Auto-Tune run.
#[derive(Debug)]
pub enum AutoTuneOutcome {
    /// Completed: a report was produced; applied to the controller when one
    /// was configured (the report itself is still handed to the wizard UI).
    Applied(Box<CalibrationReport>),
    /// Completed: the calibrator rejected the capture.
    Failed(CalFailure),
    /// The run could not complete for infrastructure reasons.
    Error(AutoTuneError),
}

/// Errors short-circuiting an Auto-Tune run.
#[derive(Debug, Error)]
pub enum AutoTuneError {
    /// The APO never confirmed bypass within [`BYPASS_CONFIRM_TIMEOUT_MS`].
    #[error("bypass was not confirmed by the APO")]
    BypassNotConfirmed,
    /// Opening the capture device failed.
    #[error("capture open failed: {0}")]
    Capture(#[from] AudioIoError),
    /// The capture stream died mid-run (device unplugged).
    #[error("capture stream lost")]
    StreamLost,
    /// The user cancelled the wizard.
    #[error("cancelled")]
    Cancelled,
    /// The report was produced but the controller channel was closed.
    #[error("controller unreachable; calibration not applied")]
    ControllerUnreachable,
    /// Auto-Tune exceeded the hard deadline or stopped receiving audio.
    #[error("auto-tune timed out")]
    TimedOut,
}

/// Event stream of a running Auto-Tune wizard.
#[derive(Debug)]
pub enum AutoTuneEvent {
    /// The bypass is confirmed and the capture is running at this rate.
    Started {
        /// Negotiated capture sample rate in Hz.
        rate_hz: u32,
    },
    /// Live calibrator state (throttled to ~30 Hz, phase changes immediate).
    Progress(CalProgress),
    /// Terminal event (exactly one per run).
    Done(AutoTuneOutcome),
}

/// Handle to a running Auto-Tune wizard.
///
/// Dropping the handle does **not** cancel the run: the lease is absolute
/// wall-clock time, so a dropped or crashed caller self-heals within
/// [`LEASE_MS`] regardless. Use [`AutoTuneHandle::cancel`] for an orderly stop.
pub struct AutoTuneHandle {
    cancel: Arc<AtomicBool>,
    events: Receiver<AutoTuneEvent>,
    join: Option<JoinHandle<()>>,
}

impl AutoTuneHandle {
    /// Requests an orderly stop: the lease is cleared and a terminal
    /// `Done(Error(Cancelled))` event is emitted.
    pub fn cancel(&mut self) {
        self.cancel.store(true, Ordering::Release);
    }

    /// The event receiver (single consumer): `Started`, `Progress`, `Done`.
    pub fn events(&mut self) -> &mut Receiver<AutoTuneEvent> {
        &mut self.events
    }

    /// Blocks until the run finishes (or the budget expires); returns the
    /// terminal outcome and joins the runner thread.
    ///
    /// `None` means the budget ran out before a `Done` event (the run is
    /// still going: the handle stays usable).
    #[must_use = "the outcome decides whether a report exists and was applied"]
    pub fn wait(mut self, budget: Duration) -> Option<AutoTuneOutcome> {
        let deadline = std::time::Instant::now() + budget;
        loop {
            let remain = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(remain) {
                Ok(AutoTuneEvent::Done(outcome)) => {
                    if let Some(j) = self.join.take() {
                        let _ = j.join();
                    }
                    return Some(outcome);
                }
                Ok(_) => {}
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => return None,
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    if let Some(j) = self.join.take() {
                        let _ = j.join();
                    }
                    return None;
                }
            }
        }
    }
}

/// Entry point for the Auto-Tune wizard (`AutoTuneRunner` in the contract
/// sketch): spawns the runner thread and returns the handle.
pub struct AutoTuneRunner;

impl AutoTuneRunner {
    /// Starts a wizard run; every path ends in exactly one
    /// [`AutoTuneEvent::Done`] and always clears the bypass lease.
    #[must_use]
    pub fn start(cfg: AutoTuneConfig) -> AutoTuneHandle {
        let (tx, rx) = channel::<AutoTuneEvent>();
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_thread = Arc::clone(&cancel);
        // Thread spawn failure degrades exactly like a dead channel: the
        // events receiver disconnects immediately and the run never held a
        // lease (nothing was written yet).
        let join = Builder::new()
            .name("tine-autotune".to_owned())
            .spawn(move || {
                run(cfg, &tx, &cancel_thread);
            })
            .ok();
        AutoTuneHandle {
            cancel,
            events: rx,
            join,
        }
    }
}

// ---------------------------------------------------------------------------
// Lease helpers (shared-memory header writes; read by `tine-app`'s `LeaseShim` under D-018)
// ---------------------------------------------------------------------------

/// Current wall-clock time in epoch milliseconds.
fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Extends the bypass lease to `now + LEASE_MS`.
///
/// The `Release` store pairs with the APO's `Acquire` load of
/// `bypass_lease_until_ms`: it may not observe a *shorter* lease than the
/// one we last wrote.
pub fn renew_lease(client: &ShmClient, now_ms: u64) {
    client
        .layout()
        .header
        .bypass_lease_until_ms
        .store(now_ms + LEASE_MS, Ordering::Release);
}

/// Clears the bypass lease (processing resumes as soon as the APO reads it).
pub fn clear_lease(client: &ShmClient) {
    // 0 is in the past for any observer reading wall-clock time.
    client.layout().header.bypass_lease_until_ms.store(0, Ordering::Release);
}

/// The lease deadline currently visible in the header (epoch ms; 0 = none).
#[must_use]
pub fn lease_until_ms(client: &ShmClient) -> u64 {
    client.layout().header.bypass_lease_until_ms.load(Ordering::Relaxed)
}

// ---------------------------------------------------------------------------
// Runner body
// ---------------------------------------------------------------------------

/// Runs the state machine; always clears the lease before returning.
fn run(
    AutoTuneConfig {
        client,
        controller,
        capture,
        factory,
        preset,
        macros,
        pins,
        current,
        keep_pins,
    }: AutoTuneConfig,
    tx: &Sender<AutoTuneEvent>,
    cancel: &AtomicBool,
) {
    // Lease from the very first instruction: the APO must be bypassed before
    // the capture opens, and a crash anywhere below self-heals within LEASE_MS.
    let hard_deadline = std::time::Instant::now() + Duration::from_millis(HARD_DEADLINE_MS);
    renew_lease(&client, now_ms());

    let outcome: Result<CalibrationReport, AutoTuneOutcome> = match confirm_bypass(&client) {
        Ok(()) => capture_phase(
            &client,
            capture,
            &factory,
            preset,
            macros,
            pins,
            current,
            tx,
            cancel,
            hard_deadline,
        ),
        Err(err) => Err(AutoTuneOutcome::Error(err)),
    };

    let outcome = match outcome {
        Ok(report) => match dispatch(controller.as_ref(), &report, keep_pins) {
            Ok(()) => AutoTuneOutcome::Applied(Box::new(report)),
            Err(err) => AutoTuneOutcome::Error(err),
        },
        Err(outcome) => outcome,
    };

    // Always release the bypass, including on success: processing must
    // resume right after the report lands.
    clear_lease(&client);
    let _ = tx.send(AutoTuneEvent::Done(outcome));
}

/// Waits until the APO's state flags report bypass, renewing the lease.
fn confirm_bypass(client: &Arc<ShmClient>) -> Result<(), AutoTuneError> {
    let deadline = std::time::Instant::now() + Duration::from_millis(BYPASS_CONFIRM_TIMEOUT_MS);
    loop {
        // `state` flags mirror the telemetry BYPASS bit; reading the raw
        // header avoids contending the triple-buffer's single-reader slot
        // (the visualizer may be draining telemetry concurrently).
        if client.state_flags() & flags::BYPASS != 0 {
            return Ok(());
        }
        if std::time::Instant::now() >= deadline {
            return Err(AutoTuneError::BypassNotConfirmed);
        }
        renew_lease(client, now_ms());
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// An instant one progress period in the past (forces the first send).
fn last_progress_anchor(period: Duration) -> std::time::Instant {
    std::time::Instant::now()
        .checked_sub(period)
        .unwrap_or_else(std::time::Instant::now)
}

/// Opens the capture, feeds the calibrator, throttles progress at 30 Hz,
/// renews the lease, and returns the finished report.
#[allow(clippy::too_many_lines, clippy::too_many_arguments)] // one cohesive state machine
fn capture_phase(
    client: &Arc<ShmClient>,
    capture: CaptureSpec,
    factory: &Arc<dyn CalibratorFactory>,
    preset: PresetDef,
    macros: MacroPositions,
    pins: PinSet,
    current: DspParams,
    tx: &Sender<AutoTuneEvent>,
    cancel: &AtomicBool,
    hard_deadline: std::time::Instant,
) -> Result<CalibrationReport, AutoTuneOutcome> {
    let mut source: Box<dyn SampleSource> = match capture {
        CaptureSpec::Device { device, want_rate } => match DeviceCapture::open(&device, want_rate) {
            Ok(cap) => Box::new(cap),
            Err(err) => return Err(AutoTuneOutcome::Error(AutoTuneError::Capture(err))),
        },
        CaptureSpec::Source(src) => src,
    };

    if source.rate_hz() == 0 {
        return Err(AutoTuneOutcome::Error(AutoTuneError::StreamLost));
    }
    let _ = tx.send(AutoTuneEvent::Started {
        rate_hz: source.rate_hz(),
    });

    let mut calibrator = factory.make(source.rate_hz() as f32);
    let mut buffer = vec![0.0_f32; 4096].into_boxed_slice();
    let progress_period = Duration::from_nanos(1_000_000_000 / u64::from(PROGRESS_HZ));
    let mut last_progress_sent = last_progress_anchor(progress_period);
    let mut last_renew = std::time::Instant::now();
    let mut last_audio_at = std::time::Instant::now();
    let no_progress_timeout = Duration::from_millis(NO_PROGRESS_TIMEOUT_MS);
    let mut last_phase: Option<CalPhase> = None;
    #[allow(unused_assignments)] // the open-loop initializer exists for the (impossible) empty-read path
    let mut final_progress: Option<CalProgress> = None;

    loop {
        if cancel.load(Ordering::Acquire) {
            return Err(AutoTuneOutcome::Error(AutoTuneError::Cancelled));
        }
        if source.has_error() {
            return Err(AutoTuneOutcome::Error(AutoTuneError::StreamLost));
        }
        if std::time::Instant::now() >= hard_deadline {
            return Err(AutoTuneOutcome::Error(AutoTuneError::TimedOut));
        }

        // Lease renewal: never let the deadline drift closer than LEASE_MS -
        // RENEW_MS even when the calibrator stalls on a huge chunk.
        if last_renew.elapsed() >= Duration::from_millis(RENEW_MS) {
            renew_lease(client, now_ms());
            last_renew = std::time::Instant::now();
        }

        let got = source.read(&mut buffer);
        if got == 0 {
            if last_audio_at.elapsed() >= no_progress_timeout {
                return Err(AutoTuneOutcome::Error(AutoTuneError::TimedOut));
            }
            // Nothing new yet: a short poll keeps the UI latency low without
            // spinning (the RT producer runs on its own callback cadence).
            std::thread::sleep(Duration::from_millis(4));
            continue;
        }
        last_audio_at = std::time::Instant::now();

        let chunk: &[f32] = buffer.get(..got).map_or(&buffer[..0], |c| c);
        let progress = calibrator.push(chunk);
        final_progress = Some(progress);

        let phase_changed = last_phase != Some(progress.phase);
        if phase_changed || last_progress_sent.elapsed() >= progress_period {
            let _ = tx.send(AutoTuneEvent::Progress(progress));
            last_progress_sent = std::time::Instant::now();
            last_phase = Some(progress.phase);
        }

        if progress.phase == CalPhase::Analyzing {
            break;
        }
    }

    // Deterministic final progress for the UI (analysis at 100 %).
    let mut analyzing = final_progress.unwrap_or(CalProgress {
        phase: CalPhase::Countdown,
        phase_progress: 0.0,
        live_level_db: -90.0,
        voiced_ms: 0,
    });
    analyzing.phase = CalPhase::Analyzing;
    analyzing.phase_progress = 1.0;
    let _ = tx.send(AutoTuneEvent::Progress(analyzing));

    let outcome = calibrator.finish(&preset, macros, pins, &current);
    match outcome {
        Ok(report) => Ok(report),
        Err(failure) => {
            let mut failed = analyzing;
            failed.phase = CalPhase::Failed;
            let _ = tx.send(AutoTuneEvent::Progress(failed));
            Err(AutoTuneOutcome::Failed(failure))
        }
    }
}

/// Dispatches `Command::ApplyCalibration` through the controller.
fn dispatch(
    controller: Option<&ControllerHandle>,
    report: &CalibrationReport,
    keep_pins: KeepPins,
) -> Result<(), AutoTuneError> {
    match controller {
        None => Ok(()),
        Some(handle) => handle
            .send(Command::ApplyCalibration(report.calibration, keep_pins))
            .map_err(|_| AutoTuneError::ControllerUnreachable),
    }
}
