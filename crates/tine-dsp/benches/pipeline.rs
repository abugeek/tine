//! Criterion benchmark for `DspPipeline` (480-sample frames at 48 kHz and 44.1 kHz).
//!
//! `pipeline_480_*` is the hard CI perf gate (< 150 µs per 480-sample block,
//! see `docs/project/09-engineering-standards.md` §7). The `stage_*` group
//! measures the marginal cost of each stage on top of a neutral baseline
//! (all stages disabled; `RNNoise` still runs every block by design, so its
//! cost is part of the baseline and of the full chain).
//!
//! Run with: `cargo bench -p tine-dsp --profile perf`

#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_protocol::params::{DenoiseEngine, DspParams, FilterType};

/// Deterministic 480-sample speech-ish block (pink-ish noise + 220 Hz hum).
fn bench_input(seed: u64, amp: f32) -> Vec<f32> {
    let mut rng = seed;
    let mut out = Vec::with_capacity(480);
    let mut b = 0.0f32;
    for i in 0..480 {
        // xorshift64
        rng ^= rng >> 12;
        rng ^= rng << 25;
        rng ^= rng >> 27;
        rng = rng.wrapping_mul(0x2545_f491_4f6c_dd1d);
        let w = ((rng >> 40) as f32 / 16_777_216.0) * 2.0 - 1.0;
        b = 0.99 * b + w * 0.1;
        let hum = 0.3 * (2.0 * std::f32::consts::PI * 220.0 * (i as f32) / 48_000.0).sin();
        out.push((b + hum) * amp);
    }
    out
}

fn full_chain_params() -> DspParams {
    let mut params = DspParams::DEFAULT;
    params.hpf_enabled = true;
    params.gate_enabled = true;
    params.deesser_enabled = true;
    params.comp_enabled = true;
    params.denoise_mix = 0.5;
    params
}

fn bench_pipeline_rate(c: &mut Criterion, name: &str, sample_rate: f32) {
    bench_pipeline_engine(c, name, sample_rate, DenoiseEngine::Rnnoise);
}

fn bench_pipeline_engine(c: &mut Criterion, name: &str, sample_rate: f32, engine: DenoiseEngine) {
    let mut pipe = DspPipeline::for_engine(sample_rate, 480, engine);
    let params = full_chain_params();

    let orig = bench_input(0x1234_5678_9abc_def0, 0.25);
    let mut buf = orig.clone();

    c.bench_function(name, |b| {
        b.iter(|| {
            buf.copy_from_slice(&orig);
            pipe.process(black_box(&mut buf), black_box(&params), &mut NoMeters, &mut NoTap);
            black_box(&buf);
        });
    });
}

/// Benchmarks the marginal cost of each stage (baseline = all stages disabled;
/// `RNNoise` always runs so its cost sits in the baseline).
fn bench_stage_breakdown(c: &mut Criterion) {
    let sample_rate = 48_000.0f32;
    let fs = sample_rate;
    let orig = bench_input(0x1234_5678_9abc_def0, 0.25);

    let case = |c: &mut Criterion, name: &str, params: &DspParams| {
        let mut pipe = DspPipeline::new(fs, 480);
        let mut buf = orig.clone();
        c.bench_function(name, |b| {
            b.iter(|| {
                buf.copy_from_slice(&orig);
                pipe.process(black_box(&mut buf), black_box(params), &mut NoMeters, &mut NoTap);
                black_box(&buf);
            });
        });
    };

    let mut neutral = DspParams::DEFAULT;
    neutral.hpf_enabled = false;
    neutral.gate_enabled = false;
    neutral.deesser_enabled = false;
    neutral.comp_enabled = false;
    neutral.eq_gain_db = [0.0; 5];
    neutral.denoise_mix = 0.0;
    neutral.limiter_true_peak = false;
    case(c, "breakdown_48k_baseline_neutral", &neutral);

    let one = |neutral: &DspParams, f: fn(&mut DspParams)| {
        let mut p = *neutral;
        f(&mut p);
        p
    };

    case(
        c,
        "breakdown_48k_hpf",
        &one(&neutral, |p| {
            p.hpf_enabled = true;
            p.hpf_freq_hz = 80.0;
        }),
    );
    case(
        c,
        "breakdown_48k_denoiser_full_mix",
        &one(&neutral, |p| {
            p.denoise_mix = 1.0;
        }),
    );
    case(
        c,
        "breakdown_48k_expander_gate",
        &one(&neutral, |p| {
            p.gate_enabled = true;
            p.gate_threshold_db = -50.0;
        }),
    );
    case(
        c,
        "breakdown_48k_eq_5_band",
        &one(&neutral, |p| {
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
        }),
    );
    case(
        c,
        "breakdown_48k_deesser",
        &one(&neutral, |p| {
            p.deesser_enabled = true;
            p.deesser_freq_hz = 6_800.0;
            p.deesser_thresh_db = -30.0;
        }),
    );
    case(
        c,
        "breakdown_48k_compressor",
        &one(&neutral, |p| {
            p.comp_enabled = true;
            p.comp_thresh_db = -24.0;
            p.comp_ratio = 4.0;
        }),
    );
    case(
        c,
        "breakdown_48k_limiter_true_peak",
        &one(&neutral, |p| {
            p.limiter_true_peak = true;
        }),
    );
}

fn pipeline_benches(c: &mut Criterion) {
    bench_pipeline_rate(c, "pipeline_480_48khz", 48_000.0);
    bench_pipeline_rate(c, "pipeline_480_44_1khz", 44_100.0);
    #[cfg(feature = "deepfilter")]
    bench_pipeline_engine(
        c,
        "pipeline_480_48khz_deepfilter",
        48_000.0,
        DenoiseEngine::DeepFilterNet3,
    );
    bench_pipeline_engine(c, "pipeline_480_48khz_low_latency", 48_000.0, DenoiseEngine::Off);
    bench_pipeline_engine(c, "pipeline_480_48khz_low_latency", 48_000.0, DenoiseEngine::Off);
    bench_stage_breakdown(c);
}

criterion_group!(benches, pipeline_benches);
criterion_main!(benches);
