//! Virtual-cable DSP routing loop (`D-018`).
//!
//! Runs `DspPipeline` in-process inside `tine-engine`: captures mono `f32`
//! from the selected physical microphone via [`crate::audio_io::open_capture`],
//! processes it through [`tine_dsp::DspPipeline`] (reading [`ParamBlock`]
//! lock-free and publishing [`AudioTelemetryFrame`] + post-DSP audio tap), and
//! outputs continuously to the virtual audio cable sink via
//! [`crate::audio_io::open_playback`].
//!
//! All real-time callback paths live in the [`rt`] submodule and are strictly
//! lock-free, allocation-free, and panic-free.

use std::sync::Arc;

use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::AudioTelemetryFrame;

use crate::audio_io::{
    AudioIoError, CaptureStream, DeviceRef, PlaybackStream, StreamStats, list_inputs, open_capture, open_playback,
};
use crate::monitor::MonitorFeed;

/// Inter-stream ring capacity between capture+DSP and playback (32,768 samples ≈ 682 ms @ 48 kHz).
const AUDIO_RING_CAP: usize = 32_768;

/// Default maximum block size preallocated for [`tine_dsp::DspPipeline`].
const DEFAULT_DSP_MAX_BLOCK: usize = 8192;

/// Fallback sample rate when the input device's default rate cannot be queried up front.
const FALLBACK_SAMPLE_RATE: u32 = 48_000;

