//! Reusable, real-time-safe `cpal` audio I/O layer for device enumeration,
//! capture, playback, format conversion, and adaptive clock-drift resampling.
//!
//! All audio callbacks are lock-free, allocation-free, and panic-free.

mod convert;
mod drift;
pub mod rt;

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{
    BufferSize, Device, DeviceId, SampleFormat, Stream, StreamConfig, SupportedBufferSize, SupportedStreamConfig,
};
use thiserror::Error;
use tine_protocol::sync::SpscRing;

use self::rt::{AtomicStreamStats, CaptureProcessor, PlaybackProcessor};

/// Default preallocated scratch frame capacity for audio callbacks.
const DEFAULT_MAX_FRAMES: usize = 8192;

/// Metadata for an audio input or output endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceInfo {
    /// Stable endpoint identifier (WASAPI endpoint ID via `cpal::Device::id()`,
    /// or `"name:<name>#<index>"` fallback if the backend does not expose an ID).
    pub id: String,
    /// Human-readable device name.
    pub name: String,
    /// Whether this device is currently the system default for its direction.
    pub is_default: bool,
    /// Default sample rate in Hz.
    pub default_rate: u32,
    /// Default channel count.
    pub channels: u16,
}

/// Selector for choosing an audio device.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum DeviceRef {
    /// Use the system default device.
    #[default]
    Default,
    /// Match by stable device ID (exact match on `cpal::DeviceId` string or raw backend ID).
    Id(String),
    /// Match by device name (exact case-insensitive first, then case-insensitive substring).
    Name(String),
}

impl DeviceRef {
    /// Parses a CLI argument into a [`DeviceRef`]: `"default"` selects [`DeviceRef::Default`],
    /// `"wasapi:..."` or `"{...}"` selects [`DeviceRef::Id`], otherwise [`DeviceRef::Name`].
    #[must_use]
    pub fn parse(arg: &str) -> Self {
        let trimmed = arg.trim();
        if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("default") {
            Self::Default
        } else if trimmed.starts_with("wasapi:") || (trimmed.starts_with('{') && trimmed.ends_with('}')) {
            Self::Id(trimmed.to_owned())
        } else {
            Self::Name(trimmed.to_owned())
        }
    }
}

/// Errors returned by [`tine_engine::audio_io`](self).
#[derive(Debug, Error)]
pub enum AudioIoError {
    /// No matching input or output device was found.
    #[error("audio device not found: {0}")]
    DeviceNotFound(String),
    /// Error returned by the `cpal` audio backend.
    #[error("cpal audio backend error: {0}")]
    Backend(#[from] cpal::Error),
}

/// Real-time stream health and timing statistics.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StreamStats {
    /// Cumulative playback underrun events (callbacks with buffer starvation after priming).
    pub underruns: u64,
    /// Cumulative dropped samples due to full capture ring.
    pub overruns: u64,
    /// Total audio callbacks executed.
    pub callbacks: u64,
    /// Maximum observed callback execution duration in microseconds.
    pub max_callback_us: u32,
    /// Non-fatal backend glitch notifications (xruns, reroutes) that did not
    /// require a rebuild.
    pub xruns: u64,
}

/// Active mono `f32` capture stream pushing into an [`SpscRing`].
pub struct CaptureStream {
    stream: Stream,
    sample_rate: u32,
    channels: u16,
    stats: Arc<AtomicStreamStats>,
    overruns_fn: Box<dyn Fn() -> u64 + Send + Sync>,
}

impl CaptureStream {
    /// Actual negotiated sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Actual hardware channel count before mono downmix.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// Returns a snapshot of stream statistics.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        self.stats.snapshot(Some((self.overruns_fn)()))
    }

    /// Returns `true` if the backend reported a fatal stream error (e.g., device
    /// unplugged). Benign xruns are counted in [`StreamStats::xruns`] instead.
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.stats.error_flag.load(Ordering::Acquire)
    }

    /// Starts or resumes the stream.
    pub fn play(&self) -> Result<(), AudioIoError> {
        Ok(self.stream.play()?)
    }

    /// Pauses the stream.
    pub fn pause(&self) -> Result<(), AudioIoError> {
        Ok(self.stream.pause()?)
    }
}

