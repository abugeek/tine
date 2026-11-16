# T06 — Stage 1–2: Input Gain / DC / HPF, RNNoise Mix + Expander

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | A — DSP (parallel with T07, T08) |
| Depends on | T05 |
| Unblocks | T09 |
| Owns | `crates/tine-dsp/src/stages/input.rs`, `crates/tine-dsp/src/stages/cleanup.rs`, `crates/tine-dsp/src/resample.rs`, `crates/tine-dsp/tests/stage_cleanup.rs` |
| Size | M |

## Goal
Replace the `InputStage` and `CleanupStage` stubs with the real Stage 1 and Stage 2 from
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md#stage-details), including the RNNoise resampling path for non-48 kHz rates (D-006).

## Read first
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md) §1–2 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3 · `nnnoiseless` 0.5.2 docs (`DenoiseState`, `FRAME_SIZE`) · `rubato` 5.x docs (fixed-size, `process_into_buffer`)

## Steps
- [x] **Stage 1** (`input.rs`):
  - gain (smoothed dB → linear)
  - DC blocker (`R = exp(-2π·5 Hz/fs)`)
  - Butterworth HPF (Q 0.7071) via `primitives::biquad`, enable cross-faded over 10 ms
  - one loop per sub-block of 32 samples, retuning when needed
- [x] **Stage 2a — denoise** (`cleanup.rs`):
  - `Box<DenoiseState>` created in `prepare`
  - input and output FIFOs of 480 samples, scaled by ×32768 / ÷32768
  - dry path delayed by exactly the same latency
  - `out = dry·(1−mix) + wet·mix`, with `mix` smoothed
  - **Always run RNNoise** even at mix 0, so latency and CPU stay constant. Skipping the network at mix = 0 for longer than 1 s (with warm-up on re-enable) is allowed only if latency stays identical; document the choice.
