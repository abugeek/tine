//! Built-in 5-second "Mic Check" (sample-aligned Raw vs Tine A/B playback).
//!
//! Captures 5 seconds of pre-DSP microphone audio through [`RawCaptureFeed`],
//! processes a second buffer offline through [`DspPipeline::for_engine`] trimmed by
//! [`DspPipeline::latency_samples`] so `raw[i]` and `processed[i]` align to the
//! exact sample, and plays the loop back with a 10 ms click-free linear crossfade
//! whenever the user switches between Tine and Raw Mic.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use tine_dsp::primitives::db::lin_to_db;
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_protocol::params::{DenoiseEngine, DspParams};
use tine_protocol::sync::SpscRing;

use crate::audio_io::{DeviceRef, open_playback_with_headroom};
use crate::virtual_cable::RawCaptureFeed;

/// Capture duration in seconds for one Mic Check recording.
pub const MIC_CHECK_SECONDS: u32 = 5;

/// Linear A/B crossfade duration in milliseconds.
const CROSSFADE_MS: f32 = 10.0;

/// Minimum integrated RMS threshold in dBFS to accept a recording.
const SILENCE_FLOOR_DB: f32 = -68.0;

/// Playback ring capacity (≈ 341 ms @ 48 kHz).
const PLAY_RING_CAP: usize = 16_384;

/// Which version is currently auditioned during Mic Check playback.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MicCheckMode {
    /// Post-DSP Tine-processed audio.
    Processed,
    /// Latency-aligned unprocessed microphone audio.
    Raw,
}

/// Snapshot of the Mic Check workflow.
#[derive(Clone, Debug, PartialEq)]
pub enum MicCheckState {
    /// No Mic Check is active.
    Idle,
    /// Capturing 5 seconds of raw audio from the active router tap.
    Recording {
        /// Capture progress in `0.0..=1.0`.
        progress: f32,
        /// Live RMS input level in dBFS.
        level_db: f32,
    },
    /// Looping the recorded snippet in either Tine or Raw mode.
    Playing {
        /// Current A/B listening mode.
        mode: MicCheckMode,
        /// Loop playback cursor progress in `0.0..=1.0`.
        progress: f32,
        /// Recorded duration in seconds.
        duration_s: f32,
    },
    /// Capture or playback failed with a user-facing message.
    Failed(String),
}

/// Processes `raw` offline through [`DspPipeline::for_engine`] and aligns the
/// processed buffer to zero sample offset relative to `raw`.
#[must_use]
pub fn prepare_ab_buffers(
    raw: &[f32],
    rate_hz: u32,
    params: &DspParams,
    engine: DenoiseEngine,
) -> (Vec<f32>, Vec<f32>) {
    if raw.is_empty() {
        return (Vec::new(), Vec::new());
    }
    let fs = (rate_hz.max(8_000)) as f32;
    let mut pipe = DspPipeline::for_engine(fs, 480, engine);
    let latency = pipe.latency_samples() as usize;

    let clean_raw: Vec<f32> = raw
        .iter()
        .map(|&s| if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 })
        .collect();

    let mut proc_params = *params;
    proc_params.bypass = false;

    let mut work = Vec::with_capacity(clean_raw.len().saturating_add(latency));
    work.extend_from_slice(&clean_raw);
    work.resize(clean_raw.len().saturating_add(latency), 0.0);

    for chunk in work.chunks_mut(480) {
        pipe.process(chunk, &proc_params, &mut NoMeters, &mut NoTap);
    }

    let aligned_end = latency.saturating_add(clean_raw.len()).min(work.len());
    let mut processed: Vec<f32> = work
        .get(latency..aligned_end)
        .unwrap_or(&[])
        .iter()
        .map(|&s| if s.is_finite() { s.clamp(-1.0, 1.0) } else { 0.0 })
        .collect();
    if processed.len() < clean_raw.len() {
        processed.resize(clean_raw.len(), 0.0);
    }
    (clean_raw, processed)
}