/// Real-time DSP callback processor for the virtual-cable routing loop.
///
/// Separated from `cpal` stream setup so the exact callback path can be verified
/// with `assert_no_alloc` in tests without requiring hardware audio devices.
pub mod rt {
    #![deny(
        clippy::unwrap_used,
        clippy::expect_used,
        clippy::panic,
        clippy::indexing_slicing,
        clippy::arithmetic_side_effects
    )]

    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

    use tine_dsp::{DspPipeline, MeterSink, TapSink};
    use tine_protocol::params::DspParams;
    use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
    use tine_protocol::telemetry::AudioTelemetryFrame;

    use crate::monitor::{MONITOR_RING_CAP, MonitorFeed};

    /// `MeterSink` publishing ~60 Hz telemetry frames into a lock-free [`Triple`].
    struct TripleMeterSink {
        triple: Arc<Triple<AudioTelemetryFrame>>,
    }

    impl MeterSink for TripleMeterSink {
        #[inline]
        fn publish(&mut self, f: &AudioTelemetryFrame) {
            self.triple.publish(f);
        }
    }

    /// `TapSink` pushing post-DSP mono samples into a lock-free [`SpscRing`].
    struct RingTapSink<const TAP_N: usize> {
        ring: Arc<SpscRing<TAP_N>>,
    }

    impl<const TAP_N: usize> TapSink for RingTapSink<TAP_N> {
        #[inline]
        fn push(&mut self, post: &[f32]) {
            let _ = self.ring.push_slice(post);
        }
    }

    /// Allocation-free, lock-free real-time DSP processor invoked from the capture callback.
    pub struct VirtualCableProcessor<const TAP_N: usize> {
        pipeline: DspPipeline,
        params: Arc<ParamBlock>,
        cached_params: DspParams,
        last_gen: u64,
        meters: TripleMeterSink,
        tap: RingTapSink<TAP_N>,
        monitor_ring: Arc<SpscRing<MONITOR_RING_CAP>>,
        monitor_on: Arc<AtomicBool>,
        block_frames: Arc<AtomicU32>,
    }

    impl<const TAP_N: usize> VirtualCableProcessor<TAP_N> {
        /// Allocates and prepares the [`DspPipeline`] once outside the real-time thread.
        #[must_use]
        pub fn new(
            sample_rate: f32,
            max_block: usize,
            params: Arc<ParamBlock>,
            telemetry: Arc<Triple<AudioTelemetryFrame>>,
            tap: Arc<SpscRing<TAP_N>>,
        ) -> Self {
            Self::with_monitor(sample_rate, max_block, params, telemetry, tap, &MonitorFeed::new(0, 0))
        }

        /// Allocates and prepares the [`DspPipeline`] once outside the real-time thread,
        /// wiring the post-DSP monitor tap to `monitor_feed`.
        #[must_use]
        pub fn with_monitor(
            sample_rate: f32,
            max_block: usize,
            params: Arc<ParamBlock>,
            telemetry: Arc<Triple<AudioTelemetryFrame>>,
            tap: Arc<SpscRing<TAP_N>>,
            monitor_feed: &MonitorFeed,
        ) -> Self {
            let mut cached_params = DspParams::DEFAULT;
            let mut last_gen = 0u64;
            params.load_if_changed(&mut last_gen, &mut cached_params);

            Self {
                pipeline: DspPipeline::new(sample_rate, max_block),
                params,
                cached_params,
                last_gen,
                meters: TripleMeterSink { triple: telemetry },
                tap: RingTapSink { ring: tap },
                monitor_ring: Arc::clone(&monitor_feed.ring),
                monitor_on: Arc::clone(&monitor_feed.enabled),
                block_frames: Arc::clone(&monitor_feed.block_frames),
            }
        }

        /// Total added DSP pipeline latency in samples at the negotiated sample rate.
        #[must_use]
        pub const fn latency_samples(&self) -> u32 {
            self.pipeline.latency_samples()
        }

        /// Builds a [`MonitorFeed`] sharing this processor's post-DSP monitor tap.
        #[must_use]
        pub(crate) fn monitor_feed(&self, rate_hz: u32) -> MonitorFeed {
            MonitorFeed::with_parts(
                rate_hz,
                self.latency_samples(),
                Arc::clone(&self.monitor_ring),
                Arc::clone(&self.monitor_on),
                Arc::clone(&self.block_frames),
            )
        }

        /// Processes one mono `f32` capture block in-place (zero allocations, zero locks).
        #[inline]
        pub fn process_block(&mut self, block: &mut [f32]) {
            self.params.load_if_changed(&mut self.last_gen, &mut self.cached_params);
            self.pipeline
                .process(block, &self.cached_params, &mut self.meters, &mut self.tap);
            if self.monitor_on.load(Ordering::Relaxed) {
                let _ = self.monitor_ring.push_slice(block);
                let frames = u32::try_from(block.len()).unwrap_or(u32::MAX);
                self.block_frames.store(frames, Ordering::Relaxed);
            }
        }
    }
    /// Allocation-free, lock-free handoff slot for publishing a [`VirtualCableProcessor`]
    /// to the capture callback once the hardware sample rate has been negotiated.
    pub(super) struct ProcessorSlot<const TAP_N: usize>(std::sync::atomic::AtomicPtr<VirtualCableProcessor<TAP_N>>);

    impl<const TAP_N: usize> ProcessorSlot<TAP_N> {
        pub(super) const fn new() -> Self {
            Self(std::sync::atomic::AtomicPtr::new(std::ptr::null_mut()))
        }

        /// Publishes `proc` for the real-time capture callback to claim once.
        pub(super) fn publish(&self, proc: Box<VirtualCableProcessor<TAP_N>>) {
            self.0.store(Box::into_raw(proc), std::sync::atomic::Ordering::Release);
        }

        /// Claims the published processor on the RT thread (zero allocations, zero locks).
        #[inline]
        pub(super) fn try_claim(&self) -> Option<Box<VirtualCableProcessor<TAP_N>>> {
            let ptr = self.0.swap(std::ptr::null_mut(), std::sync::atomic::Ordering::Acquire);
            if ptr.is_null() {
                None
            } else {
                // SAFETY: `ptr` was produced by `Box::into_raw` in `publish` and is
                // claimed at most once via atomic `swap(null_mut())`.
                Some(unsafe { Box::from_raw(ptr) })
            }
        }
    }

    impl<const TAP_N: usize> Drop for ProcessorSlot<TAP_N> {
        fn drop(&mut self) {
            let ptr = *self.0.get_mut();
            if !ptr.is_null() {
                // SAFETY: Any non-null pointer still in the slot was produced by
                // `Box::into_raw` in `publish` and never claimed; `&mut self`
                // guarantees exclusive access on drop.
                unsafe {
                    drop(Box::from_raw(ptr));
                }
            }
        }
    }
}

