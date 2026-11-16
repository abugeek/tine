//! Criterion benchmark for `DspPipeline` (480-sample frames at 48 kHz and 44.1 kHz).

#![allow(missing_docs)]

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tine_dsp::{DspPipeline, NoMeters, NoTap};
use tine_protocol::params::DspParams;

fn bench_pipeline_rate(c: &mut Criterion, name: &str, sample_rate: f32) {
    let mut pipe = DspPipeline::new(sample_rate, 480);
    let mut params = DspParams::DEFAULT;
    params.hpf_enabled = true;
    params.gate_enabled = true;
    params.deesser_enabled = true;
    params.comp_enabled = true;
    params.denoise_mix = 0.5;

    let orig: Vec<f32> = (0..480).map(|i| ((i as f32) * 0.05).sin() * 0.25).collect();
    let mut buf = orig.clone();

    c.bench_function(name, |b| {
        b.iter(|| {
            buf.copy_from_slice(&orig);
            pipe.process(black_box(&mut buf), black_box(&params), &mut NoMeters, &mut NoTap);
            black_box(&buf);
        });
    });
}

fn pipeline_benches(c: &mut Criterion) {
    bench_pipeline_rate(c, "pipeline_480_48khz", 48_000.0);
    bench_pipeline_rate(c, "pipeline_480_44_1khz", 44_100.0);
}

criterion_group!(benches, pipeline_benches);
criterion_main!(benches);
