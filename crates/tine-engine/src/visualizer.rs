//! 60 Hz UI frame pump: drains the post-DSP tap, computes the spectrum, and
//! publishes [`UiFrame`]s through a lock-free [`Triple`] for the UI thread.
//!
//! The [`Visualizer`] owns exactly one thread that exists only while started
//! (T17 starts it when the Rune window opens and stops it when the window
//! closes). Stopped, there is no thread and no wakeup of any kind; the tap
//! ring simply continues to wrap. The 60 Hz tick comes from a Win32
//! high-resolution waitable timer (`CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`,
//! Windows 10 1803+), so the pump is immune to the default ~15.6 ms timer
//! granularity and needs no busy spinning.
//!
//! The small Win32 surface (`CreateEventW`, `CreateWaitableTimerExW`,
//! `SetWaitableTimer`, `WaitForMultipleObjects`) lives here rather than in
//! [`crate::win`] because that module belongs to T15 (same precedent as
//! [`crate::shm_client`], which keeps its mapping calls local); every unsafe
//! block carries a `SAFETY:` comment.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread::{Builder, JoinHandle};
use std::time::{Duration, Instant};

use bytemuck::{Pod, Zeroable};
use tine_protocol::sync::{SpscRing, Triple};
use tine_protocol::telemetry::AudioTelemetryFrame;
use windows::Win32::Foundation::WAIT_OBJECT_0;
use windows::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateEventW, CreateWaitableTimerExW, SetEvent, SetWaitableTimer,
    TIMER_ALL_ACCESS, WaitForMultipleObjects,
};

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::Foundation::HANDLE;

use crate::controller::ControllerHandle;
use crate::spectrum::{BANDS, SpectrumAnalyzer};

/// RAII wrapper for a Win32 kernel-object handle that closes it on drop.
///
/// Unlike [`crate::win::Handle`] this wrapper is `Send + Sync`: kernel-object
/// handles are valid process-wide and the operations used on them
/// (`SetEvent`, `WaitForMultipleObjects`, `CloseHandle`) are thread-safe.
#[derive(Debug)]
struct SyncHandle(HANDLE);

// SAFETY: Kernel-object handles are process-wide values, not pointers into
// thread-local storage; every operation used on them is thread-safe.
unsafe impl Send for SyncHandle {}
// SAFETY: As `Send`: sharing the handle across threads is the documented
// Win32 usage pattern for events and timers.
unsafe impl Sync for SyncHandle {}

impl SyncHandle {
    /// Wraps `handle`, returning `None` if invalid.
    fn new(handle: HANDLE) -> Option<Self> {
        if handle.is_invalid() { None } else { Some(Self(handle)) }
    }

    /// The raw handle value.
    fn as_raw(&self) -> HANDLE {
        self.0
    }
}

impl Drop for SyncHandle {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a valid open handle owned by this wrapper.
            let _ = unsafe { CloseHandle(self.0) };
        }
    }
}

/// Visualizer tick rate in hertz.
pub const TICK_HZ: u32 = 60;

/// One tick at [`TICK_HZ`].
const TICK_PERIOD: Duration = Duration::from_nanos(1_000_000_000 / 60);

/// `WaitForMultipleObjects` infinite timeout (`INFINITE`).
const INFINITE_MS: u32 = u32::MAX;

/// Maximum tap samples drained per tick (catch-up bound after a stall;
/// = 170 ms of audio at 48 kHz).
const DRAIN_CAP: usize = 8192;

/// Default tap rate when the sink format is briefly unknown (D-006 native rate).
const DEFAULT_TAP_RATE: u32 = 48_000;