/// Active playback stream pulling mono `f32` from an [`SpscRing`] with adaptive clock-drift resampling.
pub struct PlaybackStream {
    stream: Stream,
    sample_rate: u32,
    channels: u16,
    stats: Arc<AtomicStreamStats>,
}

impl PlaybackStream {
    /// Actual negotiated output sample rate in Hz.
    #[must_use]
    pub const fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    /// Actual hardware output channel count.
    #[must_use]
    pub const fn channels(&self) -> u16 {
        self.channels
    }

    /// Returns a snapshot of stream statistics.
    #[must_use]
    pub fn stats(&self) -> StreamStats {
        self.stats.snapshot(None)
    }

    /// Returns `true` if the backend reported a fatal stream error (e.g., device
    /// unplugged). Benign xruns are counted in [`StreamStats::xruns`] instead.
    #[must_use]
    pub fn has_error(&self) -> bool {
        self.stats.error_flag.load(Ordering::Acquire)
    }

    /// Starts or resumes the stream.
    pub fn play(&self) -> Result<(), AudioIoError> {
        Ok(self.stream.play()?)
    }

    /// Pauses the stream.
    pub fn pause(&self) -> Result<(), AudioIoError> {
        Ok(self.stream.pause()?)
    }
}

/// Lists all available audio input devices.
pub fn list_inputs() -> Result<Vec<DeviceInfo>, AudioIoError> {
    let host = cpal::default_host();
    let default_id = host.default_input_device().and_then(|d| d.id().ok());
    let mut out = Vec::new();
    for (idx, dev) in host.input_devices()?.enumerate() {
        out.push(build_device_info(&dev, idx, default_id.as_ref(), true));
    }
    Ok(out)
}

/// Lists all available audio output devices.
pub fn list_outputs() -> Result<Vec<DeviceInfo>, AudioIoError> {
    let host = cpal::default_host();
    let default_id = host.default_output_device().and_then(|d| d.id().ok());
    let mut out = Vec::new();
    for (idx, dev) in host.output_devices()?.enumerate() {
        out.push(build_device_info(&dev, idx, default_id.as_ref(), false));
    }
    Ok(out)
}

fn build_device_info(dev: &Device, idx: usize, default_id: Option<&DeviceId>, is_input: bool) -> DeviceInfo {
    let name = dev
        .description()
        .map_or_else(|_| format!("Audio Device #{idx}"), |d| d.name().to_owned());
    let dev_id = dev.id().ok();
    let is_default = match (dev_id.as_ref(), default_id) {
        (Some(a), Some(b)) => a == b,
        _ => false,
    };
    let id = dev_id.map_or_else(|| format!("name:{name}#{idx}"), |d| d.to_string());

    let cfg = if is_input {
        dev.default_input_config().ok()
    } else {
        dev.default_output_config().ok()
    };
    let (default_rate, channels) = cfg.map_or((0, 0), |c| (c.sample_rate(), c.channels()));

    DeviceInfo {
        id,
        name,
        is_default,
        default_rate,
        channels,
    }
}