/// Click-free sample-by-sample looping A/B mixer with a 10 ms linear crossfade.
pub struct AbCrossfader {
    raw: Vec<f32>,
    processed: Vec<f32>,
    cursor: usize,
    raw_mix: f32,
    mix_step: f32,
}

impl AbCrossfader {
    /// Creates a new crossfader starting in `initial_mode`.
    #[must_use]
    pub fn new(raw: Vec<f32>, processed: Vec<f32>, rate_hz: u32, initial_mode: MicCheckMode) -> Self {
        let len = raw.len().min(processed.len());
        let mut r = raw;
        let mut p = processed;
        r.truncate(len);
        p.truncate(len);
        let fade_samples = (f64::from(CROSSFADE_MS) * 1e-3 * f64::from(rate_hz.max(8_000))).max(1.0);
        Self {
            raw: r,
            processed: p,
            cursor: 0,
            raw_mix: match initial_mode {
                MicCheckMode::Raw => 1.0,
                MicCheckMode::Processed => 0.0,
            },
            mix_step: (1.0 / fade_samples) as f32,
        }
    }

    /// Total loop length in samples.
    #[must_use]
    pub fn len(&self) -> usize {
        self.raw.len()
    }

    /// Hot-swaps the processed buffer in place while keeping the loop cursor
    /// continuous (pads with silence if shorter, truncates if longer).
    pub fn set_processed(&mut self, mut new_processed: Vec<f32>) {
        let len = self.raw.len();
        if new_processed.len() < len {
            new_processed.resize(len, 0.0);
        } else {
            new_processed.truncate(len);
        }
        self.processed = new_processed;
    }

    /// Returns `true` when both buffers are empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.raw.is_empty()
    }

    /// Current loop progress in `0.0..=1.0`.
    #[must_use]
    pub fn progress(&self) -> f32 {
        if self.raw.is_empty() {
            0.0
        } else {
            (self.cursor as f32 / self.raw.len() as f32).clamp(0.0, 1.0)
        }
    }

    /// Fills `out` by advancing through the loop and slewing `raw_mix` toward `target_raw`.
    pub fn render_chunk(&mut self, out: &mut [f32], target_raw: bool) {
        let len = self.raw.len();
        if len == 0 {
            out.fill(0.0);
            return;
        }
        let target = if target_raw { 1.0_f32 } else { 0.0_f32 };
        let step = self.mix_step;
        let mut mix = self.raw_mix;
        let mut idx = self.cursor;
        for dst in out.iter_mut() {
            if (mix - target).abs() <= step {
                mix = target;
            } else if mix < target {
                mix += step;
            } else {
                mix -= step;
            }
            let r = self.raw[idx];
            let p = self.processed[idx];
            *dst = p * (1.0 - mix) + r * mix;
            idx += 1;
            if idx >= len {
                idx = 0;
            }
        }
        self.raw_mix = mix;
        self.cursor = idx;
    }
}

/// Manages background 5 s capture and click-free looping A/B audition playback.
pub struct MicCheckRunner {
    stop: Arc<AtomicBool>,
    is_raw: Arc<AtomicBool>,
    state: Arc<Mutex<MicCheckState>>,
    feed: RawCaptureFeed,
    thread: Option<JoinHandle<()>>,
}

impl MicCheckRunner {
    /// Starts a 5-second Mic Check session capturing from `feed` and auditioning on `output_device`.
    #[must_use]
    pub fn start(feed: RawCaptureFeed, output_device: DeviceRef, params: DspParams, engine: DenoiseEngine) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let is_raw = Arc::new(AtomicBool::new(false));
        let state = Arc::new(Mutex::new(MicCheckState::Recording {
            progress: 0.0,
            level_db: -90.0,
        }));

        let stop_t = Arc::clone(&stop);
        let raw_t = Arc::clone(&is_raw);
        let state_t = Arc::clone(&state);
        let feed_t = feed.clone();

        let thread = thread::Builder::new()
            .name("tine-mic-check".to_owned())
            .spawn(move || {
                run_worker(&feed_t, &output_device, &params, engine, &stop_t, &raw_t, &state_t);
            })
            .ok();

