//! Integration and acoustic verification tests for Stage 3 (5-band EQ),
//! Stage 4 (split-band de-esser), and `curve` (T07).

#![allow(clippy::missing_docs_in_private_items)]

#[macro_use]
mod common;

use std::f32::consts::PI;

use realfft::RealFftPlanner;
use tine_dsp::primitives::biquad::Coeffs;
use tine_dsp::primitives::db::{db_to_lin, lin_to_db};
use tine_dsp::{DspPipeline, MeterSink, NoMeters, NoTap, curve};
use tine_protocol::params::{DspParams, FilterType};
use tine_protocol::telemetry::AudioTelemetryFrame;

fn neutral_eq_params() -> DspParams {
    let mut p = DspParams::DEFAULT;
    p.hpf_enabled = false;
    p.denoise_mix = 0.0;
    p.gate_enabled = false;
    p.deesser_enabled = false;
    p.comp_enabled = false;
    p.limiter_ceiling_db = -0.1;
    p
}

fn measure_pipeline_sine_gain_db(pipe: &mut DspPipeline, params: &DspParams, freq_hz: f32, fs: f32) -> f32 {
    pipe.reset();
    let settle = ((fs * 0.4) as usize).max(4096);
    let cycles = (f64::from(freq_hz) * 0.25).ceil().max(1.0);
    let measure = ((cycles * f64::from(fs)) / f64::from(freq_hz)).round() as usize;
    let total = settle + measure;
    // -20 dBFS so the limiter never engages even with +12 dB EQ boost.
    let amp = 0.1f32;
    let mut buf = common::sine(freq_hz, fs, total, amp);
    let input_slice = buf[settle..].to_vec();

    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, params, &mut NoMeters, &mut NoTap);
    }

    let in_rms = common::rms_db(&input_slice);
    let out_rms = common::rms_db(&buf[settle..]);
    out_rms - in_rms
}

#[test]
fn all_eq_band_types_match_analytic_response() {
    let sample_rates = [44_100.0f32, 48_000.0, 96_000.0];
    let filter_types = [
        FilterType::LowShelf,
        FilterType::HighShelf,
        FilterType::Peaking,
        FilterType::Notch,
        FilterType::LowPass,
        FilterType::HighPass,
        FilterType::BandPass,
    ];

    // Log-spaced test frequencies from 20 Hz to 20 kHz (clamped below Nyquist).
    let mut base_freqs = [0.0f32; 12];
    let f_min = 20.0f32.ln();
    let f_max = 18_000.0f32.ln();
    for (i, slot) in base_freqs.iter_mut().enumerate() {
        let t = (i as f32) / 11.0;
        *slot = (f_min + t * (f_max - f_min)).exp();
    }

    for &fs in &sample_rates {
        let mut pipe = DspPipeline::new(fs, 480);
        for &ft in &filter_types {
            let mut p = neutral_eq_params();
            p.eq_type[2] = ft;
            p.eq_freq_hz[2] = 1_200.0;
            p.eq_gain_db[2] = 6.0;
            p.eq_q[2] = 1.2;

            let coeffs = Coeffs::design(ft, f64::from(fs), 1_200.0, 1.2, 6.0);
            for &f in &base_freqs {
                if f >= fs * 0.45 {
                    continue;
                }
                let expected = coeffs.magnitude_db(f64::from(f), f64::from(fs)) as f32;
                if expected <= -40.0 {
                    continue;
                }
                let measured = measure_pipeline_sine_gain_db(&mut pipe, &p, f, fs);
                assert_close_db!(measured, expected, 0.1);
            }
        }
    }
}

