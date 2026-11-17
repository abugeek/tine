//! "Hear Myself" monitor: plays the post-DSP [`MonitorFeed`] from
//! [`crate::virtual_cable::VirtualCableRouter`] to the chosen headphones with
//! adaptive clock-drift correction.
//!
//! The monitor follows its own toggle (T17 starts/stops it from
//! [`crate::model::MonitorSettings`]): while on, it runs a single playback
//! `cpal` stream pulling from the router's [`MonitorFeed`] ring buffer; while
//! off, no stream exists and `VirtualCableProcessor` skips pushing to the
//! monitor ring, so the cost is zero.
//!
//! Failures are reported, never panicked through: device errors surface in
//! [`MonitorStats`] and [`Monitor::has_error`]; a device change restarts
//! cleanly via [`Monitor::restart`].

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use thiserror::Error;
use tine_protocol::sync::SpscRing;

use crate::audio_io::{AudioIoError, DeviceRef, PlaybackStream, open_playback_with_headroom};

/// Monitor ring capacity: 16,384 samples ≈ 341 ms at 48 kHz (power of two).
pub const MONITOR_RING_CAP: usize = 16_384;

/// Added DSP latency in the processed signal at 48 kHz (D-019: 1036 samples —
/// `DspPipeline::latency_samples()` / 48 kHz).
pub const DSP_LATENCY_MS: f32 = 21.6;

/// Nominal WASAPI shared-mode device period: the playback-side buffer the
/// `cpal` API does not expose a real measurement for; see `Monitor::latency_ms`.
pub const NOMINAL_PERIOD_MS: f32 = 10.0;

/// Monitor jitter-buffer headroom in callbacks (`3.0` ≈ 30 ms at 10 ms period).
pub const MONITOR_HEADROOM_CALLBACKS: f32 = 3.0;

/// Monitor jitter-buffer headroom in milliseconds (~30 ms).
pub const MONITOR_HEADROOM_MS: f32 = MONITOR_HEADROOM_CALLBACKS * NOMINAL_PERIOD_MS;

/// Errors opening a monitor path.
#[derive(Debug, Error)]
pub enum MonitorError {
    /// Opening the playback (headphones) failed.
    #[error("monitor playback failed: {0}")]
    Playback(#[from] AudioIoError),
}

/// Lock-free post-DSP audio feed shared between [`crate::virtual_cable::VirtualCableRouter`]
/// and [`Monitor`].
#[derive(Clone)]
pub struct MonitorFeed {
    pub(crate) ring: Arc<SpscRing<MONITOR_RING_CAP>>,
    pub(crate) enabled: Arc<AtomicBool>,
    pub(crate) block_frames: Arc<AtomicU32>,
    dsp_latency: u32,
    rate_hz: u32,
}

impl MonitorFeed {
    /// Creates a new [`MonitorFeed`] for a router running at `rate_hz` with
    /// `dsp_latency` samples of added pipeline delay.
    #[must_use]
    pub fn new(rate_hz: u32, dsp_latency: u32) -> Self {
        Self {
            ring: Arc::new(SpscRing::new()),
            enabled: Arc::new(AtomicBool::new(false)),
            block_frames: Arc::new(AtomicU32::new(0)),
            dsp_latency,
            rate_hz,
        }
    }

    /// Creates a [`MonitorFeed`] from existing shared synchronization primitives.
    #[must_use]
    pub(crate) fn with_parts(
        rate_hz: u32,
        dsp_latency: u32,
        ring: Arc<SpscRing<MONITOR_RING_CAP>>,
        enabled: Arc<AtomicBool>,
        block_frames: Arc<AtomicU32>,
    ) -> Self {
        Self {
            ring,
            enabled,
            block_frames,
            dsp_latency,
            rate_hz,
        }
    }

    /// Negotiated sample rate of the source router in Hz.
    #[must_use]
    pub const fn rate_hz(&self) -> u32 {
        self.rate_hz
    }