fn resolve_device(id: &DeviceRef, is_input: bool) -> Result<Device, AudioIoError> {
    let host = cpal::default_host();
    match id {
        DeviceRef::Default => {
            let dev = if is_input {
                host.default_input_device()
            } else {
                host.default_output_device()
            };
            dev.ok_or_else(|| AudioIoError::DeviceNotFound("default".to_owned()))
        }
        DeviceRef::Id(target) => {
            if let Ok(parsed) = target.parse::<DeviceId>()
                && let Some(dev) = host.device_by_id(&parsed)
            {
                return Ok(dev);
            }
            let devices = if is_input {
                host.input_devices()?
            } else {
                host.output_devices()?
            };
            for dev in devices {
                if let Ok(did) = dev.id()
                    && (did.to_string() == *target || did.id() == target)
                {
                    return Ok(dev);
                }
            }
            Err(AudioIoError::DeviceNotFound(target.clone()))
        }
        DeviceRef::Name(query) => {
            let q_lower = query.to_ascii_lowercase();
            let devices: Vec<Device> = if is_input {
                host.input_devices()?.collect()
            } else {
                host.output_devices()?.collect()
            };

            // Exact case-insensitive match first.
            for dev in &devices {
                if let Ok(desc) = dev.description()
                    && desc.name().to_ascii_lowercase() == q_lower
                {
                    return Ok(dev.clone());
                }
            }
            // Substring match fallback.
            for dev in devices {
                if let Ok(desc) = dev.description()
                    && desc.name().to_ascii_lowercase().contains(&q_lower)
                {
                    return Ok(dev);
                }
            }
            Err(AudioIoError::DeviceNotFound(query.clone()))
        }
    }
}

fn negotiate_input_config(dev: &Device, want_rate: Option<u32>) -> Result<SupportedStreamConfig, AudioIoError> {
    let default_cfg = dev.default_input_config()?;
    let Some(target_rate) = want_rate else {
        return Ok(default_cfg);
    };
    if default_cfg.sample_rate() == target_rate && default_cfg.sample_format() == SampleFormat::F32 {
        return Ok(default_cfg);
    }

    if let Ok(ranges) = dev.supported_input_configs() {
        let mut best: Option<SupportedStreamConfig> = None;
        for range in ranges {
            if range.min_sample_rate() <= target_rate && target_rate <= range.max_sample_rate() {
                let candidate = range.with_sample_rate(target_rate);
                let better = match &best {
                    None => true,
                    Some(prev) => {
                        candidate.sample_format() == SampleFormat::F32 && prev.sample_format() != SampleFormat::F32
                    }
                };
                if better {
                    best = Some(candidate);
                }
            }
        }
        if let Some(cfg) = best {
            return Ok(cfg);
        }
    }

    Ok(default_cfg)
}

fn max_frames_from_config(cfg: &SupportedStreamConfig) -> usize {
    match cfg.buffer_size() {
        SupportedBufferSize::Range { max, .. } => (*max as usize).max(DEFAULT_MAX_FRAMES),
        SupportedBufferSize::Unknown => DEFAULT_MAX_FRAMES,
    }
}

/// Opens a capture stream on `id`, downmixing to mono `f32`, invoking `on_block`,
/// and pushing processed samples into `ring`.
#[allow(clippy::needless_pass_by_value)] // Contract signature takes `Arc<SpscRing<N>>` by value.
pub fn open_capture<const N: usize>(
    id: &DeviceRef,
    want_rate: Option<u32>,
    ring: Arc<SpscRing<N>>,
    on_block: impl FnMut(&mut [f32]) + Send + 'static,
) -> Result<CaptureStream, AudioIoError> {
    let dev = resolve_device(id, true)?;
    let supported = negotiate_input_config(&dev, want_rate)?;
    let sample_rate = supported.sample_rate();
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let max_frames = max_frames_from_config(&supported);

    let config = StreamConfig {
        channels,
        sample_rate,
        buffer_size: BufferSize::Default,
    };

    let stats = Arc::new(AtomicStreamStats::new());
    let mut processor = CaptureProcessor::new(
        channels as usize,
        max_frames,
        Arc::clone(&ring),
        on_block,
        Arc::clone(&stats),
    );

    let err_stats = Arc::clone(&stats);
    let stream = dev.build_input_stream_raw(
        config,
        sample_format,
        move |data, _| {
            processor.process_data(data);
        },
        move |err: cpal::Error| {
            // RT-safe: atomics only, no alloc/log/panic. Benign xruns are
            // counted; only fatal kinds latch the rebuild flag.
            err_stats.record_stream_error(err.kind());
        },
        None,
    )?;

    stream.play()?;

    let ring_for_stats = Arc::clone(&ring);
    Ok(CaptureStream {
        stream,
        sample_rate,
        channels,
        stats,
        overruns_fn: Box::new(move || ring_for_stats.overruns()),
    })
}