#[test]
fn curve_matches_measured_cascade_sweep_fft() {
    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);

    let mut p = neutral_eq_params();
    p.eq_type = [
        FilterType::LowShelf,
        FilterType::Peaking,
        FilterType::Peaking,
        FilterType::Peaking,
        FilterType::HighShelf,
    ];
    p.eq_freq_hz = [110.0, 320.0, 1_400.0, 4_200.0, 8_500.0];
    p.eq_gain_db = [3.5, -4.5, 2.5, 4.0, 3.0];
    p.eq_q = [0.707, 1.4, 1.2, 1.1, 0.707];

    // Measure impulse response via FFT (exact linear transfer function of the cascade).
    let n = 16_384usize;
    let mut buf = common::impulse(n, 0, 0.25);
    for chunk in buf.chunks_mut(480) {
        pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
    }
    // Undo the 0.25 impulse amplitude so FFT magnitude directly gives linear gain.
    for s in &mut buf {
        *s *= 4.0;
    }

    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);
    let mut spectrum = r2c.make_output_vec();
    r2c.process(&mut buf, &mut spectrum).expect("fft");

    let bin_hz = fs / (n as f32);
    let mut test_freqs = [0.0f32; 32];
    let f_min = 25.0f32.ln();
    let f_max = 18_000.0f32.ln();
    for (i, slot) in test_freqs.iter_mut().enumerate() {
        let t = (i as f32) / 31.0;
        *slot = (f_min + t * (f_max - f_min)).exp();
    }

    let mut curve_out = [0.0f32; 32];
    rt! {
        curve::response_curve(&p, fs, &test_freqs, &mut curve_out);
    }

    for (&f, &expected_db) in test_freqs.iter().zip(curve_out.iter()) {
        let bin = (f / bin_hz).round() as usize;
        let actual_freq = (bin as f32) * bin_hz;
        let measured_db = lin_to_db(spectrum[bin].norm());
        let analytic_at_bin = curve::response_db(&p, fs, actual_freq);
        assert_close_db!(measured_db, analytic_at_bin, 0.2);
        // Also ensure response_curve matches response_db at the exact point.
        assert_close_db!(expected_db, curve::response_db(&p, fs, f), 1e-4);
    }
}

#[test]
fn eq_frequency_sweep_has_no_clicks() {
    let fs = 48_000.0f32;
    let len = 48_000usize; // 1 second
    let amp = 0.25f32;
    let clean = common::sine(1_000.0, fs, len, amp);
    let mut max_ref_delta = 0.0f32;
    for w in clean.windows(2) {
        max_ref_delta = max_ref_delta.max((w[1] - w[0]).abs());
    }

    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.eq_type[2] = FilterType::Peaking;
    p.eq_freq_hz[2] = 100.0;
    p.eq_gain_db[2] = 6.0;
    p.eq_q[2] = 1.5;

    let mut out = Vec::with_capacity(len);
    let mut buf = [0.0f32; 480];
    let total_blocks = len / 480;

    // Warm up 1 block at 100 Hz so initial state is settled.
    buf.copy_from_slice(&clean[..480]);
    pipe.process(&mut buf, &p, &mut NoMeters, &mut NoTap);

    for b in 0..total_blocks {
        let t = (b as f32) / ((total_blocks - 1) as f32);
        // Exponential sweep 100 Hz -> 5000 Hz over 1 s
        p.eq_freq_hz[2] = 100.0 * (50.0f32).powf(t);
        let start = b * 480;
        buf.copy_from_slice(&clean[start..start + 480]);
        rt! {
            pipe.process(&mut buf, &p, &mut NoMeters, &mut NoTap);
        }
        out.extend_from_slice(&buf);
    }

    let mut max_sweep_delta = 0.0f32;
    for w in out.windows(2) {
        max_sweep_delta = max_sweep_delta.max((w[1] - w[0]).abs());
    }

    // With +6 dB peaking boost when the filter crosses 1 kHz, the signal amplitude
    // doubles (2x), so account for the +6 dB gain when comparing sample-to-sample delta,
    // or verify there is no discontinuity spike beyond 1.5x of the boosted sine's delta.
    let boosted_ref_delta = max_ref_delta * db_to_lin(6.0);
    assert!(
        max_sweep_delta <= boosted_ref_delta * 1.5,
        "sweep click detected: max_sweep_delta={max_sweep_delta:.5}, limit={:.5}",
        boosted_ref_delta * 1.5
    );
}