/// One complete UI visualization snapshot, published at 60 Hz through
/// [`Triple<UiFrame>`].
///
/// `Pod` requires zero hidden padding (hence the explicit `_pad` tail) and
/// only all-bit-patterns-valid members (`bool` is *not* `Pod`: a byte outside
/// 0/1 read as `bool` is UB), so the heartbeat liveness travels as a `u8`
/// flag: read it with [`UiFrame::heartbeat_ok`]. This is the one deliberate
/// deviation from the task-file contract sketch (`pub heartbeat_ok: bool`).
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq, Pod, Zeroable)]
pub struct UiFrame {
    /// The latest APO/engine telemetry frame.
    pub telemetry: AudioTelemetryFrame,
    /// 32 spectrum band values in dBFS (−90 dBFS floor).
    pub spectrum: [f32; BANDS],
    /// Heartbeat liveness flag: 1 = fresh heartbeat, 0 = stale (see
    /// [`UiFrame::heartbeat_ok`]).
    pub heartbeat: u8,
    /// Explicit tail padding (no hidden padding bytes; always zero).
    #[doc(hidden)]
    pub _pad: [u8; 7],
}

impl UiFrame {
    /// `true` while the engine heartbeat is fresh; the UI dims meters otherwise.
    #[must_use]
    pub const fn heartbeat_ok(&self) -> bool {
        self.heartbeat != 0
    }
}

const _: () = assert!(size_of::<UiFrame>() == 40 + 4 * BANDS + 1 + 7);

/// Where the visualizer reads telemetry and tap samples from.
///
/// Implemented for [`ControllerHandle`] (shared-memory path) and
/// [`InProcessSignals`] (in-process virtual-cable path, D-018). All methods
/// are single-reader by convention: the visualizer thread is the only
/// consumer of the tap and telemetry streams.
pub trait EngineSignals: Send + Sync + 'static {
    /// The latest telemetry frame.
    fn telemetry(&self) -> AudioTelemetryFrame;
    /// Drains up to `buf.len()` tap samples; returns the number read.
    fn drain_tap(&self, buf: &mut [f32]) -> usize;
    /// Tap sample rate in Hz (0 when unknown).
    fn tap_rate_hz(&self) -> u32;
    /// `true` while the engine heartbeat is fresh.
    fn heartbeat_ok(&self) -> bool;
}

impl EngineSignals for ControllerHandle {
    fn telemetry(&self) -> AudioTelemetryFrame {
        ControllerHandle::telemetry(self)
    }

    fn drain_tap(&self, buf: &mut [f32]) -> usize {
        ControllerHandle::drain_tap(self, buf)
    }

    fn tap_rate_hz(&self) -> u32 {
        self.shm_client().map_or(0, |c| c.sink_format().0)
    }

    fn heartbeat_ok(&self) -> bool {
        self.shm_client()
            .is_some_and(|c| matches!(c.state(), crate::shm_client::LinkState::Linked { .. }))
    }
}

/// In-process signals for the virtual-cable topology (D-018): telemetry
/// published by [`crate::virtual_cable`] through a [`Triple`], post-DSP tap
/// through an [`SpscRing`]; "heartbeat" is always alive.
pub struct InProcessSignals<const N: usize> {
    /// Telemetry frames published by the DSP loop.
    pub telemetry: Arc<Triple<AudioTelemetryFrame>>,
    /// Post-DSP tap samples pushed by the DSP loop.
    pub tap: Arc<SpscRing<N>>,
    /// Tap sample rate in Hz.
    pub rate_hz: u32,
}

impl<const N: usize> EngineSignals for InProcessSignals<N> {
    fn telemetry(&self) -> AudioTelemetryFrame {
        let mut frame = AudioTelemetryFrame::default();
        let _ = self.telemetry.latest(&mut frame);
        frame
    }

    fn drain_tap(&self, buf: &mut [f32]) -> usize {
        self.tap.pop_slice(buf)
    }

    fn tap_rate_hz(&self) -> u32 {
        self.rate_hz
    }

    fn heartbeat_ok(&self) -> bool {
        true
    }
}

/// Cumulative pump diagnostics (for tests and the CPU-duty evidence).
#[derive(Debug, Default)]
pub struct VisualizerStats {
    /// Completed ticks since start.
    pub ticks: AtomicU64,
    /// Wall time spent inside tick work (drain + FFT + publish) in ns.
    pub work_ns: AtomicU64,
}