pub use rt::VirtualCableProcessor;

/// Active capture -> `DspPipeline` -> virtual-cable playback routing loop.
pub struct VirtualCableRouter {
    capture: CaptureStream,
    playback: PlaybackStream,
    monitor_feed: MonitorFeed,
}

impl VirtualCableRouter {
    /// Starts continuous capture from `input_id`, processes mono `f32` through
    /// [`tine_dsp::DspPipeline`], publishes telemetry to `telemetry` and post-DSP
    /// samples to `tap`, and plays the result to `output_id`.
    pub fn start<const TAP_N: usize>(
        input_id: &DeviceRef,
        output_id: &DeviceRef,
        params: Arc<ParamBlock>,
        telemetry: Arc<Triple<AudioTelemetryFrame>>,
        tap: Arc<SpscRing<TAP_N>>,
    ) -> Result<Self, AudioIoError> {
        let target_rate = resolve_input_rate(input_id).unwrap_or(FALLBACK_SAMPLE_RATE);
        let slot = Arc::new(rt::ProcessorSlot::<TAP_N>::new());
        let cb_slot = Arc::clone(&slot);
        let mut active: Option<Box<VirtualCableProcessor<TAP_N>>> = None;

        let audio_ring = Arc::new(SpscRing::<AUDIO_RING_CAP>::new());
        let capture = open_capture(input_id, Some(target_rate), Arc::clone(&audio_ring), move |block| {
            if active.is_none() {
                active = cb_slot.try_claim();
            }
            if let Some(processor) = active.as_mut() {
                processor.process_block(block);
            } else {
                // Silence until `ProcessorSlot` is published so unprocessed raw
                // microphone audio never leaks into the virtual cable on startup.
                block.fill(0.0);
            }
        })?;

        // Construct `DspPipeline` with the actual hardware capture rate negotiated by
        // `open_capture`, then publish it lock-free to the capture callback.
        let actual_in_rate = capture.sample_rate();
        let processor = Box::new(VirtualCableProcessor::new(
            actual_in_rate as f32,
            DEFAULT_DSP_MAX_BLOCK,
            params,
            telemetry,
            tap,
        ));
        let monitor_feed = processor.monitor_feed(actual_in_rate);
        slot.publish(processor);

        let playback = open_playback(output_id, audio_ring, actual_in_rate)?;

        Ok(Self {
            capture,
            playback,
            monitor_feed,
        })
    }

    /// Returns a [`MonitorFeed`] handle wired to this router's post-DSP output.
    #[must_use]
    pub fn monitor_feed(&self) -> MonitorFeed {
        self.monitor_feed.clone()
    }

    /// Stops both capture and playback streams and releases audio resources.
    pub fn stop(self) {
        let _ = self.capture.pause();
        let _ = self.playback.pause();
        drop(self.capture);
        drop(self.playback);
    }

    /// Returns combined real-time stream health and timing statistics.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        let c = self.capture.stats();
        let p = self.playback.stats();
        StreamStats {
            underruns: p.underruns,
            overruns: c.overruns,
            callbacks: c.callbacks.saturating_add(p.callbacks),
            max_callback_us: c.max_callback_us.max(p.max_callback_us),
        }
    }

    /// Returns capture-side stream statistics (includes DSP processing time).
    #[must_use]
    pub fn capture_stats(&self) -> StreamStats {
        self.capture.stats()
    }

    /// Returns playback-side stream statistics.
    #[must_use]
    pub fn playback_stats(&self) -> StreamStats {
        self.playback.stats()
    }

    /// Returns `true` if either the capture or playback stream reported a backend error.
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.capture.has_error() || self.playback.has_error()
    }

    /// Negotiated capture sample rate in Hz.
    #[must_use]
    pub const fn input_rate(&self) -> u32 {
        self.capture.sample_rate()
    }

    /// Negotiated playback sample rate in Hz.
    #[must_use]
    pub const fn output_rate(&self) -> u32 {
        self.playback.sample_rate()
    }
}

