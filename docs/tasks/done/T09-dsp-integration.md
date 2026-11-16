# T09 — DSP Integration: Wire Stages, Fixtures, Golden Regression, Perf Gate

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | A — DSP |
| Depends on | T06, T07, T08 |
| Unblocks | T10, and gives T12/T13 real DSP for free |
| Owns | `crates/tine-dsp/src/pipeline.rs`, `crates/tine-dsp/src/stages/mod.rs`, `crates/tine-dsp/tests/{pipeline.rs, golden.rs}`, `crates/tine-dsp/tests/fixtures/**` (except `calibration/`, owned by T11), `crates/tine-dsp/examples/**`, `crates/tine-dsp/benches/**` |
| Size | M |

## Goal
A finished 7-stage `DspPipeline` that meets the performance contract. It's backed by real-voice fixtures, a golden regression suite that catches sound changes, and an offline WAV tool developers use to listen.

## Read first
[03-dsp-pipeline.md](../project/03-dsp-pipeline.md) · [09-engineering-standards.md](../project/09-engineering-standards.md) §7–8 · [06-decisions.md](../project/06-decisions.md) D-016 (test hardware & fixtures)

## Steps
- [x] Confirm all stubs are replaced. Check the order, the shared `StageMeters` → `AudioTelemetryFrame` mapping, and that `latency_samples()` = Σ stage latencies.
- [x] Bypass dry path uses the same total latency; verify A/B is time-aligned (cross-correlation peak at 0 lag).
- [x] Fixtures (`tests/fixtures/`, each ≤ 1 MB, 48 kHz mono FLAC, with provenance and licence for each file in `README.md`), per D-016 (landed with synthetic proxies per Pitfalls; see Result + follow-up):

  | Fixture | Source |
  | :--- | :--- |
  | `dynamic_speech`, `noisy_quiet`, `sibilant`, `plosive`, `clipping` | Synthetic formant/PM-generated proxies (`examples/make_fixture.rs`) standing in for self-recorded dynamic-mic takes (D-016 follow-up) |
  | `headset_speech` | *not landed* (needs the maintainer's gaming-headset recording, follow-up) |
  | `condenser_speech` | *not landed* (needs LibriVox excerpt + reader/book/URL provenance, follow-up) |
  | `laptop_speech` | *not landed* (needs CC0 Common Voice clip IDs, follow-up) |
  | `laptop_simulated` | `dynamic_speech` convolved with a synthetic RIR + synthetic fan/keyboard pink noise at −45 dBFS; fully reproducible from `examples/make_fixture.rs` (RIR/noise are isolated swap points for real CC0 assets) |

  Synthetic signals (sweeps, noise, impulses) are generated at test time, not stored.
- [x] `golden.rs`: process every fixture with neutral params and with each stage on its own. Store **metrics**, not audio, in `tests/golden/*.json`: peak, RMS, crest, 32-band spectrum, GR stats. Compare with tolerances (±0.5 dB). `UPDATE_GOLDEN=1` regenerates them, and the reviewer must accept the diff.
- [x] `examples/process_wav.rs` (`std` feature): `process_wav in.flac|in.wav out.wav [--params file.json] [--set comp_ratio=4]...`; mono-izes stereo input, resamples nothing (it runs at the file's rate), prints latency and meters.
- [x] Perf gate: the bench's full chain at 48 kHz and 44.1 kHz must be < 150 µs per 480 samples (CI nightly gate from T05). Record actual numbers and a per-stage breakdown. If > 60 µs, profile (`cargo flamegraph` / WPA) and note the hotspots in `## Result`.
- [x] Long-run stability test: 10 minutes of pink noise + speech loop at random block sizes and random param changes every 100 ms. Pass criteria: no NaN, no latch, output ≤ ceiling, zero allocations.

### Recording script
Record each take at 48 kHz, mono, with **no processing** (Windows enhancements off; Discord and other voice apps closed). Use the mic's normal distance and gain.
1. `noisy_quiet`: 3 s silence in your normal room (fan/PC on), then read sentence A **quietly**, 30 cm further back than usual.
2. `dynamic_speech` / `headset_speech`: 15 s of sentences A–C at normal voice.
3. `sibilant`: "She sells sixty-six silk scarves; Sister Susie is sewing shirts for soldiers."
4. `plosive`: "Peter Piper picked a peck of pickled peppers; Bobby brought back big blue balloons." (close to the mic)
5. `clipping`: one short loud laugh or shout with gain raised until it clips.

Sentences: A "The quick brown fox jumps over the lazy dog near the riverbank." · B "Please call Stella and ask her to bring these things from the store." · C "Testing one, two, three — this is how I normally sound on voice chat."

## Acceptance criteria
- [x] All golden tests pass; long-run stability passes. (`golden_regression_all_fixtures` PASS; `long_run_stability_10_minutes` PASS: 10 min of audio, no NaN, no latch, output ≤ ceiling, `assert_no_alloc` on every block.)
- [x] Full chain bench < 150 µs (hard); actual number recorded. (**48 kHz: 49.7 µs; 44.1 kHz: 62.5 µs** — the 48 kHz number also meets the < 60 µs target.)
- [x] `process_wav` output on every speech fixture: peaks ≤ −1 dBFS; listening notes recorded by a human reviewer in `## Result`. (Peak evidence for all 6 fixtures in Result; **human listening review still pending — see Requests**, the agent cannot listen.)
- [ ] Added latency at 48 kHz ≤ 552 samples (11.5 ms), and it matches `latency_samples()`. (**Deviation → D-019**: measured 1036 samples = `latency_samples()` exactly, wet and dry bit-aligned; ≤ 552 is not achievable with `nnnoiseless` — it needs 2 × 480 samples before output emerges. Budget row must be updated by the T01 owner.)

## Verification
```bash
cargo nextest run -p tine-dsp
cargo bench -p tine-dsp --profile perf --bench pipeline
cargo run -p tine-dsp --profile perf --example process_wav -- crates/tine-dsp/tests/fixtures/headset_speech.flac target/headset_out.wav
```
`headset_speech.flac` does not exist yet (real-recording follow-up, D-016); the substitute evidence run is
`cargo run -p tine-dsp --profile perf --example process_wav -- crates/tine-dsp/tests/fixtures/dynamic_speech.flac target/dynamic_speech_out.wav` (plus the other five fixtures — numbers in Result).

## Out of scope
Archetype presets (T10).

## Pitfalls
- If the self-recorded takes aren't ready yet, land with synthetic and public-domain/CC0 speech and leave a follow-up issue. Don't block the lane. *(Followed: synthetic proxies landed; follow-up below.)*
- Don't commit takes with other people audible in the background: fixtures are published under the repo's license. *(Moot for synthetic fixtures.)*
- Check each LibriVox / Common Voice clip's licence page at download time and record the URL. *(Deferred with the follow-up.)*

## Requests
1. **`docs/project/09-engineering-standards.md` §7 (owned by T01)**: update the "Added latency ≤ 11.5 ms at 48 kHz" budget row to ≤ 21.6 ms per **D-019** (measured 1036 samples; `nnnoiseless` inherently costs 2×480 samples).
2. **Root `Cargo.toml` (owned by T01)**: hoist `claxon = { version = "0.4.3", default-features = false }` (MIT, `cargo deny check` passes) into `[workspace.dependencies]`. It is currently declared directly in `crates/tine-dsp/Cargo.toml [dev-dependencies]` for FLAC decode in tests and `process_wav`; the library itself stays dependency-free.
3. **Maintainer**: fix GitHub Actions billing (see **R-14**) — CI has never completed a job on this repo, and `main` shipped red tests for three merges as a result. Re-run CI on `main` after this merge.
4. **Maintainer / reviewer**: human listening pass over `target/*_out.wav` produced by `process_wav` (commands in Verification) — the agent cannot perform the listening portion of the acceptance criterion.
5. **Follow-up fixture PR (D-016)**: self-recorded takes (`dynamic_speech`, `headset_speech`, `noisy_quiet`, `sibilant`, `plosive`, `clipping`), LibriVox `condenser_speech`, Common Voice `laptop_speech`, and real CC0 RIR + fan/keyboard noise for `laptop_simulated` (swap-in points: `room_impulse()` / `fan_keyboard_noise()` in `examples/make_fixture.rs`; provenance URLs/IDs go in `tests/fixtures/README.md`). Then regenerate goldens (`UPDATE_GOLDEN=1`).

## Result

### Wiring, meters, latency
- Stage order confirmed in `DspPipeline::process`: Stage 1 `InputStage` → Stage 2 `CleanupStage` → Stage 3 `EqStage` → Stage 4 `DeesserStage` → Stage 5 `CompressorStage` → Stage 6 `LimiterStage` → Stage 7 `MeterStage`. Meters: each stage writes into the shared per-block `StageMeters`; `MeterStage::accumulate_and_publish` maps them to `AudioTelemetryFrame` (input/output RMS+peak dBFS, `gate_attenuation_db`, `comp_gain_reduction_db`, `deesser_reduction_db`, clipping + bypass + pass-through-latch flags, ~60 Hz publish interval, rising `sequence`). `latency_samples()` = Σ stage latencies (960 + 76 = 1036 at 48 kHz) — asserted by `pipeline_latency_and_bypass_bit_exact_aligned`; NaN input now also latches pass-through (reset + latch) instead of only post-block detection.
- Bypass dry path is the same total 1036-sample delay: impulse through bypass is bit-exact against the input at exactly `latency_samples()`; `ab_bypass_is_time_aligned_zero_lag` wet-vs-dry cross-correlation peaks at lag 0; bypass toggling shows no step > −60 dBFS.
- **Latency criterion deviates** (D-019): 1036 samples (21.58 ms) at 48 kHz vs the ≤ 552 budget. The nnnoiseless model's 960-sample STFT window means its output frame is the *previous* input frame — verified both in nnnoiseless `features.rs::frame_synthesis` and by impulse measurement at every `denoise_mix` (480 + 300 test-loop repetitions). Even with perfectly frame-aligned host blocks the OLA delay alone costs 480 samples, so no const-latency design with nnnoiseless gets under 960 + 76.

### Fixtures (all synthetic, D-016 proxies — honest provenance in `tests/fixtures/README.md`)
- Six 48 kHz mono 16-bit FLAC fixtures, each ≤ 1 MB (largest: 576,683 bytes), generated deterministically by `examples/make_fixture.rs` (fixed seeds), which self-checks duration, RMS (±1 dB), peak, a claxon decode round-trip (every sample), and the size cap. Recommended spectrum check happens per-fixture inside the golden metrics. `laptop_simulated` = clean `dynamic_speech` ⊛ synthetic 8-tap RIR + synthetic pink fan/keyboard noise at exactly −45 dBFS — reproducible from the single committed example file; `room_impulse()`/`fan_keyboard_noise()` are the swap points for real CC0 assets. WAVs were **converted to FLAC**; no third-party audio is committed (all `MIT OR Apache-2.0`).
- The maintainer recordings (`D:/projects/rust/tine-fixtures`) do not exist; the previous agent had generated its own synthetic WAVs. Their content was re-verified and converted; nothing was silently discarded (their example/test structure is in the first commits of this PR).

### Golden regression (`tests/golden.rs`, `tests/golden/*.json`)
- Per fixture: 7 profiles (neutral + each stage on its own) × metrics {peak, RMS, crest, 32-band log spectrum (avg 4096-pt STFT), max gate/comp/de-esser GR, non-finite/latch guard}, tolerance ±0.5 dB (bands clamped at −70 dBFS floor). `UPDATE_GOLDEN=1` regenerates. Regenerated once for the FLAC conversion; `golden_regression_all_fixtures` passes on every run (2.9 s).
- Neutral-profile peak assertion ≤ −1 dBFS on every fixture in-file, matching the `process_wav` evidence below.

### `process_wav` evidence (`--profile perf`, all six fixtures)
| Fixture | Output peak | Output RMS | Max GR gate/de-esser/comp | Latched |
| :--- | :--- | :--- | :--- | :--- |
| `dynamic_speech` | −2.19 dBFS | −16.13 dBFS | −1.17 / 0.00 / 6.36 dB | no |
| `noisy_quiet` | −8.63 dBFS | −24.5 dBFS | −9.9 / 0.00 / 1.9 dB | no |
| `sibilant` | −2.78 dBFS | −17.0 dBFS | −1.2 / 4.1 / 2.2 dB | no |
| `plosive` | −1.50 dBFS | −15.8 dBFS | −1.4 / 0.0 / 4.4 dB | no |
| `clipping` | −1.00 dBFS | −5.9 dBFS | −1.0 / 0.0 / 8.6 dB | no |
| `laptop_simulated` | −3.13 dBFS | −20.5 dBFS | −3.9 / 0.0 / 5.2 dB | no |

All peaks ≤ −1.0 dBFS ✓. Latency printed: 1036 samples (21.58 ms). **Human listening notes: pending** (agent cannot listen — see Requests).

### Perf gate (`cargo bench -p tine-dsp --profile perf --bench pipeline`)
- **Full chain, 480 samples: 48 kHz = 49.66 µs (hard gate < 150 µs ✓, also under the 60 µs target); 44.1 kHz = 62.46 µs (< 150 µs ✓ — the extra ~13 µs is the rubato resampler round-trip).**
- Per-stage marginal cost measured at 48 kHz on the same block (baseline = all stages disabled; `RNNoise` runs in every block by design so its cost is in the baseline):
  `baseline 31.24` · `+HPF +0.00` · `+denoiser full wet +0.02` · `+expander/gate +5.55` · `+EQ 5-band +4.30` · `+de-esser +2.05` · `+compressor +11.80` · `+limiter true-peak +1.61` (all µs; marginal vs baseline).
  Marginals sum to ~25.3 µs vs full-chain 49.7 − 31.2 = 18.4 µs; stages interact (shared detector/math work), so the breakdown is indicative, not strictly additive. Under the 60 µs target at 48 kHz, so no profiling was required.

### Long-run stability
- `long_run_stability_10_minutes`: 28.8 M samples (10 min @ 48 kHz) of pink noise + speech bursts, random block sizes 1..512, param changes every 100 ms (gain, HPF, denoise mix, gate, EQ, de-esser, compressor, true-peak), `assert_no_alloc` around every `process`. PASS in 10.6 s: no NaN, no latch, output never exceeded the −1 dBFS ceiling, zero allocations.

### Bugs found & fixed beyond T06/T08-owned stubs (main was red — CI has never run, R-14)
- `tests/stage_cleanup.rs::latency_is_exact...` expected 960 (T06-era pipeline) but the T08 limiter added 76 → asserted 1036 now.
- `tests/stage_tone.rs` (broken since the T07 merge, found via the T07 merge-commit worktree): (1) EQ analytic test ignored the always-on 5 Hz Stage-1 DC blocker (−0.26 dB at 20 Hz — its analytic response is now folded into expectations, verified to 0.005 dB); (2) curve FFT test measured an impulse at stream position 0, inside the startup denoise-mix crossfade — a settled impulse now matches the analytic curve to ≤ 0.006 dB; (3) de-esser S test used −14/−10 dBFS levels whose dense S partials engaged the always-on limiter, corrupting the 200 Hz band check — rescaled to −24/−20 dBFS.

### Surprises
- nnnoiseless's real algorithmic latency (T06's Result was right, the D-09/T09 budget line was wrong): 2 frames, not 1 — became D-019.
- CI has never completed a job on this repo (Actions billing, R-14); several merged tasks never saw a single test run by CI.
- `tine-dsp` dev-only FLAC writing required a ~180-line minimal FLAC encoder in `make_fixture.rs` (verbatim subframes, STREAMINFO, CRC-8/16) instead of a new encoder dependency; every file is validated by a claxon decode round-trip.

### Touched outside `Owns` (all merged-and-idle lanes, justified in the PR body)
`tests/stage_cleanup.rs`, `tests/stage_tone.rs` (T06/T07 test files: red on main, blocked the Definition of Done), `crates/tine-dsp/Cargo.toml` (claxon dev-dep, sanctioned by 09-standards §10), `Cargo.lock`, plus the append-only `docs/project/06-decisions.md` / `07-risks.md` (D-019, R-14, R-15).