impl VisualizerStats {
    /// Completed ticks and the fraction of wall time spent in tick work.
    ///
    /// The work fraction is the pump's CPU duty on one core; the acceptance
    /// criterion is a duty below 1 %.
    #[must_use]
    pub fn duty(&self) -> (u64, f64) {
        let ticks = self.ticks.load(Ordering::Relaxed);
        let expected_ns = ticks.saturating_mul(TICK_PERIOD.as_nanos() as u64);
        let work_ns = self.work_ns.load(Ordering::Relaxed);
        if expected_ns == 0 {
            (ticks, 0.0)
        } else {
            (ticks, work_ns as f64 / expected_ns as f64)
        }
    }
}

/// Live 60 Hz UI frame pump on one background thread.
///
/// [`Visualizer::start`] spawns the thread; [`Visualizer::stop`] (or dropping)
/// signals the stop event, joins the thread, and closes the handles. While
/// stopped there is no thread and no wakeup: a stopped visualizer costs
/// precisely nothing.
pub struct Visualizer {
    frames: Arc<Triple<UiFrame>>,
    stop: Option<Arc<SyncHandle>>,
    join: Option<JoinHandle<()>>,
    stats: Arc<VisualizerStats>,
}

impl core::fmt::Debug for Visualizer {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        let (ticks, duty) = self.stats.duty();
        f.debug_struct("Visualizer")
            .field("running", &self.is_running())
            .field("ticks", &ticks)
            .field("work_duty", &duty)
            .finish_non_exhaustive()
    }
}

impl Visualizer {
    /// Starts pumping [`UiFrame`]s from `signals` into a fresh [`Triple`].
    ///
    /// The tap rate is read once up front (the analyzer's band plan is fixed
    /// for its lifetime; a format change restarts the visualizer, see T17).
    ///
    /// If Win32 object creation fails (OOM-level refcount exhaustion), the
    /// visualizer stays inert: no thread, zero cost — never a leaked,
    /// unstoppable one.
    #[must_use]
    pub fn start<S: EngineSignals>(signals: S) -> Self {
        let frames = Arc::new(Triple::new(UiFrame::default()));
        let stats = Arc::new(VisualizerStats::default());

        // Manual-reset stop event; `stop`/`Drop` sets it to break the wait.
        // SAFETY: Win32 object creation with null name (anonymous event); the
        // returned handle is owned by the RAII `Handle` and closed exactly once.
        let stop = unsafe { CreateEventW(None, true, false, windows::core::PCWSTR::null()) }
            .ok()
            .and_then(SyncHandle::new)
            .map(Arc::new);

        let join = stop.as_ref().and_then(|stop| {
            let frames_out = Arc::clone(&frames);
            let stats_out = Arc::clone(&stats);
            let stop_wait = Arc::clone(stop);
            Builder::new()
                .name("tine-visualizer".to_owned())
                .spawn(move || pump(&signals, &frames_out, &stop_wait, &stats_out))
                .ok()
        });

        Self {
            frames,
            stop,
            join,
            stats,
        }
    }

    /// The published-frame buffer the UI reads (latest-wins, lock-free).
    #[must_use]
    pub fn frames(&self) -> Arc<Triple<UiFrame>> {
        Arc::clone(&self.frames)
    }

    /// Cumulative tick / CPU-duty counters (diagnostics).
    #[must_use]
    pub fn stats(&self) -> Arc<VisualizerStats> {
        Arc::clone(&self.stats)
    }

    /// `true` while the pump thread is alive.
    #[must_use]
    pub fn is_running(&self) -> bool {
        self.join.as_ref().is_some_and(|j| !j.is_finished())
    }

