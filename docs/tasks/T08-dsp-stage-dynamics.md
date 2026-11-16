# T08 — Stage 5–6: Compressor, Lookahead Limiter

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: T08-worker) |
| Lane | A — DSP (parallel with T06, T07) |
| Depends on | T05 |
| Unblocks | T09, T11 (calibrator uses the static curve) |
| Owns | `crates/tine-dsp/src/stages/compressor.rs`, `crates/tine-dsp/src/stages/limiter.rs`, `crates/tine-dsp/tests/stage_dynamics.rs` |
| Size | M |

## Goal
Replace the `CompressorStage` and `LimiterStage` stubs. The limiter's guarantee (no sample above the ceiling) is the last line of defence for everyone's ears.

## Read first
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md) §5–6 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3

## Contract (provides)
```rust
impl CompressorStage { pub fn static_curve_db(p: &DspParams, input_db: f32) -> f32; pub fn auto_makeup_db(p: &DspParams) -> f32; }
```

## Steps
- [ ] Compressor:
  - detector in the log domain
  - level = max(RMS (10 ms), peak − 6 dB) for fast transient catch
  - quadratic soft knee
  - attack/release ballistics in the gain domain
  - program-dependent release: release shortens by up to 50 % when the over-threshold duration is < 50 ms
  - makeup = manual, or auto (`-(T - T/R)·0.5`, smoothed)
  - `comp_gain_reduction_db` → `StageMeters`
- [ ] Limiter:
  - lookahead = round(1.5 ms · fs); buffers sized for 192 kHz in `prepare`
  - `SlidingMax` of |x| over the lookahead window
  - target gain = min(1, ceiling / max)
  - gain ramps down linearly across the lookahead (reaches target before the peak exits the delay), releases exponentially over 50 ms
  - optional 4× oversampled true-peak detector (polyphase FIR, coefficients precomputed) when `limiter_true_peak` is on; its latency is included **always** so latency stays constant
  - final safety clamp to the ceiling (should never engage; count engagements in debug builds)

## Acceptance criteria
- Static curve: threshold −20, ratio 4, knee 0 → input −10 dBFS gives −17.5 ±0.1 dBFS (pre-makeup). Knee 6 dB is continuous, monotone, and its derivative is continuous.
- Attack/release time constants within ±10 % of the setting (step tests).
- Limiter: strict property test over 10⁶ random signals (full-scale squares, impulses, noise, +24 dB overdriven speech): **no output sample > ceiling** (sample-peak mode). True-peak mode: 4× oversampled output peak ≤ ceiling + 0.1 dB.
- Signals below the ceiling pass bit-exact apart from the delay (sample-peak mode).
- The safety clamp never engages in the property test.
- Zero allocations; per-stage bench recorded (target < 8 µs / 480 samples without true-peak).

## Verification
```bash
cargo nextest run -p tine-dsp stage_dynamics
cargo bench -p tine-dsp --profile perf --bench pipeline
```

## Out of scope
Auto-Tune makeup iteration (T11).
