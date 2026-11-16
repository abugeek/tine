# T11 — `AutoCalibrator`

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | A — DSP |
| Depends on | T10 |
| Unblocks | T16 (Auto-Tune runner), T20 (wizard UI) |
| Owns | `crates/tine-dsp/src/calibrator.rs`, `crates/tine-dsp/tests/calibrator.rs`, `crates/tine-dsp/tests/fixtures/calibration/**` |
| Size | M |

## Goal
Layer B of [04-mic-adaptation.md](../project/04-mic-adaptation.md#layer-b--auto-tune-8-seconds): a pure, deterministic state machine. It turns 9 seconds of raw mic audio into a `Calibration` plus a human-readable explanation. Its loudness targets are proven by simulating the whole pipeline offline.

## Read first
[04-mic-adaptation.md](../project/04-mic-adaptation.md) Layer B · `MacroMapper` (T10) · `CompressorStage::static_curve_db` (T08)

## Contract (provides)
```rust
pub struct AutoCalibrator;  // preallocates 9 s mono buffer at fs in new()
pub enum CalPhase { Countdown, Noise, Voice, Analyzing, Done, Failed }
pub struct CalProgress { pub phase: CalPhase, pub phase_progress: f32, pub live_level_db: f32, pub voiced_ms: u32 }
pub enum CalFailure { TooQuiet, TooNoisy { snr_db: f32 }, Clipping { pct: f32 }, NotEnoughSpeech { voiced_ms: u32 }, GainOutOfRange { needed_db: f32 } }
impl AutoCalibrator {
    pub fn new(fs: f32) -> Self;
    pub fn push(&mut self, samples: &[f32]) -> CalProgress;   // allocation-free; called from a non-RT thread
    pub fn finish(&mut self, preset: &PresetDef, macros: MacroPositions, pins: PinSet, current: &DspParams) -> Result<CalibrationReport, CalFailure>;
}
pub struct CalibrationReport { pub calibration: Calibration, pub changes: [(ParamId, f32, f32); N], pub simulated_peak_db: f32, pub simulated_rms_db: f32 }
```

## Steps
- [x] Phases: Countdown 1 s (discarded) → Noise 3 s → Voice 5 s → Analyzing.
- [x] Noise: median of 100 ms window RMS (robust to a click), max peak.
- [x] Voice:
  - voiced windows are those > noise + 10 dB (20 ms windows)
  - integrated RMS over voiced windows
  - crest factor
  - spectral tilt from band energies < 350 Hz vs 1–6 kHz (biquad band filters from `primitives`)
- [x] Derivation:
  - gate = noise + 6
  - denoise = 0.85 if noise > −48 dBFS else 0.50
  - gain so voiced RMS → −18 dBFS (±36 dB limits)
  - comp threshold = voiced RMS after gain + 2
- [x] Verify: run the full `DspPipeline` over the captured voice with the derived params (and the current preset/macros). Adjust makeup until output peaks fall in −6…−3 dBFS, at most 4 iterations; if it doesn't converge, use the closest.
- [x] Failures as listed. Clipping = > 0.5 % of samples ≥ −0.3 dBFS. Not enough speech = < 1.5 s voiced. Too noisy = SNR < 10 dB.
- [x] Tilt is reported (a suggestion for Body) and not auto-applied.

## Acceptance criteria
- `noisy_quiet` fixture → gate within noise RMS + 6 ±0.5 dB, denoise 0.85, simulated peaks in −6…−3 dBFS.
- Each failure triggered by a dedicated synthetic input, with the right variant and payload.
- Deterministic: bit-identical report for identical input.
- `push` allocation-free; `finish` < 50 ms for 5 s of 48 kHz audio (perf profile).

## Verification
```bash
# lib unit tests (nextest's positional filter matches test names):
cargo nextest run -p tine-dsp calibrator
# integration suite (binary name is `calibrator`; tests do not contain that word):
cargo nextest run -p tine-dsp --test calibrator
# perf budget test (not run by default):
cargo test -p tine-dsp --profile perf --test calibrator -- --ignored --nocapture
```

## Requests
None — only files under **Owns** were changed, plus this task file and the
`06-decisions.md` append-only log (D-023).

## Result

`AutoCalibrator` is implemented in `crates/tine-dsp/src/calibrator.rs` (~900 lines
with docs + unit tests), tested by 15 integration tests
(`crates/tine-dsp/tests/calibrator.rs`) + 3 lib unit tests. All spec ambiguities
were resolved and recorded as **D-023** (`docs/project/06-decisions.md`).

### Acceptance criteria — measured evidence

- **`noisy_quiet` fixture** (9 s layout built at test time: 1 s discard + 3 s
  pink noise + 5 s quiet speech; `noisy_quiet_fixture_meets_acceptance`):
  noise RMS (median of 100 ms windows) **−43.86 dBFS** → gate **−37.86 dB**
  (= noise + 6 within 0.00 dB, criterion ±0.5 dB), **denoise 0.85**, simulated
  peak **−4.50 dBFS** (inside −6…−3), simulated RMS −18.85 dBFS. Derived:
  gain **+9.12 dB** (voiced RMS −27.12 → −18 target), comp threshold **−16 dB**
  (= −18 + 2), verified makeup **+3.67 dB**, crest **12.16 dB**, tilt **+12.51 dB**
  (low-heavy speech proxy → UI can suggest lowering Body).
- **Every failure variant** triggered by a dedicated synthetic input with the
  right payload (`digital_silence_is_too_quiet`, `full_scale_voice_rejects_clipping`,
  `sparse_speech_rejects_not_enough_speech`, `speech_at_noise_floor_rejects_too_noisy`,
  `inaudible_mic_rejects_gain_out_of_range`, `finish_without_capture_is_too_quiet`):
  Clipping pct measured **8.80 %** vs test-computed **8.80 %** (full-scale sine,
  measured region = noise+voice); TooNoisy **snr ≈ 8 dB** (< 10); NotEnoughSpeech
  voiced **≈ 800 ms** (< 1500); GainOutOfRange **needed ≈ +54 dB** (> +36).
- **Deterministic**: bit-identical reports for identical input
  (`report_is_deterministic_and_chunk_agnostic` — capture in 480 vs 4801-sample
  blocks; plus `finish_is_reproducible_on_second_call`).
- **`push` allocation-free**: `push_is_allocation_free` under
  `#[global_allocator] AllocDisabler` + `assert_no_alloc`, 123-sample blocks
  (never window/frame aligned) including both in-push phase transitions and the
  empty-push edge.
- **`finish` < 50 ms for 5 s @ 48 kHz** (`finish_meets_50ms_budget`, warm,
  `--profile perf`): **34.06 ms** (33.84 ms under `--release`), peak −4.50 dB.

### Decisions taken (D-023)

- **SNR = whole-voice-segment RMS − noise RMS** (not the voiced-only RMS):
  with voiced windows *defined* as > noise+10, a voiced-window SNR is always
  ≥ 10 dB and the TooNoisy rejection would be mathematically unreachable.
- **TooQuiet** = measured-capture peak < −80 dBFS (muted/dead mic; also covers
  `finish()` before any capture). Precedence: Clipping → TooQuiet →
  NotEnoughSpeech → TooNoisy → GainOutOfRange.
- **`changes` rows** are full `MacroMapper::apply` values (with vs. without the
  calibration, same macros/pins/current) — macro-shifted, so e.g. the gate row
  `before` is −53 dB (preset −50 + dynamic purity 0.4), not the raw preset base.
- **Offline verification**: stages 1–4 (input→cleanup→EQ→de-esser) run **once**
  per finish (they don't depend on makeup); compressor+limiter re-run fresh per
  makeup candidate. `offline_verification_matches_full_pipeline_bit_exactly`
  proves the simulation is **bit-identical** to a fresh `DspPipeline::new(fs, 480)`
  on the same params+audio (peak and RMS exactly equal) — same semantics, ~3×
  cheaper, which is what keeps finish() at ~34 ms.
- **Makeup is SPECS-clamped to 0…+24 dB**, so peaks that already exceed −3 dBFS
  at 0 dB makeup cannot be repaired; the search then keeps makeup 0 (closest).

### Surprises / notes

- The naive "re-run the whole pipeline per candidate" would cost ~100 ms
  (4 × 24.8 ms at the T09-measured 49.7 µs/480); the stage split + bit-exactness
  proof is what makes the 50 ms budget pass with margin.
- nextest's positional filter matches *test names*: `cargo nextest run -p
  tine-dsp calibrator` runs only the 3 lib unit tests — the integration suite
  needs `--test calibrator` (both in Verification above).
- Concurrent cargo invocations on this machine occasionally queue on the shared
  Cargo registry/target locks for minutes; retries succeed.

### Follow-ups (for T16/T20 owners)

- T16 (Auto-Tune runner) feeds `push()` on a non-RT capture thread (APO in
  bypass); `CalProgress.live_level_db` is per-push-call RMS — smooth it ~60 Hz
  for the wizard meter. `finish()` may allocate and block ~35 ms.
- T20 (wizard UI) renders from `CalReport.changes` + `Calibration` (tilt is a
  Body *suggestion*); failure payloads carry all user-facing numbers
  (pct / snr_db / voiced_ms / needed_db). `TooQuiet` should map to
  "your microphone seems muted" copy.