    /// Stops the pump thread and joins it; no wakeup remains afterwards.
    ///
    /// Idempotent: an already-stopped visualizer returns immediately.
    pub fn stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // SAFETY: `stop` is a valid open event handle owned by the `Arc`.
            let _ = unsafe { SetEvent(stop.as_raw()) };
        }
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Visualizer {
    fn drop(&mut self) {
        self.stop();
    }
}

/// High-resolution one-shot waitable timer (RAII).
struct TickTimer(SyncHandle);

impl TickTimer {
    /// Creates a `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION` timer.
    fn new() -> Option<Self> {
        // SAFETY: Win32 object creation with attributes default and null name;
        // the handle is owned by the RAII `SyncHandle` and closed exactly once.
        let timer = unsafe {
            CreateWaitableTimerExW(
                None,
                windows::core::PCWSTR::null(),
                CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
                TIMER_ALL_ACCESS.0,
            )
        };
        timer.ok().and_then(SyncHandle::new).map(Self)
    }

    /// Arms the timer to fire in `due_ns` nanoseconds (relative).
    ///
    /// Win32 due times are in 100-ns units, so nanoseconds divide by 100;
    /// a negative value makes the due time relative.
    fn arm(&self, due_ns: u64) {
        let relative_100ns = -((due_ns / 100).cast_signed()).max(1);
        // SAFETY: `self` holds a valid timer handle; `relative_100ns` is a
        // stack value valid for the call; negative = relative due time.
        let _ = unsafe { SetWaitableTimer(self.0.as_raw(), &raw const relative_100ns, 0, None, None, false) };
    }
}

