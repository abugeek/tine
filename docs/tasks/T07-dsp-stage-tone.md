# T07 — Stage 3–4: 5-Band Parametric EQ, Split-Band De-Esser

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: T07-worker) |
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
- [ ] EQ: 5 cascaded biquads.
  - Per-band type/freq/gain/Q smoothed; retune ≤ every 32 samples.
  - A band whose |gain| has *settled* below 0.01 dB (gain-type filters only) is skipped. Keep running it while gain is still smoothing toward zero, so there are no clicks.
  - Changing a band's type cross-fades old → new filter over 10 ms.
- [ ] De-esser:
  - LR4 crossover at `deesser_freq_hz` (low = LP·LP, high = HP·HP, sum is all-pass and magnitude-flat)
  - sidechain 2nd-order band-pass at the same freq, fast RMS (attack 0.5 ms, release 40 ms)
  - GR = min(amount, (level − thresh)·(1 − 1/4)) on the high band only
  - `deesser_reduction_db` → `StageMeters`
  - with enable off, cross-fade to bypass
- [ ] `curve.rs`: analytic magnitude from the same coefficient design (`f64`), so the UI line matches what's heard.

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