    /// Added DSP pipeline latency of the source router in samples.
    #[must_use]
    pub const fn dsp_latency(&self) -> u32 {
        self.dsp_latency
    }
}

/// Live monitor statistics for the latency badge and stream health.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MonitorStats {
    /// Playback underruns (audio glitches the user would hear).
    pub underruns: u64,
    /// Feed-ring drops (full monitor ring).
    pub overruns: u64,
    /// Total playback callbacks.
    pub callbacks: u64,
    /// Longest callback execution in microseconds.
    pub max_callback_us: u32,
    /// Source router sample rate in Hz.
    pub capture_rate_hz: u32,
    /// Negotiated playback sample rate in Hz.
    pub playback_rate_hz: u32,
    /// Raw stream error flag (device lost / unplugged).
    pub stream_lost: bool,
}

/// Live "Hear Myself" monitor playing post-DSP audio to headphones.
pub struct Monitor {
    playback: Option<PlaybackStream>,
    feed: MonitorFeed,
}

impl Monitor {
    /// Drains any stale samples from `feed`, enables the post-DSP tap, and opens
    /// a playback stream to `output` with drift-corrected resampling.
    pub fn start(feed: &MonitorFeed, output: &DeviceRef) -> Result<Self, MonitorError> {
        let mut scratch = [0.0_f32; 1024];
        while feed.ring.pop_slice(&mut scratch) > 0 {}

        feed.enabled.store(true, Ordering::Release);
        match open_playback_with_headroom(output, Arc::clone(&feed.ring), feed.rate_hz, MONITOR_HEADROOM_CALLBACKS) {
            Ok(playback) => Ok(Self {
                playback: Some(playback),
                feed: feed.clone(),
            }),
            Err(err) => {
                feed.enabled.store(false, Ordering::Release);
                Err(MonitorError::Playback(err))
            }
        }
    }

    /// Restarts the playback stream on a new device (device change / re-selection).
    /// The old stream is paused and dropped *before* opening the new one,
    /// so the device is released even if opening the new one fails.
    pub fn restart(&mut self, feed: &MonitorFeed, output: &DeviceRef) -> Result<(), MonitorError> {
        self.feed.enabled.store(false, Ordering::Release);
        if let Some(p) = self.playback.take() {
            let _ = p.pause();
            drop(p);
        }
        let fresh = Self::start(feed, output)?;
        *self = fresh;
        Ok(())
    }

    /// Stats snapshot (underruns drive the ⚠ badge; latency drives the label).
    #[must_use]
    pub fn stats(&self) -> MonitorStats {
        let p = self.playback.as_ref().map(PlaybackStream::stats).unwrap_or_default();
        let lost = self.playback.as_ref().is_none_or(PlaybackStream::has_error);
        MonitorStats {
            underruns: p.underruns,
            overruns: self.feed.ring.overruns(),
            callbacks: p.callbacks,
            max_callback_us: p.max_callback_us,
            capture_rate_hz: self.feed.rate_hz,
            playback_rate_hz: self.playback.as_ref().map_or(0, PlaybackStream::sample_rate),
            stream_lost: lost,
        }
    }

    /// The latency badge value in milliseconds:
    /// `block_frames / rate + dsp_latency / rate + MONITOR_HEADROOM_MS + NOMINAL_PERIOD_MS`.
    #[must_use]
    pub fn latency_ms(&self) -> f32 {
        let rate = self.feed.rate_hz;
        let (block_ms, dsp_ms) = if rate > 0 {
            let frames = self.feed.block_frames.load(Ordering::Relaxed);
            (
                frames as f32 * 1_000.0 / rate as f32,
                self.feed.dsp_latency as f32 * 1_000.0 / rate as f32,
            )
        } else {
            (0.0, 0.0)
        };
        block_ms + dsp_ms + MONITOR_HEADROOM_MS + NOMINAL_PERIOD_MS
    }