        if thread.is_none()
            && let Ok(mut guard) = state.lock()
        {
            *guard = MicCheckState::Failed("Failed to spawn Mic Check audio worker thread.".to_owned());
        }

        Self {
            stop,
            is_raw,
            state,
            feed,
            thread,
        }
    }

    /// Returns a snapshot of the current Mic Check state.
    #[must_use]
    pub fn state(&self) -> MicCheckState {
        match self.state.lock() {
            Ok(guard) => guard.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    /// Switches A/B audition between Tine (`Processed`) and raw microphone (`Raw`).
    pub fn set_mode(&self, mode: MicCheckMode) {
        self.is_raw.store(mode == MicCheckMode::Raw, Ordering::Release);
    }

    /// Stops capture/playback and joins the background worker thread.
    pub fn stop(mut self) {
        self.signal_and_join();
    }

    fn signal_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.feed.set_enabled(false);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for MicCheckRunner {
    fn drop(&mut self) {
        self.signal_and_join();
    }
}

/// Loops a pre-recorded voice buffer (captured during Auto-Tune's Speak phase)
/// through an offline [`DspPipeline`], hot-swapping the processed audio in place
/// whenever the user selects a different Voice Style pill or toggles Raw vs. Tuned.
pub struct CalibrationPreviewPlayer {
    stop: Arc<AtomicBool>,
    is_raw: Arc<AtomicBool>,
    pending_update: Arc<Mutex<Option<(DspParams, DenoiseEngine)>>>,
    thread: Option<JoinHandle<()>>,
}

impl CalibrationPreviewPlayer {
    /// Starts looping `raw_voice` (sampled at `rate_hz`) to `output_device`,
    /// processed with `params` and `engine`, starting in Raw mode when `play_raw`
    /// is `true` or Tuned mode otherwise.
    #[must_use]
    pub fn start(
        raw_voice: Vec<f32>,
        rate_hz: u32,
        output_device: DeviceRef,
        params: DspParams,
        engine: DenoiseEngine,
        play_raw: bool,
    ) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let is_raw = Arc::new(AtomicBool::new(play_raw));
        let pending_update = Arc::new(Mutex::new(None));

        let stop_t = Arc::clone(&stop);
        let raw_t = Arc::clone(&is_raw);
        let pending_t = Arc::clone(&pending_update);

        let thread = thread::Builder::new()
            .name("tine-cal-preview".to_owned())
            .spawn(move || {
                run_calibration_preview(
                    &raw_voice,
                    rate_hz,
                    &output_device,
                    &params,
                    engine,
                    play_raw,
                    &stop_t,
                    &raw_t,
                    &pending_t,
                );
            })
            .ok();

        Self {
            stop,
            is_raw,
            pending_update,
            thread,
        }
    }

    /// Updates the auditioned DSP parameters, denoiser engine, and Raw/Tuned mode
    /// while keeping loop playback continuous.
    pub fn update(&self, params: DspParams, engine: DenoiseEngine, play_raw: bool) {
        self.is_raw.store(play_raw, Ordering::Release);
        if let Ok(mut guard) = self.pending_update.lock() {
            *guard = Some((params, engine));
        }
    }

    /// Returns `true` when the preview is currently auditioning the raw signal.
    #[must_use]
    pub fn is_raw(&self) -> bool {
        self.is_raw.load(Ordering::Acquire)
    }

    /// Signals the background playback thread to stop and joins it.
    pub fn stop(mut self) {
        self.signal_and_join();
    }

    fn signal_and_join(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(handle) = self.thread.take() {
            let _ = handle.join();
        }
    }
}

impl Drop for CalibrationPreviewPlayer {
    fn drop(&mut self) {
        self.signal_and_join();
    }
}

fn fill_playback_ring(
    play_ring: &SpscRing<PLAY_RING_CAP>,
    fader: &mut AbCrossfader,
    out_chunk: &mut [f32; 240],
    target_fill: usize,
    want_raw: bool,
) {
    let free = play_ring.free();
    let buffered = PLAY_RING_CAP.saturating_sub(free);
    if buffered < target_fill {
        let push_blocks = ((target_fill - buffered) / out_chunk.len()).max(1);
        for _ in 0..push_blocks {
            if play_ring.free() < out_chunk.len() {
                break;
            }
            fader.render_chunk(out_chunk, want_raw);
            let _ = play_ring.push_slice(out_chunk);
        }
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "Internal thread worker for CalibrationPreviewPlayer"
)]
fn run_calibration_preview(
    raw_voice: &[f32],
    rate_hz: u32,
    output_device: &DeviceRef,
    params: &DspParams,
    engine: DenoiseEngine,
    play_raw: bool,
    stop: &Arc<AtomicBool>,
    is_raw: &Arc<AtomicBool>,
    pending_update: &Arc<Mutex<Option<(DspParams, DenoiseEngine)>>>,
) {
    if raw_voice.is_empty() {
        return;
    }

    let safe_rate = rate_hz.max(8_000);
    let (raw_buf, proc_buf) = prepare_ab_buffers(raw_voice, rate_hz, params, engine);
    let initial_mode = if play_raw {
        MicCheckMode::Raw
    } else {
        MicCheckMode::Processed
    };
    let mut fader = AbCrossfader::new(raw_buf, proc_buf, rate_hz, initial_mode);

    let play_ring = Arc::new(SpscRing::<PLAY_RING_CAP>::new());
    let Ok(stream) = open_playback_with_headroom(output_device, Arc::clone(&play_ring), safe_rate, 3.0) else {
        return;
    };

    let mut out_chunk = [0.0_f32; 240];
    let target_fill = (safe_rate as usize / 20).clamp(1024, 4096);

    while !stop.load(Ordering::Acquire) && !stream.has_error() {
        if let Some((new_params, new_engine)) = pending_update.lock().ok().and_then(|mut g| g.take()) {
            let (_, new_proc) = prepare_ab_buffers(raw_voice, rate_hz, &new_params, new_engine);
            fader.set_processed(new_proc);
        }

        let want_raw = is_raw.load(Ordering::Acquire);
        fill_playback_ring(&play_ring, &mut fader, &mut out_chunk, target_fill, want_raw);
        thread::sleep(Duration::from_millis(5));
    }
}

fn set_state(slot: &Arc<Mutex<MicCheckState>>, next: MicCheckState) {
    if let Ok(mut guard) = slot.lock() {
        *guard = next;
    }
}

fn run_worker(
    feed: &RawCaptureFeed,
    output_device: &DeviceRef,
    params: &DspParams,
    engine: DenoiseEngine,
    stop: &Arc<AtomicBool>,
    is_raw: &Arc<AtomicBool>,
    state: &Arc<Mutex<MicCheckState>>,
) {
    let rate_hz = feed.rate_hz().max(8_000);
    let target_samples = (rate_hz as usize).saturating_mul(MIC_CHECK_SECONDS as usize);
    let mut recorded = Vec::with_capacity(target_samples);

    feed.drain();
    feed.set_enabled(true);

    let mut chunk = [0.0_f32; 1024];
    let mut last_sample_at = Instant::now();
    let deadline = Instant::now() + Duration::from_secs(10);

    while recorded.len() < target_samples {
        if stop.load(Ordering::Acquire) {
            feed.set_enabled(false);
            set_state(state, MicCheckState::Idle);
            return;
        }
        let n = feed.pop_slice(&mut chunk);
        if n > 0 {
            last_sample_at = Instant::now();
            let take = n.min(target_samples - recorded.len());
            let slice = &chunk[..take];
            recorded.extend_from_slice(slice);

            let mut sq = 0.0_f64;
            for &s in slice {
                sq += f64::from(s) * f64::from(s);
            }
            let rms = (sq / slice.len().max(1) as f64).sqrt() as f32;
            let level_db = lin_to_db(rms).clamp(-90.0, 0.0);
            let progress = (recorded.len() as f32 / target_samples as f32).clamp(0.0, 1.0);
            set_state(state, MicCheckState::Recording { progress, level_db });
        } else {
            if last_sample_at.elapsed() > Duration::from_secs(2) || Instant::now() >= deadline {
                feed.set_enabled(false);
                set_state(
                    state,
                    MicCheckState::Failed("No audio received from your microphone during Mic Check.".to_owned()),
                );
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
    }

    feed.set_enabled(false);

    if recorded.is_empty() {
        set_state(
            state,
            MicCheckState::Failed("Mic Check recording was empty.".to_owned()),
        );
        return;
    }

    let mut total_sq = 0.0_f64;
    for &s in &recorded {
        total_sq += f64::from(s) * f64::from(s);
    }
    let total_rms_db = lin_to_db((total_sq / recorded.len() as f64).sqrt() as f32);
    if total_rms_db < SILENCE_FLOOR_DB {
        set_state(
            state,
            MicCheckState::Failed(
                "Microphone sounded completely silent — speak into your mic and try again.".to_owned(),
            ),
        );
        return;
    }

    let (raw_buf, proc_buf) = prepare_ab_buffers(&recorded, rate_hz, params, engine);
    let duration_s = (raw_buf.len() as f32) / (rate_hz as f32);
    let initial_mode = if is_raw.load(Ordering::Acquire) {
        MicCheckMode::Raw
    } else {
        MicCheckMode::Processed
    };
    let mut fader = AbCrossfader::new(raw_buf, proc_buf, rate_hz, initial_mode);

    let play_ring = Arc::new(SpscRing::<PLAY_RING_CAP>::new());
    let stream = match open_playback_with_headroom(output_device, Arc::clone(&play_ring), rate_hz, 3.0) {
        Ok(s) => s,
        Err(e) => {
            set_state(
                state,
                MicCheckState::Failed(format!("Cannot open headphones/speakers for playback: {e}")),
            );
            return;
        }
    };

    let mut out_chunk = [0.0_f32; 240];
    let target_fill = (rate_hz as usize / 20).clamp(1024, 4096);

    while !stop.load(Ordering::Acquire) {
        if stream.has_error() {
            set_state(
                state,
                MicCheckState::Failed("Playback device disconnected during Mic Check.".to_owned()),
            );
            return;
        }

        let want_raw = is_raw.load(Ordering::Acquire);
        fill_playback_ring(&play_ring, &mut fader, &mut out_chunk, target_fill, want_raw);

        let mode = if want_raw {
            MicCheckMode::Raw
        } else {
            MicCheckMode::Processed
        };
        set_state(
            state,
            MicCheckState::Playing {
                mode,
                progress: fader.progress(),
                duration_s,
            },
        );
        thread::sleep(Duration::from_millis(5));
    }

    drop(stream);
    set_state(state, MicCheckState::Idle);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prepare_ab_buffers_is_sample_aligned_for_all_engines() {
        let rate = 48_000u32;
        let mut raw = vec![0.0_f32; 4_800];
        // Clean midband pulse with a unique center peak at sample 2120 so argmax is unambiguous.
        for i in 0..240 {
            let dt = (i as f32 - 120.0) / rate as f32;
            let env = (std::f32::consts::PI * i as f32 / 240.0).sin().powi(2);
            raw[2000 + i] = 0.25 * env * (2.0 * std::f32::consts::PI * 1000.0 * dt).cos();
        }
        let mut params = DspParams::DEFAULT;
        params.input_gain_db = 0.0;
        params.denoise_mix = 0.0;
        params.eq_gain_db = [0.0; 5];
        params.gate_enabled = false;
        params.comp_enabled = false;
        params.deesser_enabled = false;
        params.hpf_enabled = false;

        let raw_peak_idx = raw
            .iter()
            .enumerate()
            .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
            .map_or(0, |(i, _)| i);

        let engines = [
            DenoiseEngine::Rnnoise,
            DenoiseEngine::DeepFilterNet3,
            DenoiseEngine::Off,
        ];

        for engine in engines {
            let (raw_out, proc_out) = prepare_ab_buffers(&raw, rate, &params, engine);
            assert_eq!(raw_out.len(), raw.len());
            assert_eq!(proc_out.len(), raw.len());
            let proc_peak_idx = proc_out
                .iter()
                .enumerate()
                .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
                .map_or(0, |(i, _)| i);
            let lag = raw_peak_idx.abs_diff(proc_peak_idx);
            assert!(
                lag <= 1,
                "engine {engine:?} misaligned raw vs processed peak by {lag} samples (raw={raw_peak_idx}, proc={proc_peak_idx})"
            );
        }
    }

    #[test]
    fn ab_crossfader_switches_smoothly_without_step_discontinuity() {
        let raw = vec![0.6_f32; 4_800];
        let processed = vec![-0.6_f32; 4_800];
        let mut fader = AbCrossfader::new(raw, processed, 48_000, MicCheckMode::Processed);

        let mut buf = [0.0_f32; 960];
        fader.render_chunk(&mut buf[..480], false);
        assert!((buf[479] - (-0.6)).abs() < 1e-5);

        // Toggle to Raw: should take ~480 samples (10 ms) with max per-sample delta <= 0.01.
        fader.render_chunk(&mut buf[480..], true);
        let mut prev = buf[479];
        for (idx, &cur) in buf[480..].iter().enumerate() {
            let step = (cur - prev).abs();
            assert!(step < 0.01, "step discontinuity at sample {idx}: {step}");
            prev = cur;
        }
        assert!((buf[959] - 0.6).abs() < 1e-5, "must settle cleanly to Raw after 10 ms");
    }

    #[test]
    fn ab_crossfader_set_processed_preserves_cursor_and_length() {
        let len = 100usize;
        let raw = vec![0.1_f32; len];
        let processed = vec![0.5_f32; len];
        let mut fader = AbCrossfader::new(raw, processed, 48_000, MicCheckMode::Processed);

        let mut out = [0.0_f32; 25];
        fader.render_chunk(&mut out, false);
        assert!((fader.progress() - 0.25).abs() < 1e-6);
        assert!(out.iter().all(|&s| (s - 0.5).abs() < 1e-6));

        // Hot-swap with a shorter buffer: cursor stays at 25, len stays 100, tail padded with 0.0.
        let shorter = vec![0.8_f32; 40];
        fader.set_processed(shorter);
        assert_eq!(fader.len(), len);
        assert!((fader.progress() - 0.25).abs() < 1e-6);

        // Next 25 samples cover indices 25..50: indices 25..40 are 0.8, indices 40..50 are 0.0.
        fader.render_chunk(&mut out, false);
        assert!((fader.progress() - 0.50).abs() < 1e-6);
        for (i, &s) in out[..15].iter().enumerate() {
            assert!((s - 0.8).abs() < 1e-6, "expected 0.8 at offset {i}, got {s}");
        }
        for (i, &s) in out[15..].iter().enumerate() {
            assert!(s.abs() < 1e-6, "expected silence pad at offset {}, got {s}", 15 + i);
        }

        // Hot-swap with a longer buffer: cursor stays at 50, truncated to len (100).
        let longer: Vec<f32> = (0..150).map(|i| i as f32 * 0.005).collect();
        fader.set_processed(longer);
        assert_eq!(fader.len(), len);
        assert!((fader.progress() - 0.50).abs() < 1e-6);

        let mut out50 = [0.0_f32; 50];
        fader.render_chunk(&mut out50, false);
        // Wrapped back to 0 after rendering 50..100.
        assert!((fader.progress() - 0.0).abs() < 1e-6);
        for (i, &s) in out50.iter().enumerate() {
            let expected = (50 + i) as f32 * 0.005;
            assert!(
                (s - expected).abs() < 1e-6,
                "mismatch at index {}: {s} vs {expected}",
                50 + i
            );
        }
    }
}
