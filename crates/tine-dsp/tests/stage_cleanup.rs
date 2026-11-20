//! Stage 1 (Input) and Stage 2 (Denoise + Expander) verification tests.

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use assert_no_alloc::AllocDisabler;
use tine_dsp::denoise::Denoiser;
use tine_dsp::{DspPipeline, MeterSink, NoMeters, NoTap};
use tine_protocol::params::DspParams;
use tine_protocol::telemetry::AudioTelemetryFrame;

#[global_allocator]
static ALLOC: AllocDisabler = AllocDisabler;

#[derive(Default)]
struct RecordingMeters {
    frames: Vec<AudioTelemetryFrame>,
}

impl MeterSink for RecordingMeters {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.frames.push(*f);
    }
}

fn neutral_params() -> DspParams {
    let mut p = DspParams::DEFAULT;
    p.hpf_enabled = false;
    p.denoise_mix = 0.0;
    p.gate_enabled = false;
    p.deesser_enabled = false;
    p.comp_enabled = false;
    p
}

#[test]
fn hpf_100hz_attenuation_and_passband() {
    let fs = 48_000.0;
    let mut params = neutral_params();
    params.hpf_enabled = true;
    params.hpf_freq_hz = 100.0;

    let measure_gain_db = |freq_hz: f32| -> f32 {
        let mut pipe = DspPipeline::new(fs, 512);
        // 1.5 s sine so smoother (10 ms) and HPF transient settle completely.
        let mut buf = common::sine(freq_hz, fs, 72_000, 0.5);
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
        }
        let steady = &buf[48_000..];
        let in_rms = common::rms_db(&common::sine(freq_hz, fs, steady.len(), 0.5));
        common::rms_db(steady) - in_rms
    };

    let g50 = measure_gain_db(50.0);
    let g25 = measure_gain_db(25.0);
    let g1k = measure_gain_db(1_000.0);

    assert!(g50 <= -11.0, "50 Hz gain was {g50:.2} dB (expected <= -11 dB)");
    assert!(g25 <= -23.0, "25 Hz gain was {g25:.2} dB (expected <= -23 dB)");
    assert!(g1k.abs() <= 0.1, "1 kHz gain was {g1k:.3} dB (expected ±0.1 dB)");
}

#[test]
fn dc_offset_removed_within_200ms() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let params = neutral_params();

    // 300 ms of constant 0.1 DC offset
    let mut buf = vec![0.1f32; 14_400];
    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    // Account for pipeline latency: 200 ms after impulse/step emerges = latency + 9600 samples.
    let check_idx = pipe.latency_samples() as usize + 9_600;
    let residual = buf[check_idx].abs();
    assert!(
        residual < 0.001,
        "DC residual at 200 ms (idx {check_idx}) was {residual:.6} (expected < 0.001)"
    );
}

