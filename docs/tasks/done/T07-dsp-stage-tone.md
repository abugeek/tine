# T07 — Stage 3–4: 5-Band Parametric EQ, Split-Band De-Esser

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | A — DSP (parallel with T06, T08) |
| Depends on | T05 |
| Unblocks | T09, T19 (curve math for the Golden Line) |
| Owns | `crates/tine-dsp/src/stages/eq.rs`, `crates/tine-dsp/src/stages/deesser.rs`, `crates/tine-dsp/src/curve.rs`, `crates/tine-dsp/tests/stage_tone.rs` |
| Size | M |

## Goal
Replace the `EqStage` and `DeesserStage` stubs, and provide the pure response-curve function the UI draws.

## Read first
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md) §3–4 · [09-engineering-standards.md](../project/09-engineering-standards.md) §3 · `primitives::biquad` from T05

## Contract (provides)
```rust
pub mod curve {
    /// Combined magnitude of stage 1 HPF + 5 EQ bands + high-shelf etc. for the current params, in dB.
    pub fn response_db(params: &DspParams, fs: f32, freq_hz: f32) -> f32;
    /// Fills `out` with response at `freqs` (log-spaced), no allocation.
    pub fn response_curve(params: &DspParams, fs: f32, freqs: &[f32], out: &mut [f32]);
}
```

## Steps
- [x] EQ: 5 cascaded biquads.
  - Per-band type/freq/gain/Q smoothed; retune ≤ every 32 samples.
  - A band whose |gain| has *settled* below 0.01 dB (gain-type filters only) is skipped. Keep running it while gain is still smoothing toward zero, so there are no clicks.
  - Changing a band's type cross-fades old → new filter over 10 ms.
- [x] De-esser:
  - LR4 crossover at `deesser_freq_hz` (low = LP·LP, high = HP·HP, sum is all-pass and magnitude-flat)
  - sidechain 2nd-order band-pass at the same freq, fast RMS (attack 0.5 ms, release 40 ms)
  - GR = min(amount, (level − thresh)·(1 − 1/4)) on the high band only
  - `deesser_reduction_db` → `StageMeters`
  - with enable off, cross-fade to bypass
- [x] `curve.rs`: analytic magnitude from the same coefficient design (`f64`), so the UI line matches what's heard.

## Acceptance criteria
- Each band type matches analytic response within 0.1 dB across 20 Hz–20 kHz at 44.1/48/96 kHz.
- `curve::response_db` matches the measured cascade (sweep + FFT) within 0.2 dB.
- Sweeping a band's freq 100 Hz → 5 kHz in 1 s on a 1 kHz sine: no clicks (max |Δsample| stays within 1.5× the clean reference).
- De-esser at idle: crossover sum flat within ±0.1 dB. A synthetic "S" (band-limited noise 6–9 kHz, −10 dBFS) over a 200 Hz tone: 6–9 kHz energy reduced ≥ 6 dB, 200 Hz change ≤ 0.3 dB.
- Zero allocations; per-stage bench recorded (target < 10 µs / 480 samples for both together).

## Verification
```bash
cargo nextest run -p tine-dsp stage_tone
cargo bench -p tine-dsp --profile perf --bench pipeline
```

## Out of scope
Macro mapping to EQ gains (T10).

## Result
- **Performance (`--profile perf`, 480 samples @ 48 kHz):**
  - Stage 3 (5 EQ bands active): **4.700 µs**
  - Stage 4 (De-esser active GR): **4.638 µs**
  - Combined Stages 3 + 4: **9.338 µs** (target < 10 µs)
  - Full `pipeline_480_48khz` bench: **5.8881 µs** (44.1 kHz: **5.7775 µs**)
- **Verification:**
  - `cargo test -p tine-dsp --test stage_tone`: 9 passed, 0 failed (1 ignored perf test passes under `--profile perf`).
  - `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `cargo deny check`, `cargo build -p tine-dsp --no-default-features`: all passed.
- **Decisions & optimizations:**
  - In `DeesserStage`, avoided per-sample `log10`/`pow10` and `sqrt_f32` software divisions by tracking `mean_sq` directly and computing the 4:1 gain `(thresh_lin / env)^(3/4)` as `(mean_sq / thresh_sq)^(-3/8)` via division-free `fast_inv_sqrt` (~0.17% / ~0.015 dB accuracy), reducing De-esser cost from 14.238 µs to 4.638 µs.
  - Added settled fast paths in both `EqStage` and `DeesserStage` (hoisting filter state into local variables across the block) while retaining 32-sample chunked retuning and 10 ms cross-fades when parameters or topology change.
- **Surprises:**
  - `sqrt_f32` in `primitives::math` uses 3 software `f32` divisions, which dominated the initial de-esser profile before switching to division-free `fast_inv_sqrt`.
- **Follow-ups:**
  - None for T07; T09 will benchmark the full 7-stage pipeline once T06 and T08 land.
