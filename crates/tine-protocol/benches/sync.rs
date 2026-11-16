//! Criterion benchmarks for `tine-protocol::sync` primitives.

use std::hint::black_box;

use criterion::{Criterion, criterion_group, criterion_main};
use tine_protocol::params::DspParams;
use tine_protocol::sync::{ParamBlock, SpscRing, Triple};
use tine_protocol::telemetry::AudioTelemetryFrame;

fn bench_param_block_unchanged(c: &mut Criterion) {
    let block = ParamBlock::new();
    let mut last_gen = 0u64;
    let mut out = DspParams::DEFAULT;
    // Prime `last_gen` to match current generation.
    let _ = block.load_if_changed(&mut last_gen, &mut out);

    c.bench_function("ParamBlock::load_if_changed (unchanged)", |b| {
        b.iter(|| {
            let changed = block.load_if_changed(black_box(&mut last_gen), black_box(&mut out));
            black_box(changed)
        });
    });
}

fn bench_triple_publish(c: &mut Criterion) {
    let triple = Triple::new(AudioTelemetryFrame::default());
    let mut frame = AudioTelemetryFrame {
        sequence: 1,
        input_rms_db: -24.0,
        output_rms_db: -18.0,
        ..AudioTelemetryFrame::default()
    };

    c.bench_function("Triple::publish", |b| {
        b.iter(|| {
            frame.sequence = frame.sequence.wrapping_add(1);
            triple.publish(black_box(&frame));
        });
    });
}

fn bench_spsc_push_480(c: &mut Criterion) {
    let ring: Box<SpscRing<32768>> = Box::new(SpscRing::new());
    let input = [0.25f32; 480];
    let mut drain = [0.0f32; 480];

    c.bench_function("SpscRing::push_slice(480)", |b| {
        b.iter(|| {
            let pushed = ring.push_slice(black_box(&input));
            let popped = ring.pop_slice(black_box(&mut drain));
            black_box((pushed, popped))
        });
    });
}

criterion_group!(
    benches,
    bench_param_block_unchanged,
    bench_triple_publish,
    bench_spsc_push_480
);
criterion_main!(benches);