/// Opens a playback stream on `id`, pulling mono `f32` from `ring` (at `src_rate`),
/// applying adaptive clock-drift resampling with the default headroom (`2.0` callbacks),
/// and upmixing to all output channels.
pub fn open_playback<const N: usize>(
    id: &DeviceRef,
    ring: Arc<SpscRing<N>>,
    src_rate: u32,
) -> Result<PlaybackStream, AudioIoError> {
    open_playback_with_headroom(id, ring, src_rate, 2.0)
}

/// Opens a playback stream on `id`, pulling mono `f32` from `ring` (at `src_rate`),
/// applying adaptive clock-drift resampling with a custom `headroom_callbacks` target
/// (e.g., `2.0` for virtual-cable routing, `3.0` for "Hear Myself" monitoring),
/// and upmixing to all output channels.
pub fn open_playback_with_headroom<const N: usize>(
    id: &DeviceRef,
    ring: Arc<SpscRing<N>>,
    src_rate: u32,
    headroom_callbacks: f32,
) -> Result<PlaybackStream, AudioIoError> {
    let dev = resolve_device(id, false)?;
    let supported = dev.default_output_config()?;
    let dst_rate = supported.sample_rate();
    let channels = supported.channels();
    let sample_format = supported.sample_format();
    let max_frames = max_frames_from_config(&supported);

    let config = StreamConfig {
        channels,
        sample_rate: dst_rate,
        buffer_size: BufferSize::Default,
    };

    let stats = Arc::new(AtomicStreamStats::new());
    let mut processor = PlaybackProcessor::with_headroom(
        channels as usize,
        src_rate,
        dst_rate,
        max_frames,
        ring,
        Arc::clone(&stats),
        headroom_callbacks,
    );

    let err_stats = Arc::clone(&stats);
    let stream = dev.build_output_stream_raw(
        config,
        sample_format,
        move |data, _| {
            processor.process_data(data);
        },
        move |err: cpal::Error| {
            // RT-safe: atomics only, no alloc/log/panic. Benign xruns are
            // counted; only fatal kinds latch the rebuild flag.
            err_stats.record_stream_error(err.kind());
        },
        None,
    )?;

    stream.play()?;

    Ok(PlaybackStream {
        stream,
        sample_rate: dst_rate,
        channels,
        stats,
    })
}

/// Constructs a [`CaptureProcessor`] directly for offline / `assert_no_alloc` testing.
#[must_use]
pub fn test_capture_processor<const N: usize, F>(
    channels: usize,
    max_frames: usize,
    ring: Arc<SpscRing<N>>,
    on_block: F,
) -> CaptureProcessor<N, F>
where
    F: FnMut(&mut [f32]) + Send + 'static,
{
    CaptureProcessor::new(channels, max_frames, ring, on_block, Arc::new(AtomicStreamStats::new()))
}

/// Constructs a [`PlaybackProcessor`] directly for offline / `assert_no_alloc` testing.
#[must_use]
pub fn test_playback_processor<const N: usize>(
    channels: usize,
    src_rate: u32,
    dst_rate: u32,
    max_frames: usize,
    ring: Arc<SpscRing<N>>,
) -> PlaybackProcessor<N> {
    PlaybackProcessor::new(
        channels,
        src_rate,
        dst_rate,
        max_frames,
        ring,
        Arc::new(AtomicStreamStats::new()),
    )
}