/// The pump thread body.
fn pump<S: EngineSignals>(
    signals: &S,
    frames: &Arc<Triple<UiFrame>>,
    stop: &Arc<SyncHandle>,
    stats: &Arc<VisualizerStats>,
) {
    let Some(timer) = TickTimer::new() else {
        // Requires Windows 10 1803+; below our supported baseline. Treat as a
        // stopped visualizer rather than spinning: exit without ticks.
        return;
    };

    let rate = signals.tap_rate_hz();
    let mut spectrum = SpectrumAnalyzer::new(if rate > 0 { rate as f32 } else { DEFAULT_TAP_RATE as f32 });

    let mut tap_buf = vec![0.0_f32; DRAIN_CAP].into_boxed_slice();
    let start = Instant::now();
    let mut next_tick = start;

    loop {
        // Absolute scheduling: tick k fires at start + k·T, so tick work
        // never accumulates drift.
        next_tick += TICK_PERIOD;
        let now = Instant::now();
        let due_ns = if next_tick > now {
            (next_tick - now).as_nanos() as u64
        } else {
            1
        };
        timer.arm(due_ns);

        // Wait for the tick or the stop event, whichever comes first.
        // SAFETY: both handles are valid; the slice covers exactly the two
        // entries; an infinite wait means the result is one of the two.
        // SAFETY: both handles are valid; the slice covers exactly the two
        // entries; an infinite wait means the result is one of them.
        let res = unsafe { WaitForMultipleObjects(&[timer.0.as_raw(), stop.as_raw()], false, INFINITE_MS) };
        if res != WAIT_OBJECT_0 {
            // Stop event signalled, or the wait failed: exit either way.
            break;
        }

        let t0 = Instant::now();

        // Drain up to DRAIN_CAP samples (steady state ≈ 800 @ 48 kHz·60 Hz).
        let mut total = 0_usize;
        while total < DRAIN_CAP {
            let room = DRAIN_CAP - total;
            if let Some(chunk) = tap_buf.get_mut(total..total + room) {
                let n = signals.drain_tap(chunk);
                if n == 0 || n < chunk.len() {
                    total += n;
                    break;
                }
                total += n;
            } else {
                break;
            }
        }
        if total > 0
            && let Some(fed) = tap_buf.get(..total)
        {
            spectrum.feed(fed);
        }

        // Publish one frame; heartbeat liveness decides the UI dim state.
        let frame = UiFrame {
            telemetry: signals.telemetry(),
            spectrum: *spectrum.bands(),
            heartbeat: u8::from(signals.heartbeat_ok()),
            _pad: [0; 7],
        };
        frames.publish(&frame);

        stats.ticks.fetch_add(1, Ordering::Relaxed);
        stats
            .work_ns
            .fetch_add(t0.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }
}

#[cfg(test)]
mod tests {
    use super::{InProcessSignals, TICK_HZ, UiFrame, Visualizer};
    use std::sync::Arc;
    use tine_protocol::sync::{SpscRing, Triple};
    use tine_protocol::telemetry::AudioTelemetryFrame;

    const TAP_N: usize = 32_768;

    fn signals_with_tap() -> (
        InProcessSignals<TAP_N>,
        Arc<SpscRing<TAP_N>>,
        Arc<Triple<AudioTelemetryFrame>>,
    ) {
        let tap = Arc::new(SpscRing::<TAP_N>::new());
        let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
        let signals = InProcessSignals {
            telemetry: Arc::clone(&telemetry),
            tap: Arc::clone(&tap),
            rate_hz: 48_000,
        };
        (signals, tap, telemetry)
    }

    #[test]
    fn visualizer_pumps_frames_and_spectra_at_60hz() {
        let (signals, tap, telemetry) = signals_with_tap();
        let mut vis = Visualizer::start(signals);

        // Produce a 1 kHz tone into the tap while the pump runs.
        let producer = std::thread::spawn(move || {
            let period = 48.0_f32;
            let mut phase = 0.0_f32;
            let mut block = [0.0_f32; 480];
            for _ in 0..200 {
                for x in &mut block {
                    *x = 0.25 * (2.0 * core::f32::consts::PI * phase).sin();
                    phase = (phase + 1.0 / period) % 1.0;
                }
                let _ = tap.push_slice(&block);
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
        });

        // Let the pump gather ~200 ticks (>3 s).
        std::thread::sleep(std::time::Duration::from_millis(2_600));
        producer.join().ok();

        let frames = vis.frames();
        let mut frame = UiFrame::default();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if frames.latest(&mut frame) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            frame.heartbeat_ok(),
            "in-process signals always report a live heartbeat"
        );
        let top = frame.spectrum.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            top > -40.0,
            "1 kHz tone must register in the spectrum, got {top:.1} dBFS"
        );
        // 1 kHz sits at band ln(1000/20)/ln(1000)·32 = 18.1 → band 18.
        let peak_band = frame
            .spectrum
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.total_cmp(b.1))
            .map_or(0, |(i, _)| i);
        assert!(peak_band.abs_diff(18) <= 1, "1 kHz peak in band {peak_band}");

        let (ticks, duty) = vis.stats().duty();
        println!("visualizer: {ticks} ticks over ~2.6 s, work duty {duty:.4} (CPU fraction of one core)");
        assert!(ticks >= 100, "60 Hz pump produced only {ticks} ticks in 2.6 s");
        assert!(duty < 0.1, "work duty {duty} exceeds the generous debug-build budget");

        // Stop: joins, thread exits, no new frames published afterwards.
        vis.stop();
        let _ = telemetry;
        let stats = vis.stats();
        let ticks_at_stop = stats.duty().0;
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(stats.duty().0, ticks_at_stop, "stopped visualizer must not wake");
        assert!(!vis.is_running(), "pump thread must be joined after stop");
    }

    #[test]
    fn tick_rate_is_60hz() {
        let (signals, _tap, _t) = signals_with_tap();
        let mut vis = Visualizer::start(signals);
        std::thread::sleep(std::time::Duration::from_millis(1_100));
        let (ticks, _duty) = vis.stats().duty();
        let expected = u64::from(TICK_HZ) * 11 / 10;
        println!("{ticks} ticks in 1.1 s (expect ≈{expected})");
        assert!((35..=95).contains(&ticks), "tick rate far from 60 Hz: {ticks}/1.1 s");
        vis.stop();
    }
}