#[test]
fn denoise_reduces_noise_in_gaps_and_preserves_speech() {
    let fs = 48_000.0;
    let mut pipe = DspPipeline::new(fs, 512);
    let mut params = neutral_params();
    params.denoise_mix = 1.0;

    // Synthesize speech-like harmonic bursts (fundamental 150 Hz + formants) with 0.5 s gaps,
    // plus white noise at -50 dBFS.
    let n_total = 48_000 * 3; // 3 seconds
    let mut clean_speech = vec![0.0f32; n_total];
    for (i, slot) in clean_speech.iter_mut().enumerate() {
        let t = (i as f32) / fs;
        // Active speech in [0.5..1.5] and [2.0..2.8]; gaps in [0.0..0.5], [1.5..2.0], [2.8..3.0]
        let in_speech = (0.5..1.5).contains(&t) || (2.0..2.8).contains(&t);
        if in_speech {
            let f0 = 2.0 * std::f32::consts::PI * 150.0 * t;
            *slot = 0.12 * f0.sin() + 0.08 * (2.0 * f0).sin() + 0.05 * (4.0 * f0).sin() + 0.03 * (7.0 * f0).sin();
        }
    }

    // White noise at -50 dBFS RMS: uniform [-a, a] has RMS = a / sqrt(3), so a = 10^(-50/20) * sqrt(3).
    let noise_amp = 10.0f32.powf(-50.0 / 20.0) * 3.0f32.sqrt();
    let noise = common::white_noise(n_total, noise_amp, 0x1234_5678);

    let mut noisy: Vec<f32> = clean_speech.iter().zip(noise.iter()).map(|(&s, &n)| s + n).collect();
    let orig_noisy = noisy.clone();

    for chunk in noisy.chunks_mut(480) {
        pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
    }

    let lat = pipe.latency_samples() as usize;
    // Gap region: [1.6s .. 1.95s] shifted by latency
    let gap_start = (1.6 * fs) as usize + lat;
    let gap_end = (1.95 * fs) as usize + lat;
    let gap_in_rms = common::rms_db(&orig_noisy[(1.6 * fs) as usize..(1.95 * fs) as usize]);
    let gap_out_rms = common::rms_db(&noisy[gap_start..gap_end]);
    let noise_reduction = gap_in_rms - gap_out_rms;
    assert!(
        noise_reduction >= 15.0,
        "noise reduction in gap was {noise_reduction:.2} dB (in={gap_in_rms:.2}, out={gap_out_rms:.2})"
    );

    // Speech region: [0.7s .. 1.4s] shifted by latency
    let sp_in_rms = common::rms_db(&clean_speech[(0.7 * fs) as usize..(1.4 * fs) as usize]);
    let sp_out_rms = common::rms_db(&noisy[(0.7 * fs) as usize + lat..(1.4 * fs) as usize + lat]);
    let speech_diff = (sp_out_rms - sp_in_rms).abs();
    assert!(
        speech_diff <= 2.0,
        "speech-band RMS changed by {speech_diff:.2} dB (in={sp_in_rms:.2}, out={sp_out_rms:.2})"
    );
}

#[test]
fn latency_is_exact_and_constant_across_rates_and_toggles() {
    // Full-pipeline latency since T08: CleanupStage 960 (480-sample FIFO framing +
    // 480-sample `nnnoiseless` overlap-add synthesis delay) + LimiterStage 76
    // (1.5 ms lookahead = 72 samples + 4-sample true-peak FIR group delay).
    for &(fs, expected_48k) in &[(48_000.0f32, Some(1_036u32)), (44_100.0f32, None)] {
        let mut pipe = DspPipeline::new(fs, 512);
        let lat = pipe.latency_samples();
        if let Some(exp) = expected_48k {
            assert_eq!(lat, exp);
        } else {
            assert!(lat > 400 && lat < 1_500, "unexpected 44.1kHz latency: {lat}");
        }

        for &(denoise_mix, hpf_enabled, gate_enabled) in &[
            (0.0, false, false),
            (0.5, true, false),
            (1.0, false, true),
            (1.0, true, true),
        ] {
            pipe.reset();
            let mut params = neutral_params();
            params.denoise_mix = denoise_mix;
            params.hpf_enabled = hpf_enabled;
            params.gate_enabled = gate_enabled;

            let impulse_idx = 200usize;
            let mut buf = common::impulse(4_096, impulse_idx, 0.8);
            for chunk in buf.chunks_mut(137) {
                pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
            }

            let (peak_idx, peak_val) = buf
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.abs().total_cmp(&b.1.abs()))
                .expect("non-empty");
            assert!(peak_val.abs() > 0.05, "impulse vanished at fs={fs}, mix={denoise_mix}");
            let measured = (peak_idx - impulse_idx) as u32;
            assert!(
                measured.abs_diff(lat) <= 1,
                "fs={fs}, mix={denoise_mix}, hpf={hpf_enabled}, gate={gate_enabled}: measured={measured}, expected={lat}"
            );
        }
    }
}