fn resolve_input_rate(input_id: &DeviceRef) -> Option<u32> {
    let inputs = list_inputs().ok()?;
    let matched = match input_id {
        DeviceRef::Default => inputs.iter().find(|d| d.is_default),
        DeviceRef::Id(target) => inputs.iter().find(|d| d.id == *target),
        DeviceRef::Name(query) => {
            let q = query.to_ascii_lowercase();
            inputs
                .iter()
                .find(|d| d.name.to_ascii_lowercase() == q)
                .or_else(|| inputs.iter().find(|d| d.name.to_ascii_lowercase().contains(&q)))
        }
    }?;
    (matched.default_rate > 0).then_some(matched.default_rate)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn start_returns_device_not_found_for_missing_input() {
        let params = Arc::new(ParamBlock::new());
        let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
        let tap = Arc::new(SpscRing::<1024>::new());

        let Err(err) = VirtualCableRouter::start(
            &DeviceRef::Name("__nonexistent_tine_input_device_12345__".to_owned()),
            &DeviceRef::Default,
            params,
            telemetry,
            tap,
        ) else {
            panic!("expected DeviceNotFound error")
        };
        assert!(matches!(err, AudioIoError::DeviceNotFound(_)));
    }

    #[test]
    fn processor_slot_claims_once_and_cleans_up_unclaimed() {
        let params = Arc::new(ParamBlock::new());
        let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
        let tap = Arc::new(SpscRing::<1024>::new());

        let slot = rt::ProcessorSlot::<1024>::new();
        assert!(slot.try_claim().is_none());

        slot.publish(Box::new(VirtualCableProcessor::new(
            44_100.0,
            512,
            Arc::clone(&params),
            Arc::clone(&telemetry),
            Arc::clone(&tap),
        )));
        let mut claimed = slot.try_claim().expect("published processor should be claimed");
        assert!(slot.try_claim().is_none());

        let mut block = [0.0f32; 64];
        claimed.process_block(&mut block);

        // Dropping a populated slot without claiming must not leak or panic.
        let unclaimed_slot = rt::ProcessorSlot::<1024>::new();
        unclaimed_slot.publish(Box::new(VirtualCableProcessor::new(
            48_000.0, 512, params, telemetry, tap,
        )));
        drop(unclaimed_slot);
    }

    #[test]
    fn monitor_ring_receives_samples_only_when_enabled_zero_alloc() {
        use std::sync::atomic::Ordering;

        let params = Arc::new(ParamBlock::new());
        let telemetry = Arc::new(Triple::new(AudioTelemetryFrame::default()));
        let tap = Arc::new(SpscRing::<4096>::new());
        let feed = MonitorFeed::new(48_000, 1036);

        let mut proc = VirtualCableProcessor::with_monitor(48_000.0, 512, params, telemetry, tap, &feed);
        assert_eq!(proc.latency_samples(), 1036);

        let mut block = [0.25_f32; 480];
        let mut drained = [0.0_f32; 1024];

        // While monitor_on is false, process_block must not push to monitor_ring.
        assert_no_alloc::assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
        assert_eq!(feed.ring.pop_slice(&mut drained), 0);
        assert_eq!(feed.block_frames.load(Ordering::Relaxed), 0);

        // Enable monitor_on and verify post-DSP samples + block_frames are recorded with zero allocs.
        feed.enabled.store(true, Ordering::Release);
        block.fill(0.25);
        assert_no_alloc::assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
        let got = feed.ring.pop_slice(&mut drained);
        assert_eq!(got, 480);
        assert_eq!(feed.block_frames.load(Ordering::Relaxed), 480);
        assert_eq!(drained[..480], block[..]);

        // Disable monitor_on again and verify no further samples enter monitor_ring.
        feed.enabled.store(false, Ordering::Release);
        assert_no_alloc::assert_no_alloc(|| {
            proc.process_block(&mut block);
        });
        assert_eq!(feed.ring.pop_slice(&mut drained), 0);
    }
}
