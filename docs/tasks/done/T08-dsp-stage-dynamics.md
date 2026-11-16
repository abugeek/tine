# T08 — Stage 5–6: Compressor, Lookahead Limiter

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] Compressor:
  - detector in the log domain
  - level = max(RMS (10 ms), peak − 6 dB) for fast transient catch
  - quadratic soft knee
  - attack/release ballistics in the gain domain
  - program-dependent release: release shortens by up to 50 % when the over-threshold duration is < 50 ms
  - makeup = manual, or auto (`-(T - T/R)·0.5`, smoothed)
  - `comp_gain_reduction_db` → `StageMeters`
- [x] Limiter:
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

## Result
- **Measured performance (`cargo bench -p tine-dsp --profile perf --bench pipeline`, 480 samples, before T06 rebase)**:
  - `pipeline_480_48khz`: **16.538 µs** (95% CI `[16.055 µs, 17.037 µs]`) for the full pipeline including `CompressorStage` + `LimiterStage` (with `fast_lin_to_db`/`fast_db_to_lin` and `[f32; 16]` FIR ring; down from 58.767 µs with `f64` Taylor log/exp per sample).
  - `pipeline_480_44_1khz`: **15.667 µs** (95% CI `[15.385 µs, 15.983 µs]`).
- **Tests**:
  - `cargo test -p tine-dsp --test stage_dynamics`: 7 passed, 0 failed (static curve, soft knee $C^1$/monotone, 1M-sample brickwall + `assert_no_alloc`, 4x true-peak intersample bound, proptest arbitrary ceilings, zero-alloc under param changes & constant latency).
  - `cargo test -p tine-dsp --lib`: 19 passed, 0 failed (including `fast_db_approximations_within_0_01_db`, `attack_and_release_time_constants_within_10_percent`, `program_dependent_release_shortens_on_short_transient`, `below_ceiling_is_bit_exact_apart_from_delay`, `safety_clamp_never_engages_over_1m_samples`).
  - `cargo fmt --all --check`, `cargo build -p tine-dsp --no-default-features`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo deny check`: all passed.
- **Decisions & implementation notes**:
  - `CompressorStage` and `LimiterStage` are `pub struct` in `stages::{compressor, limiter}` so `tine-dsp` integration tests and T11 (`calibrator`) can call `CompressorStage::static_curve_db` and `CompressorStage::auto_makeup_db`.
  - Used validated fast `f32` `fast_lin_to_db` / `fast_db_to_lin` approximations (< 0.01 dB error over `[-119, +24] dB`) in `CompressorStage::process` per `09-engineering-standards.md` §3.5.
  - `LimiterStage` uses a 9-tap Kaiser-windowed sinc ($\beta = 6.5$) 4x polyphase FIR interpolator (`FIR_HALF = 4` samples always included in the audio delay line; total latency = 76 samples at 48 kHz = 1.583 ms) and biases `target_gain` down by 1 ULP when `max_peak > ceiling` so the safety clamp never engages under `f32` rounding.
- **Follow-ups**:
  - `crates/tine-dsp/tests/pipeline.rs` (owned by T05/T09) still has T05 skeleton assertions (`latency_samples() == 0`, 6-sample NaN test) that T09 will update when integrating all stages.
