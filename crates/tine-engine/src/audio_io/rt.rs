//! Real-time callback processors for capture and playback streams.
//!
//! Separated from `cpal` stream setup so the exact callback paths can be verified
//! with `assert_no_alloc` in unit tests without requiring hardware audio devices.

#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::arithmetic_side_effects
)]

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::time::Instant;

use cpal::Data;
use tine_protocol::sync::SpscRing;

use super::StreamStats;
use super::convert::{downmix_to_mono, upmix_from_mono};
use super::drift::AdaptiveResampler;

/// Lock-free atomic counters updated from the RT callback and read from control threads.
pub(crate) struct AtomicStreamStats {
    pub(crate) underruns: AtomicU64,
    pub(crate) overruns: AtomicU64,
    pub(crate) callbacks: AtomicU64,
    pub(crate) max_callback_us: AtomicU32,
    pub(crate) error_flag: AtomicBool,
}

impl AtomicStreamStats {
    pub(crate) fn new() -> Self {
        Self {
            underruns: AtomicU64::new(0),
            overruns: AtomicU64::new(0),
            callbacks: AtomicU64::new(0),
            max_callback_us: AtomicU32::new(0),
            error_flag: AtomicBool::new(false),
        }
    }

    pub(crate) fn snapshot(&self, ring_overruns: Option<u64>) -> StreamStats {
        StreamStats {
            underruns: self.underruns.load(Ordering::Relaxed),
            overruns: ring_overruns.unwrap_or_else(|| self.overruns.load(Ordering::Relaxed)),
            callbacks: self.callbacks.load(Ordering::Relaxed),
            max_callback_us: self.max_callback_us.load(Ordering::Relaxed),
        }
    }

    #[inline]
    fn record_callback(&self, elapsed_us: u32) {
        self.callbacks.fetch_add(1, Ordering::Relaxed);
        self.max_callback_us.fetch_max(elapsed_us, Ordering::Relaxed);
    }
}

/// Real-time capture callback state machine.
pub struct CaptureProcessor<const N: usize, F>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    channels: usize,
    scratch: Box<[f32]>,
    ring: Arc<SpscRing<N>>,
    on_block: F,
    stats: Arc<AtomicStreamStats>,
}

impl<const N: usize, F> CaptureProcessor<N, F>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    /// Allocates scratch storage for `max_frames` mono samples.
    pub(crate) fn new(
        channels: usize,
        max_frames: usize,
        ring: Arc<SpscRing<N>>,
        on_block: F,
        stats: Arc<AtomicStreamStats>,
    ) -> Self {
        let cap = max_frames.max(256);
        Self {
            channels: channels.max(1),
            scratch: vec![0.0; cap].into_boxed_slice(),
            ring,
            on_block,
            stats,
        }
    }

    /// Processes one `cpal::Data` input buffer in real time (zero allocations).
    ///
    /// If `data` contains more frames than `self.scratch.len()`, it is processed in chunks.
    pub fn process_data(&mut self, data: &Data) {
        let start = Instant::now();
        let total_frames = data.len().checked_div(self.channels).unwrap_or(0);
        let mut offset = 0usize;

        while offset < total_frames {
            let chunk_cap = self.scratch.len().min(total_frames.saturating_sub(offset));
            let Some(mono_buf) = self.scratch.get_mut(..chunk_cap) else {
                break;
            };
            let converted = downmix_to_mono(data, self.channels, offset, mono_buf);
            if converted == 0 {
                break;
            }
            if let Some(active) = mono_buf.get_mut(..converted) {
                (self.on_block)(active);
                let pushed = self.ring.push_slice(active);
                if pushed < active.len() {
                    self.stats
                        .overruns
                        .fetch_add(active.len().saturating_sub(pushed) as u64, Ordering::Relaxed);
                }
            }
            offset = offset.saturating_add(converted);
        }

        let elapsed_us = u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX);
        self.stats.record_callback(elapsed_us);
    }
}

/// Real-time playback callback state machine.
pub struct PlaybackProcessor<const N: usize> {
    channels: usize,
    ring: Arc<SpscRing<N>>,
    resampler: AdaptiveResampler,
    pop_scratch: Box<[f32]>,
    mono_scratch: Box<[f32]>,
    stats: Arc<AtomicStreamStats>,
}

impl<const N: usize> PlaybackProcessor<N> {
    /// Allocates jitter buffer and scratch buffers once outside the RT thread,
    /// using the default [`super::drift::HEADROOM_CALLBACKS`] (`2.0` ≈ 20 ms).
    pub(crate) fn new(
        channels: usize,
        src_rate: u32,
        dst_rate: u32,
        max_frames: usize,
        ring: Arc<SpscRing<N>>,
        stats: Arc<AtomicStreamStats>,
    ) -> Self {
        Self::with_headroom(
            channels,
            src_rate,
            dst_rate,
            max_frames,
            ring,
            stats,
            super::drift::HEADROOM_CALLBACKS as f32,
        )
    }

