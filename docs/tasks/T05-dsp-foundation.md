# T05 — DSP Primitives, Test/Bench Harness, `DspPipeline` Skeleton

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | 0 — Foundation |
| Depends on | T04 |
| Unblocks | T06, T07, T08 (stages), T12 (devhost), T13 (APO) |
| Owns | `crates/tine-dsp/src/{lib.rs, pipeline.rs, meter.rs, rt.rs, primitives/**}`, `crates/tine-dsp/src/stages/mod.rs` (stub stages), `crates/tine-dsp/tests/common/**`, `crates/tine-dsp/benches/**` |
| Size | M |

## Goal
Everything the stage tasks and the host lanes need, so they can work in parallel:
- the allocation-free building blocks
- the measurement harness
- a **complete, stable `DspPipeline` public API** with pass-through stages, bypass cross-fade, metering, NaN guard, and a working benchmark

APO (T13) and devhost (T12) integrate against this skeleton immediately. Stage tasks replace the stubs without changing the API.

## Read first
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md) · [09-engineering-standards.md](../project/09-engineering-standards.md) §3, §7, §8 · D-006, D-010

## Contract (provides)

```rust
pub struct DspPipeline { /* concrete stage structs, no dyn */ }
impl DspPipeline {
    pub fn new(sample_rate: f32, max_block: usize) -> Self;          // all allocation happens here
    pub fn process(&mut self, buf: &mut [f32], params: &DspParams, meters: &mut impl MeterSink, tap: &mut impl TapSink);
    pub fn reset(&mut self);                                         // clears state + pass-through latch
    pub fn latency_samples(&self) -> u32;                            // constant for the pipeline's lifetime
    pub fn is_passthrough_latched(&self) -> bool;
}
pub trait MeterSink { fn publish(&mut self, f: &AudioTelemetryFrame); }   // called ~60 Hz (every fs/60 samples)
pub trait TapSink   { fn push(&mut self, post: &[f32]); }                // post-limiter mono
pub struct NoMeters; pub struct NoTap;                                    // zero-cost sinks
// Per-stage internal contract (crate-private) the stage tasks implement:
pub(crate) trait Stage { fn prepare(&mut self, fs: f32, max_block: usize); fn reset(&mut self);
                         fn process(&mut self, buf: &mut [f32], p: &DspParams, m: &mut StageMeters); fn latency(&self) -> u32; }
```

`process` accepts any block length (it chunks internally to `max_block`). Static dispatch: `Stage` is used through
generics/concrete fields only.

## Steps
### Primitives (`primitives/`)
- [ ] `db.rs`: `db_to_lin`, `lin_to_db` (floor −120 dB); exact versions. Add a fast approximation only if the bench proves a win, with an error bound test (< 0.01 dB).
- [ ] `smoother.rs`: one-pole, τ ms → coefficient from fs (`f64` design), `set_target`, `next`, `skip(n)` (closed-form jump for settled checks), `is_settled`, `snap`.
- [ ] `biquad.rs` (**own implementation, D-010**): RBJ Audio-EQ-Cookbook designs for all `FilterType`s, computed in `f64`, run as TDF-II in `f32`; `Coeffs`, `Biquad::process_block(&mut [f32])`, `retune_if_changed(type, f, q, gain, eps)`; Linkwitz–Riley 4th order helper (two cascaded Butterworth) and analytic `magnitude_db(f)` for any coefficient set.
- [ ] `envelope.rs`: peak (attack/release), RMS (one-pole on x²), with block-processing loops kept vectorizable.
- [ ] `delay.rs`: fixed power-of-two ring `DelayLine`; `sliding_max.rs`: monotone deque over a fixed buffer (O(1) amortized).
- [ ] `fifo.rs`: fixed-frame FIFO (for 480-sample RNNoise frames).
- [ ] `rt.rs`: `DenormalGuard` (MXCSR FTZ|DAZ set/restore via `core::arch::x86_64`; no-op elsewhere), `debug_assert_rt!()`.

### Pipeline skeleton
- [ ] `stages/mod.rs`: six pass-through stub structs (`InputStage`, `CleanupStage`, `EqStage`, `DeesserStage`, `CompressorStage`, `LimiterStage`) implementing `Stage`. Each stage file is created as a stub so T06–T08 own disjoint files: `stages/{input,cleanup}.rs` (T06), `stages/{eq,deesser}.rs` (T07), `stages/{compressor,limiter}.rs` (T08).
- [ ] `pipeline.rs`:
  - global bypass as a 10 ms equal-gain cross-fade against a dry path delayed by `latency_samples()`
  - per-stage enable flags read from `DspParams`
  - NaN/Inf guard at the end (reset + latch + output dry)
- [ ] `meter.rs` (Stage 7): input/output peak + RMS, collects stage GR values, publishes an `AudioTelemetryFrame` every `round(fs/60)` samples with a sequence counter.

### Harness & bench
- [ ] `tests/common/`: generators (sine, log sweep, white/pink noise, impulse, square, bursts), measurements (RMS, peak, crest, THD, FFT magnitude response via an in-harness DFT or `realfft` dev-dep), WAV/FLAC I/O (`hound`), `assert_no_alloc` wrapper macro `rt!{ ... }`, and `assert_close_db!`.
- [ ] `benches/pipeline.rs` (criterion, `perf` profile): 480-sample frame at 48 kHz and 44.1 kHz, all stages enabled; prints µs/frame. Add a CI job `bench-budget` (nightly) that fails when the median > 150 µs.

## Acceptance criteria
- Smoother: 63 % of a step reached in τ ± 2 %.
- Biquad: each `FilterType` matches the analytic cookbook magnitude within 0.05 dB at 20 log-spaced frequencies for 3 sample rates.
- `SlidingMax` equals a naive reference on 10⁵ random sequences (proptest).
- Skeleton: output equals input delayed by `latency_samples()` (0 for now) bit-exactly; bypass toggles produce no step > −60 dBFS on a sine; NaN injection latches pass-through.
- Zero allocations in `process` over 10 000 calls with random block sizes 1..=max_block.
- Bench runs in CI and records the baseline in `## Result`.

## Verification
```bash
cargo nextest run -p tine-dsp
cargo bench -p tine-dsp --profile perf --bench pipeline
cargo build -p tine-dsp --no-default-features
```

## Out of scope
Real stage algorithms (T06–T08), presets/macros (T10).

## Pitfalls
- Keep `tine-dsp` `no_std` + `alloc` (`Box<[f32]>` only in `new`). `std` feature only for tests/examples.
- Avoid `f32::powf`/`exp` per sample; precompute.