#[test]
fn expander_attenuates_below_threshold_passes_above_and_hysteresis_prevents_chatter() {
    let fs = 48_000.0;
    let mut params = neutral_params();
    params.gate_enabled = true;
    params.gate_threshold_db = -50.0;
    params.gate_ratio = 4.0;
    params.gate_hysteresis_db = 4.0;
    params.gate_attack_ms = 1.0;
    params.gate_release_ms = 80.0;

    // 1. -60 dBFS noise -> >= 20 dB attenuation + telemetry reports attenuation
    {
        let mut pipe = DspPipeline::new(fs, 512);
        let mut meters = RecordingMeters::default();
        let amp = 10.0f32.powf(-60.0 / 20.0);
        let mut buf = common::white_noise(48_000, amp, 0xdead_beef);
        let in_rms = common::rms_db(&buf);
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut meters, &mut NoTap);
        }
        let out_rms = common::rms_db(&buf[24_000..]);
        let atten = in_rms - out_rms;
        assert!(
            atten >= 20.0,
            "expander attenuation was {atten:.2} dB (expected >= 20 dB)"
        );
        assert!(
            meters.frames.iter().any(|f| f.gate_attenuation_db <= -15.0),
            "expected gate_attenuation_db <= -15 in telemetry"
        );
    }

    // 2. Signal 5 dB above threshold (-45 dBFS peak) -> changes <= 0.5 dB
    {
        let mut pipe = DspPipeline::new(fs, 512);
        let amp = 10.0f32.powf(-45.0 / 20.0);
        let orig = common::sine(1_000.0, fs, 48_000, amp);
        let mut buf = orig.clone();
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
        }
        let in_rms = common::rms_db(&orig[24_000..]);
        let out_rms = common::rms_db(&buf[24_000..]);
        assert!(
            (out_rms - in_rms).abs() <= 0.5,
            "above-threshold signal changed by {:.3} dB",
            out_rms - in_rms
        );
    }

    // 3. Signal hovering ±1 dB around threshold (-50 dBFS) -> toggles state < 3 times in 2 s
    {
        let mut pipe = DspPipeline::new(fs, 512);
        let n = 96_000; // 2 seconds
        let mut buf = Vec::with_capacity(n);
        for i in 0..n {
            let t = (i as f32) / fs;
            // Slow 3 Hz amplitude modulation between -51 dBFS and -49 dBFS
            let mod_db = -50.0 + (2.0 * std::f32::consts::PI * 3.0 * t).sin();
            let amp = 10.0f32.powf(mod_db / 20.0);
            let carrier = (2.0 * std::f32::consts::PI * 1_000.0 * t).sin();
            buf.push(amp * carrier);
        }
        let mut meters = RecordingMeters::default();
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut meters, &mut NoTap);
        }

        // Count transitions between attenuated (< -3 dB) and open (>= -0.5 dB) after initial open.
        let mut toggles = 0usize;
        let mut was_attenuated: Option<bool> = None;
        for f in meters.frames.iter().skip(5) {
            let is_atten = f.gate_attenuation_db < -1.0;
            if let Some(prev) = was_attenuated
                && prev != is_atten
            {
                toggles += 1;
            }
            was_attenuated = Some(is_atten);
        }
        assert!(toggles < 3, "gate toggled {toggles} times in 2 s (expected < 3)");
    }
}

#[test]
fn zero_allocations_in_stage1_and_stage2_at_48k_and_44_1k() {
    for &fs in &[48_000.0f32, 44_100.0] {
        let max_block = 512usize;
        let mut pipe = DspPipeline::new(fs, max_block);
        let mut params = DspParams::DEFAULT;
        params.hpf_enabled = true;
        params.gate_enabled = true;
        params.denoise_mix = 0.75;

        let mut buf = vec![0.0f32; max_block];
        for i in 0..500usize {
            let len = (i * 137) % max_block + 1;
            let slice = &mut buf[..len];
            for (j, s) in slice.iter_mut().enumerate() {
                *s = ((j as f32) * 0.03).sin() * 0.2;
            }
            params.denoise_mix = if i % 50 < 25 { 0.8 } else { 0.0 };
            params.gate_enabled = i % 40 < 30;
            rt! {
                pipe.process(slice, &params, &mut NoMeters, &mut NoTap);
            }
        }
    }
}