    /// Allocates jitter buffer and scratch buffers once outside the RT thread
    /// with a configurable `headroom_callbacks` target (e.g. `2.0` for virtual
    /// cable routing, `3.0` for monitor).
    #[must_use]
    pub(crate) fn with_headroom(
        channels: usize,
        src_rate: u32,
        dst_rate: u32,
        max_frames: usize,
        ring: Arc<SpscRing<N>>,
        stats: Arc<AtomicStreamStats>,
        headroom_callbacks: f32,
    ) -> Self {
        let cap = max_frames.max(256);
        let jitter_cap = N.clamp(2048, 16384);
        Self {
            channels: channels.max(1),
            ring,
            resampler: AdaptiveResampler::with_headroom(src_rate, dst_rate, jitter_cap, f64::from(headroom_callbacks)),
            pop_scratch: vec![0.0; 1024].into_boxed_slice(),
            mono_scratch: vec![0.0; cap].into_boxed_slice(),
            stats,
        }
    }

    /// Renders one `cpal::Data` output buffer in real time (zero allocations).
    pub fn process_data(&mut self, data: &mut Data) {
        let start = Instant::now();
        let total_frames = data.len().checked_div(self.channels).unwrap_or(0);
        let desired_target = self.resampler.desired_target_for(total_frames);
        let was_primed = self.resampler.is_primed();

        // Drain available samples from SpscRing into the local jitter buffer.
        // While unprimed, drop any excess backlog down to `desired_target` so
        // startup queuing before playback's first callback does not inflate latency.
        loop {
            if !was_primed && self.resampler.fill_level() > desired_target {
                self.resampler.trim_to_target(desired_target);
            }
            let free = self.resampler.free();
            if free == 0 {
                break;
            }
            let want = free.min(self.pop_scratch.len());
            let Some(pop_buf) = self.pop_scratch.get_mut(..want) else {
                break;
            };
            let got = self.ring.pop_slice(pop_buf);
            if got == 0 {
                break;
            }
            if let Some(samples) = pop_buf.get(..got) {
                self.resampler.push_input(samples);
            }
            if got < want {
                break;
            }
        }
        if !was_primed && self.resampler.fill_level() > desired_target {
            self.resampler.trim_to_target(desired_target);
        }

        // Render in chunks bounded by mono_scratch (in case cpal sends a larger buffer).
        let mut any_underrun = false;

        if total_frames <= self.mono_scratch.len() {
            if let Some(mono) = self.mono_scratch.get_mut(..total_frames) {
                any_underrun = self.resampler.render(mono);
                upmix_from_mono(mono, self.channels, data);
            }
        } else {
            // Fallback for unexpectedly huge buffers: fill what fits and zero the rest.
            let cap = self.mono_scratch.len();
            if let Some(mono) = self.mono_scratch.get_mut(..cap) {
                any_underrun = self.resampler.render(mono);
                upmix_from_mono(mono, self.channels, data);
            }
        }

        if any_underrun {
            self.stats.underruns.fetch_add(1, Ordering::Relaxed);
        }

        let elapsed_us = u32::try_from(start.elapsed().as_micros()).unwrap_or(u32::MAX);
        self.stats.record_callback(elapsed_us);
    }

    /// Returns the current local jitter buffer fill level in samples (for testing/diagnostics).
    #[must_use]
    pub fn fill_level(&self) -> usize {
        self.resampler.fill_level()
    }

    /// Returns the current adaptive resampling ratio (for testing/diagnostics).
    #[must_use]
    pub fn last_ratio(&self) -> f64 {
        self.resampler.last_ratio()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resampler_trim_before_priming_drops_startup_backlog() {
        let mut resampler = AdaptiveResampler::with_headroom(48_000, 48_000, 16_384, 2.0);
        let desired_target = resampler.desired_target_for(480);
        assert_eq!(desired_target, 960);

        // Simulate 120 ms of startup backlog (5760 samples).
        let chunk = [0.25_f32; 480];
        for _ in 0..12 {
            resampler.push_input(&chunk);
            if !resampler.is_primed() && resampler.fill_level() > desired_target {
                resampler.trim_to_target(desired_target);
            }
        }
        assert_eq!(resampler.fill_level(), desired_target);

        let mut out = [0.0_f32; 480];
        assert!(!resampler.render(&mut out));
        assert!(resampler.is_primed());
        assert_eq!(resampler.fill_level(), 480);
    }
}
