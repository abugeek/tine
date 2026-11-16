# T06 — Stage 1–2: Input Gain / DC / HPF, RNNoise Mix + Expander

| Field | Value |
| :--- | :--- |
| Status | todo |
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
- [ ] **Stage 1** (`input.rs`):
  - gain (smoothed dB → linear)
  - DC blocker (`R = exp(-2π·5 Hz/fs)`)
  - Butterworth HPF (Q 0.7071) via `primitives::biquad`, enable cross-faded over 10 ms
  - one loop per sub-block of 32 samples, retuning when needed
- [ ] **Stage 2a — denoise** (`cleanup.rs`):
  - `Box<DenoiseState>` created in `prepare`
  - input and output FIFOs of 480 samples, scaled by ×32768 / ÷32768
  - dry path delayed by exactly the same latency
  - `out = dry·(1−mix) + wet·mix`, with `mix` smoothed
  - **Always run RNNoise** even at mix 0, so latency and CPU stay constant. Skipping the network at mix = 0 for longer than 1 s (with warm-up on re-enable) is allowed only if latency stays identical; document the choice.
- [ ] **Stage 2a at fs ≠ 48 kHz** (`resample.rs`):
  - resampler pair fs → 48 kHz → fs from `rubato` (or a hand-rolled polyphase if rubato isn't allocation-free per call; prove it with `assert_no_alloc`)
  - report the added delay so `latency()` stays constant
- [ ] **Stage 2b — expander**:
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