struct MockVadDenoiser {
    vad_bits: Arc<AtomicU32>,
}

impl Denoiser for MockVadDenoiser {
    fn frame_len(&self) -> usize {
        480
    }

    fn native_rate(&self) -> u32 {
        48_000
    }

    fn algo_latency(&self) -> u32 {
        0
    }

    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32 {
        out.copy_from_slice(inp);
        f32::from_bits(self.vad_bits.load(Ordering::Relaxed))
    }

    fn reset(&mut self) {}
}

struct MinGateMeter {
    min_gate_gr_db: f32,
}

impl Default for MinGateMeter {
    fn default() -> Self {
        Self { min_gate_gr_db: 0.0 }
    }
}

impl MeterSink for MinGateMeter {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        if f.gate_attenuation_db < self.min_gate_gr_db {
            self.min_gate_gr_db = f.gate_attenuation_db;
        }
    }
}

#[test]
fn vad_low_raises_gate_threshold_for_non_speech_transients() {
    let fs = 48_000.0;
    let vad_bits = Arc::new(AtomicU32::new(1.0f32.to_bits()));
    let mut pipe = DspPipeline::with_denoiser(
        fs,
        512,
        Box::new(MockVadDenoiser {
            vad_bits: Arc::clone(&vad_bits),
        }),
    );

    let mut params = neutral_params();
    params.denoise_mix = 1.0;
    params.gate_enabled = true;
    params.gate_threshold_db = -40.0;
    params.gate_hysteresis_db = 4.0;
    params.gate_ratio = 4.0;
    params.gate_attack_ms = 1.0;
    params.gate_release_ms = 30.0;

    // -38 dBFS RMS 1 kHz sine: 2 dB above -40 dBFS gate threshold.
    let rms_amp = 10.0f32.powf(-38.0 / 20.0) * std::f32::consts::SQRT_2;

    // 1. Speech (VAD = 1.0): gate stays open.
    vad_bits.store(1.0f32.to_bits(), Ordering::Relaxed);
    let mut speech_buf = common::sine(1_000.0, fs, 48_000, rms_amp);
    let mut meters_speech = MinGateMeter::default();
    rt! {
        for chunk in speech_buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut meters_speech, &mut NoTap);
        }
    }
    let speech_out_rms = common::rms_db(&speech_buf[24_000..]);
    assert!(
        (speech_out_rms - (-38.0)).abs() <= 0.6,
        "speech should stay unattenuated: got {speech_out_rms:.2} dBFS"
    );

    // 2. Non-speech (VAD = 0.0): gate threshold rises up to +10 dB -> -38 dBFS is attenuated.
    vad_bits.store(0.0f32.to_bits(), Ordering::Relaxed);
    let mut click_buf = common::sine(1_000.0, fs, 48_000, rms_amp);
    let mut meters_click = MinGateMeter::default();
    rt! {
        for chunk in click_buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut meters_click, &mut NoTap);
        }
    }
    let click_out_rms = common::rms_db(&click_buf[36_000..]);
    assert!(
        click_out_rms < -50.0,
        "low-VAD non-speech signal should be attenuated by VAD boost: got {click_out_rms:.2} dBFS"
    );
    assert!(
        meters_click.min_gate_gr_db <= -12.0,
        "expected strong gate attenuation on low VAD (got {})",
        meters_click.min_gate_gr_db
    );
}