#[test]
fn eq_type_change_crossfades_without_clicks() {
    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.eq_type[2] = FilterType::Peaking;
    p.eq_freq_hz[2] = 1_000.0;
    p.eq_gain_db[2] = 6.0;

    let clean = common::sine(1_000.0, fs, 9_600, 0.25);
    let mut max_ref_delta = 0.0f32;
    for w in clean.windows(2) {
        max_ref_delta = max_ref_delta.max((w[1] - w[0]).abs());
    }

    let mut out = clean.clone();
    for (idx, chunk) in out.chunks_mut(480).enumerate() {
        if idx == 10 {
            p.eq_type[2] = FilterType::LowPass;
            p.eq_freq_hz[2] = 800.0;
        }
        rt! {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
    }

    let mut max_delta = 0.0f32;
    for w in out[480..].windows(2) {
        max_delta = max_delta.max((w[1] - w[0]).abs());
    }
    let limit = max_ref_delta * db_to_lin(6.0) * 1.5;
    assert!(
        max_delta <= limit,
        "type switch click: max_delta={max_delta:.5}, limit={limit:.5}"
    );
}

#[test]
fn deesser_idle_crossover_is_flat() {
    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.deesser_enabled = true;
    p.deesser_freq_hz = 6_800.0;
    p.deesser_thresh_db = 0.0; // idle: never triggers on -20 dBFS signals
    p.deesser_amount_db = 12.0;

    for &f in &[100.0f32, 500.0, 2_000.0, 6_800.0, 10_000.0, 15_000.0] {
        let gain_db = measure_pipeline_sine_gain_db(&mut pipe, &p, f, fs);
        assert_close_db!(gain_db, 0.0, 0.1);
    }
}

struct CaptureMeters {
    last: Option<AudioTelemetryFrame>,
}

impl MeterSink for CaptureMeters {
    fn publish(&mut self, f: &AudioTelemetryFrame) {
        self.last = Some(*f);
    }
}

fn band_rms_fft_db(samples: &[f32], fs: f32, f_lo: f32, f_hi: f32) -> f32 {
    let n = samples.len();
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(n);
    let mut input: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, &x)| {
            let w = 0.5 * (1.0 - (2.0 * PI * (i as f32) / ((n - 1) as f32)).cos());
            x * w
        })
        .collect();
    let mut spectrum = r2c.make_output_vec();
    r2c.process(&mut input, &mut spectrum).expect("fft");

    let bin_hz = fs / (n as f32);
    let lo_bin = ((f_lo / bin_hz).floor() as usize).max(1);
    let hi_bin = ((f_hi / bin_hz).ceil() as usize).min(spectrum.len() - 1);

    let sum_sq: f64 = spectrum[lo_bin..=hi_bin]
        .iter()
        .map(|c| {
            let m = f64::from(c.norm());
            m * m
        })
        .sum();
    lin_to_db((sum_sq.sqrt() as f32) / (n as f32))
}

