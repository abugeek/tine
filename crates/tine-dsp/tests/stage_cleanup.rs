//! Stage 1 (Input) and Stage 2 (Denoise + Expander) verification tests.

mod common;

use assert_no_alloc::AllocDisabler;
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
    for &(fs, expected_48k) in &[(48_000.0f32, Some(960u32)), (44_100.0f32, None)] {
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