#[test]
fn vad_boost_inactive_when_denoise_mix_zero() {
    let fs = 48_000.0;
    let vad_bits = Arc::new(AtomicU32::new(0.0f32.to_bits()));
    let mut pipe = DspPipeline::with_denoiser(
        fs,
        512,
        Box::new(MockVadDenoiser {
            vad_bits: Arc::clone(&vad_bits),
        }),
    );

    let mut params = neutral_params();
    params.denoise_mix = 0.0;
    params.gate_enabled = true;
    params.gate_threshold_db = -40.0;
    params.gate_hysteresis_db = 4.0;
    params.gate_ratio = 4.0;
    params.gate_attack_ms = 1.0;
    params.gate_release_ms = 30.0;

    let rms_amp = 10.0f32.powf(-38.0 / 20.0) * std::f32::consts::SQRT_2;
    let mut buf = common::sine(1_000.0, fs, 48_000, rms_amp);
    let mut meters = MinGateMeter::default();
    rt! {
        for chunk in buf.chunks_mut(480) {
            pipe.process(chunk, &params, &mut meters, &mut NoTap);
        }
    }
    let out_rms = common::rms_db(&buf[24_000..]);
    assert!(
        (out_rms - (-38.0)).abs() <= 0.6,
        "denoise_mix=0.0 must disable VAD gate boost: got {out_rms:.2} dBFS"
    );
}

#[cfg(feature = "deepfilter")]
#[test]
fn deepfilter_reduces_noise_and_clicks_alloc_free() {
    use tine_protocol::params::DenoiseEngine;

    let fs = 48_000.0;
    let mut pipe = DspPipeline::for_engine(fs, 512, DenoiseEngine::DeepFilterNet3);
    assert_eq!(pipe.latency_samples(), 480 + 1440 + 76);

    let mut params = neutral_params();
    params.denoise_mix = 1.0;

    // 1. Steady background white/pink-ish noise at ~-30 dBFS must attenuate strongly after settle.
    let mut noise = common::pink_noise(48_000, 0.08, 0xCAFE_BABE);
    let in_noise_rms = common::rms_db(&noise[24_000..]);
    rt! {
        for chunk in noise.chunks_mut(480) {
            pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
        }
    }
    assert!(noise.iter().all(|s| s.is_finite()), "DeepFilter output must be finite");
    let out_noise_rms = common::rms_db(&noise[32_000..]);
    assert!(
        in_noise_rms - out_noise_rms >= 10.0,
        "DeepFilterNet3 must attenuate steady noise by >= 10 dB: in={in_noise_rms:.2}, out={out_noise_rms:.2}"
    );

    // Reset must also be allocation-free.
    rt! {
        pipe.reset();
    }
}

#[test]
fn bypass_denoiser_gives_76_sample_low_latency_and_runs_expander() {
    use tine_protocol::params::DenoiseEngine;

    let fs = 48_000.0;
    let mut pipe = DspPipeline::for_engine(fs, 512, DenoiseEngine::Off);
    assert_eq!(pipe.latency_samples(), 76);

    // 1. Impulse alignment test (all stages except limiter neutral).
    let mut neutral = neutral_params();
    neutral.gate_enabled = false;
    let mut impulse = vec![0.0_f32; 960];
    impulse[100] = 0.2;
    rt! {
        for chunk in impulse.chunks_mut(480) {
            pipe.process(chunk, &neutral, &mut NoMeters, &mut NoTap);
        }
    }
    let peak_idx = impulse
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .map_or(0, |(i, _)| i);
    assert_eq!(peak_idx, 100 + 76, "Off mode must delay by exactly 76 limiter samples");

    // 2. Stage 2b downward expander must still run in low-latency Off mode.
    pipe.reset();
    let mut gated = neutral_params();
    gated.gate_enabled = true;
    gated.gate_threshold_db = -30.0;
    gated.gate_hysteresis_db = 4.0;
    gated.gate_ratio = 4.0;
    gated.gate_attack_ms = 1.0;
    gated.gate_release_ms = 20.0;

    let quiet_amp = 10.0f32.powf(-45.0 / 20.0) * std::f32::consts::SQRT_2;
    let mut quiet = common::sine(1_000.0, fs, 48_000, quiet_amp);
    let mut meters = MinGateMeter::default();
    rt! {
        for chunk in quiet.chunks_mut(480) {
            pipe.process(chunk, &gated, &mut meters, &mut NoTap);
        }
    }
    let out_rms = common::rms_db(&quiet[36_000..]);
    assert!(
        out_rms < -55.0,
        "expander must attenuate in Off mode: got {out_rms:.2} dBFS"
    );
}