#[test]
fn deesser_attenuates_synthetic_s_without_touching_200hz() {
    let fs = 48_000.0f32;
    let len = 24_000usize; // 0.5 s

    // 200 Hz vowel tone at -14 dBFS
    let tone = common::sine(200.0, fs, len, db_to_lin(-14.0));

    // Synthetic "S": deterministic sum of dense inharmonic sines in 6-9 kHz scaled to -10 dBFS RMS.
    let mut s_noise = vec![0.0f32; len];
    let num_partials = 40usize;
    for k in 0..num_partials {
        let f = 6_000.0 + (3_000.0 * (k as f32)) / ((num_partials - 1) as f32);
        let phase = ((k * 73 + 17) as f32) * 0.618_034;
        let w = 2.0 * PI * f / fs;
        for (n, slot) in s_noise.iter_mut().enumerate() {
            *slot += (w * (n as f32) + phase).sin();
        }
    }
    let cur_rms_lin = db_to_lin(common::rms_db(&s_noise));
    let target_rms_lin = db_to_lin(-10.0);
    let scale = target_rms_lin / cur_rms_lin;
    for s in &mut s_noise {
        *s *= scale;
    }

    let combined_in: Vec<f32> = tone.iter().zip(s_noise.iter()).map(|(&a, &b)| a + b).collect();
    let mut out = combined_in.clone();

    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.deesser_enabled = true;
    p.deesser_freq_hz = 5_800.0;
    p.deesser_thresh_db = -30.0;
    p.deesser_amount_db = 12.0;

    let mut meters = CaptureMeters { last: None };
    for chunk in out.chunks_mut(480) {
        rt! {
            pipe.process(chunk, &p, &mut meters, &mut NoTap);
        }
    }

    // Analyze steady-state second half (after attack/settle)
    let settle = 8_192usize;
    let in_steady = &combined_in[settle..];
    let out_steady = &out[settle..];

    let in_s_db = band_rms_fft_db(in_steady, fs, 6_000.0, 9_000.0);
    let out_s_db = band_rms_fft_db(out_steady, fs, 6_000.0, 9_000.0);
    let s_reduction = in_s_db - out_s_db;
    assert!(
        s_reduction >= 6.0,
        "expected >= 6 dB 6-9 kHz reduction, got {s_reduction:.2} dB"
    );

    let in_200_db = band_rms_fft_db(in_steady, fs, 180.0, 220.0);
    let out_200_db = band_rms_fft_db(out_steady, fs, 180.0, 220.0);
    assert_close_db!(out_200_db, in_200_db, 0.3);

    let frame = meters.last.expect("telemetry published");
    assert!(
        frame.deesser_reduction_db >= 6.0,
        "expected telemetry deesser_reduction_db >= 6.0, got {:.2}",
        frame.deesser_reduction_db
    );
}