    /// `true` when the playback stream reported an error (device unplugged): the
    /// UI shows a monitor error and may call [`Monitor::restart`].
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.playback.as_ref().is_none_or(PlaybackStream::has_error)
    }

    /// Stops monitoring and releases the playback device.
    pub fn stop(self) {
        drop(self);
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        if let Some(p) = self.playback.take() {
            self.feed.enabled.store(false, Ordering::Release);
            let _ = p.pause();
            drop(p);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::Ordering;

    use super::{DSP_LATENCY_MS, MONITOR_HEADROOM_MS, Monitor, MonitorFeed, NOMINAL_PERIOD_MS};

    #[test]
    fn nonexistent_output_fails_with_a_reported_error() {
        let feed = MonitorFeed::new(48_000, 1036);
        let err = Monitor::start(
            &feed,
            &crate::audio_io::DeviceRef::Name("__tine_no_such_output_device_31415__".to_owned()),
        );
        let Err(err) = err else {
            panic!("expected a MonitorError for a nonexistent output device");
        };
        assert!(
            err.to_string().contains("playback failed"),
            "unexpected error text: {err}"
        );
        assert!(
            !feed.enabled.load(Ordering::Acquire),
            "feed must remain disabled when Monitor::start fails"
        );
    }

    /// Hardware soak (default 5 min, override with `TINE_SOAK_SECS`):
    /// captures default input into a `MonitorFeed` and plays to default
    /// output (or `TINE_HW_OUT`), then reports underruns, overruns and the
    /// measured latency badge. Run with:
    /// `cargo nextest run -p tine-engine --run-ignored only monitor_soak`.
    #[test]
    #[ignore = "opens real audio devices; run explicitly with --run-ignored"]
    fn monitor_soak() {
        let out_arg = std::env::var("TINE_HW_OUT").ok();
        let out_ref = out_arg
            .as_deref()
            .map_or(crate::audio_io::DeviceRef::Default, crate::audio_io::DeviceRef::parse);
        let secs: u64 = std::env::var("TINE_SOAK_SECS")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(300);

        let feed = MonitorFeed::new(48_000, 1036);
        let mut monitor = match Monitor::start(&feed, &out_ref) {
            Ok(m) => m,
            Err(e) => panic!("monitor failed to start on real device: {e}"),
        };

        let start = std::time::Instant::now();
        std::thread::sleep(std::time::Duration::from_secs(2));
        let warmup_stats = monitor.stats();
        let mut tick = 0_u64;
        while start.elapsed() < std::time::Duration::from_secs(secs) {
            std::thread::sleep(std::time::Duration::from_secs(5));
            tick += 5;
            let last_stats = monitor.stats();
            assert!(!last_stats.stream_lost, "stream lost during soak: {last_stats:?}");
            if tick.is_multiple_of(300) {
                println!(
                    "  soak +{tick}s: underruns={} overruns={} (cumulative callbacks {} vs warm-up {}) max_callback_us={}",
                    last_stats.underruns,
                    last_stats.overruns,
                    last_stats.callbacks,
                    warmup_stats.callbacks,
                    last_stats.max_callback_us
                );
            }
        }

        let final_stats = monitor.stats();
        let latency = monitor.latency_ms();
        println!(
            "monitor soak ({secs} s): underruns={} overruns={} callbacks={} max_callback_us={} capture={} Hz playback={} Hz latency badge={latency:.1} ms",
            final_stats.underruns,
            final_stats.overruns,
            final_stats.callbacks,
            final_stats.max_callback_us,
            final_stats.capture_rate_hz,
            final_stats.playback_rate_hz,
        );
        assert_eq!(
            final_stats.underruns, warmup_stats.underruns,
            "underruns after warm-up: the badge must stay clean over the soak"
        );
        assert_eq!(
            warmup_stats.underruns, 0,
            "priming must not count as underruns: {warmup_stats:?}"
        );
        monitor
            .restart(&feed, &out_ref)
            .unwrap_or_else(|e| panic!("restart after soak failed: {e}"));
        std::thread::sleep(std::time::Duration::from_millis(500));
        let _ = monitor.has_error();
        monitor.stop();
    }

    #[test]
    fn latency_badge_is_measured_capture_plus_headroom_plus_dsp() {
        let expected = NOMINAL_PERIOD_MS + MONITOR_HEADROOM_MS + DSP_LATENCY_MS;
        assert!((expected - 61.6).abs() < 0.1, "badge formula drifted: {expected}");
        let feed = MonitorFeed::new(48_000, 1036);
        assert_eq!(feed.rate_hz(), 48_000);
        assert_eq!(feed.dsp_latency(), 1036);
    }
}