struct CountingDenoiser {
    calls: Arc<std::sync::atomic::AtomicUsize>,
    resets: Arc<std::sync::atomic::AtomicUsize>,
}

impl Denoiser for CountingDenoiser {
    fn frame_len(&self) -> usize {
        480
    }
    fn native_rate(&self) -> u32 {
        48_000
    }
    fn algo_latency(&self) -> u32 {
        480
    }
    fn process_frame(&mut self, inp: &[f32], out: &mut [f32]) -> f32 {
        self.calls.fetch_add(1, Ordering::Relaxed);
        let n = inp.len().min(out.len());
        out[..n].copy_from_slice(&inp[..n]);
        1.0
    }
    fn reset(&mut self) {
        self.resets.fetch_add(1, Ordering::Relaxed);
    }
}

#[test]
fn zero_mix_skips_neural_inference_calls_and_preserves_latency() {
    use std::sync::atomic::AtomicUsize;

    let calls = Arc::new(AtomicUsize::new(0));
    let resets = Arc::new(AtomicUsize::new(0));
    let mut pipe = DspPipeline::with_denoiser(
        48_000.0,
        512,
        Box::new(CountingDenoiser {
            calls: Arc::clone(&calls),
            resets: Arc::clone(&resets),
        }),
    );
    let expected_latency = pipe.latency_samples();
    assert_eq!(expected_latency, 960 + 76);

    let mut params = neutral_params();
    params.denoise_mix = 0.0;

    // Warm through the 10 ms smoother until mix settles at <= 1e-6 (~140 ms).
    let mut block = [0.05_f32; 480];
    rt! {
        for _ in 0..30 {
            block.fill(0.05);
            pipe.process(&mut block, &params, &mut NoMeters, &mut NoTap);
        }
    }
    let settled_calls = calls.load(Ordering::Relaxed);

    // Next 50 frames at mix=0.0 must invoke zero denoiser frames.
    rt! {
        for _ in 0..50 {
            block.fill(0.05);
            pipe.process(&mut block, &params, &mut NoMeters, &mut NoTap);
        }
    }
    assert_eq!(
        calls.load(Ordering::Relaxed),
        settled_calls,
        "denoise_mix=0.0 must skip neural inference calls once smoother settles"
    );

    // Impulse alignment must still equal `expected_latency` while idle.
    let mut impulse = vec![0.0_f32; 2_400];
    impulse[200] = 0.2;
    rt! {
        for chunk in impulse.chunks_mut(480) {
            pipe.process(chunk, &params, &mut NoMeters, &mut NoTap);
        }
    }
    let peak_idx = impulse
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .map_or(0, |(i, _)| i);
    assert_eq!(peak_idx, 200 + expected_latency as usize);

    // Raising mix > 0 resets cleanly and resumes inference calls.
    let resets_before_wake = resets.load(Ordering::Relaxed);
    params.denoise_mix = 0.5;
    rt! {
        for _ in 0..5 {
            block.fill(0.05);
            pipe.process(&mut block, &params, &mut NoMeters, &mut NoTap);
            assert!(block.iter().all(|s| s.is_finite()));
        }
    }
    assert!(
        resets.load(Ordering::Relaxed) > resets_before_wake,
        "waking denoiser must reset history"
    );
    assert!(
        calls.load(Ordering::Relaxed) > settled_calls,
        "waking denoiser must resume inference"
    );
}