#[test]
fn eq_gain_ramp_to_zero_settles_and_skips_without_clicks() {
    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.eq_type[1] = FilterType::Peaking;
    p.eq_freq_hz[1] = 1_000.0;
    p.eq_gain_db[1] = 6.0;

    let clean = common::sine(1_000.0, fs, 9_600, 0.25);
    let mut max_ref_delta = 0.0f32;
    for w in clean.windows(2) {
        max_ref_delta = max_ref_delta.max((w[1] - w[0]).abs());
    }

    let mut out = clean.clone();
    for (idx, chunk) in out.chunks_mut(480).enumerate() {
        if idx == 5 {
            p.eq_gain_db[1] = 0.0;
        }
        rt! {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
    }

    let mut max_delta = 0.0f32;
    for w in out[480..].windows(2) {
        max_delta = max_delta.max((w[1] - w[0]).abs());
    }
    let limit = max_ref_delta * db_to_lin(6.0) * 1.5;
    assert!(
        max_delta <= limit,
        "gain-to-zero click: max_delta={max_delta:.5}, limit={limit:.5}"
    );
}

#[test]
fn deesser_enable_toggle_crossfades_without_clicks() {
    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.deesser_enabled = true;
    p.deesser_freq_hz = 6_800.0;
    p.deesser_thresh_db = -30.0;
    p.deesser_amount_db = 12.0;

    let clean = common::sine(7_500.0, fs, 9_600, 0.25);
    let mut max_ref_delta = 0.0f32;
    for w in clean.windows(2) {
        max_ref_delta = max_ref_delta.max((w[1] - w[0]).abs());
    }

    let mut out = clean.clone();
    for (idx, chunk) in out.chunks_mut(480).enumerate() {
        if idx == 8 {
            p.deesser_enabled = false;
        }
        rt! {
            pipe.process(chunk, &p, &mut NoMeters, &mut NoTap);
        }
    }

    let mut max_delta = 0.0f32;
    for w in out[480..].windows(2) {
        max_delta = max_delta.max((w[1] - w[0]).abs());
    }
    assert!(
        max_delta <= max_ref_delta * 1.5,
        "deesser bypass click: max_delta={max_delta:.5}, limit={:.5}",
        max_ref_delta * 1.5
    );
}

#[test]
fn curve_includes_hpf_when_enabled() {
    let fs = 48_000.0f32;
    let mut p = neutral_eq_params();
    p.hpf_enabled = true;
    p.hpf_freq_hz = 100.0;

    // 2nd-order Butterworth HPF is -3.01 dB at cutoff and ~-12 dB/octave below.
    assert_close_db!(curve::response_db(&p, fs, 100.0), -3.01, 0.05);
    assert_close_db!(curve::response_db(&p, fs, 50.0), -12.3, 0.2);
    assert_close_db!(curve::response_db(&p, fs, 1_000.0), 0.0, 0.05);
}

#[test]
#[ignore = "perf measurement run explicitly under --profile perf"]
fn perf_eq_and_deesser_480_samples() {
    use std::hint::black_box;
    use std::time::Instant;

    let fs = 48_000.0f32;
    let mut pipe = DspPipeline::new(fs, 480);
    let mut p = neutral_eq_params();
    p.eq_gain_db = [2.0, -3.0, 1.5, 3.0, 2.5];
    p.deesser_enabled = true;
    p.deesser_freq_hz = 6_800.0;
    p.deesser_thresh_db = -30.0;
    p.deesser_amount_db = 6.0;

    let orig = common::sine(7_200.0, fs, 480, 0.25);
    let mut buf = orig.clone();
    let base_p = neutral_eq_params();

    // Warmup
    for _ in 0..1_000 {
        buf.copy_from_slice(&orig);
        pipe.process(black_box(&mut buf), black_box(&base_p), &mut NoMeters, &mut NoTap);
        pipe.process(black_box(&mut buf), black_box(&p), &mut NoMeters, &mut NoTap);
    }

    let iters = 50_000usize;
    let start_base = Instant::now();
    for _ in 0..iters {
        buf.copy_from_slice(&orig);
        pipe.process(black_box(&mut buf), black_box(&base_p), &mut NoMeters, &mut NoTap);
        black_box(&buf);
    }
    let base_us = (start_base.elapsed().as_secs_f64() * 1e6) / (iters as f64);

    let start_active = Instant::now();
    for _ in 0..iters {
        buf.copy_from_slice(&orig);
        pipe.process(black_box(&mut buf), black_box(&p), &mut NoMeters, &mut NoTap);
        black_box(&buf);
    }
    let active_us = (start_active.elapsed().as_secs_f64() * 1e6) / (iters as f64);

    let mut eq_only_p = base_p;
    eq_only_p.eq_gain_db = p.eq_gain_db;
    let start_eq = Instant::now();
    for _ in 0..iters {
        buf.copy_from_slice(&orig);
        pipe.process(black_box(&mut buf), black_box(&eq_only_p), &mut NoMeters, &mut NoTap);
        black_box(&buf);
    }
    let eq_us = (start_eq.elapsed().as_secs_f64() * 1e6) / (iters as f64) - base_us;
    let delta_us = active_us - base_us;
    let deesser_us = delta_us - eq_us;
    println!(
        "Baseline: {base_us:.3} us | EQ (5 bands): {eq_us:.3} us | De-esser: {deesser_us:.3} us | Both: {delta_us:.3} us (total {active_us:.3} us)"
    );
    assert!(delta_us < 10.0, "expected EQ + De-esser < 10 us, got {delta_us:.3} us");
}