- [x] **Stage 2a at fs ≠ 48 kHz** (`resample.rs`):
  - resampler pair fs → 48 kHz → fs from `rubato` (or a hand-rolled polyphase if rubato isn't allocation-free per call; prove it with `assert_no_alloc`)
  - report the added delay so `latency()` stays constant
- [x] **Stage 2b — expander**:
  - peak detector (attack param, release param) in dB
  - soft knee 6 dB
  - hysteresis: opens at threshold, closes at threshold − hysteresis
  - gain computer: `(ratio−1)·(level−threshold)` below threshold, floor −80 dB
  - gain smoothed by attack/release
  - writes `gate_attenuation_db` to `StageMeters`

## Acceptance criteria
- HPF 100 Hz: 50 Hz attenuated ≥ 11 dB (2nd-order Butterworth theory ≈ 12.3 dB), 25 Hz ≥ 23 dB, 1 kHz ±0.1 dB.
- DC 0.1 offset → < 0.001 within 200 ms.
- Denoise: speech fixture + white noise at −50 dBFS, mix 1.0 → noise in speech gaps reduced ≥ 15 dB; speech-band RMS change ≤ 2 dB.
- Latency: impulse test gives exactly `latency()` samples at 48 kHz (480) and the documented value at 44.1 kHz; constant across mix and enable changes.
- Expander: −60 dBFS noise with threshold −50, ratio 4 → ≥ 20 dB attenuation; a signal 5 dB above threshold changes ≤ 0.5 dB; a signal hovering ±1 dB around threshold toggles state < 3 times in 2 s.
- Zero allocations in `process`; per-stage bench cost recorded (target: stage 2 < 40 µs / 480 samples, which is dominated by RNNoise).

## Verification
```bash
cargo nextest run -p tine-dsp stage_cleanup
cargo bench -p tine-dsp --profile perf --bench pipeline
```

## Out of scope
Other stages; macro mapping.

## Pitfalls
- `nnnoiseless` processes in place on `&[f32; 480]`-sized slices: use the FIFOs to avoid any per-call allocation.
- The first frame of RNNoise output is warm-up. Keep the dry path aligned anyway.

## Requests
- **`crates/tine-dsp/tests/pipeline.rs` (owned by T05/T09)**: Now that `CleanupStage` is no longer a 0-latency stub, three T05 skeleton tests fail because they assume `latency_samples() == 0` and bit-exact identity (`skeleton_is_bit_exact_passthrough`, `bypass_toggle_has_no_step_above_minus_60_dbfs`, `nan_injection_latches_passthrough_and_reset_clears` — where the 6-sample NaN input is still inside the 960-sample delay line at the end of the 6-sample call). Update these in T09 when all stages are integrated.

## Result
- **Stage 1 (`input.rs`)**: Implemented smoothed input gain, 5 Hz DC blocker (`R = exp(-2π·5/fs)`), and cross-faded 2nd-order Butterworth HPF (`Q = 0.7071`) retuned per 32-sample sub-block.
  - HPF 100 Hz: 50 Hz ≤ −11 dB, 25 Hz ≤ −23 dB, 1 kHz within ±0.1 dB (verified in `stage_cleanup`).
  - DC 0.1 step decays to < 0.001 within 200 ms (verified in `stage_cleanup`).
- **Stage 2a (`cleanup.rs`, `resample.rs`)**:
  - `DenoiseState` is boxed and warmed up in `prepare()` (initializes `nnnoiseless` `OnceCell` outside RT). Always runs even at `denoise_mix == 0.0` so CPU and latency remain constant.
  - **Latency finding**: `nnnoiseless::DenoiseState::process_frame` uses a 960-sample 50% overlap-add STFT window, which adds 480 samples of OLA delay on top of the 480-sample FIFO framing delay for arbitrary block sizes. Total `CleanupStage::latency()` at 48 kHz is **960 samples** (20 ms), with `dry_delay` matched to 960 samples so dry and wet are phase-aligned across all `denoise_mix` and enable settings. (At 44.1 kHz, `RnnoiseResampler` measures exact round-trip latency in `prepare()`, ~920 samples, and aligns `dry_delay` to within ≤ 1 sample.)
  - Denoise on speech + −50 dBFS white noise at `mix = 1.0`: ≥ 15 dB noise reduction in gaps and ≤ 2 dB speech-band RMS change (verified in `stage_cleanup`).
  - `rubato::Async<f32>` (`SincInterpolationType::Linear`, `sinc_len = 32`, `BlackmanHarris2`, `oversampling_factor = 128`) with `SequentialSlice` adapters is proven 100% allocation-free per call at 44.1 kHz and 48 kHz via `assert_no_alloc`.
- **Stage 2b (`cleanup.rs`)**: Peak envelope detector (`PeakEnvelope`) in dB, 6 dB soft knee, hysteresis (`threshold` to `threshold - hysteresis`), `(ratio - 1) * (level - threshold)` clamped at −80 dB, attack/release-smoothed linear gain, and `gate_attenuation_db` telemetry.
  - −60 dBFS noise at threshold −50 / ratio 4 → ≥ 20 dB attenuation; +5 dB signal → ≤ 0.5 dB change; ±1 dB hover → < 3 toggles in 2 s (verified in `stage_cleanup`).
- **Performance (`cargo bench -p tine-dsp --profile perf --bench pipeline`)**:
  - `pipeline_480_48khz`: **38.794 µs** (CI [38.051 µs, 39.598 µs]) — under the 40 µs target.
  - `pipeline_480_44_1khz`: **52.702 µs** (CI [51.120 µs, 54.427 µs]) — includes `44.1k -> 48k -> 44.1k` sinc resampling + RNNoise.
- **Verification**:
  - `cargo test -p tine-dsp --test stage_cleanup`: 6/6 passed.
  - `cargo test -p tine-dsp --lib`: 14/14 passed.
  - `cargo fmt --all --check`: passed.
  - `cargo clippy --workspace --all-targets -- -D warnings`: passed.
  - `cargo deny check`: passed (`advisories ok, bans ok, licenses ok, sources ok`).
  - `cargo build -p tine-dsp --no-default-features`: passed.
