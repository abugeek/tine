# T11 — `AutoCalibrator`

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: tine-worker) |
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
- [ ] Phases: Countdown 1 s (discarded) → Noise 3 s → Voice 5 s → Analyzing.
- [ ] Noise: median of 100 ms window RMS (robust to a click), max peak.
- [ ] Voice:
  - voiced windows are those > noise + 10 dB (20 ms windows)
  - integrated RMS over voiced windows
  - crest factor
  - spectral tilt from band energies < 350 Hz vs 1–6 kHz (biquad band filters from `primitives`)
- [ ] Derivation:
  - gate = noise + 6
  - denoise = 0.85 if noise > −48 dBFS else 0.50
  - gain so voiced RMS → −18 dBFS (±36 dB limits)
  - comp threshold = voiced RMS after gain + 2
- [ ] Verify: run the full `DspPipeline` over the captured voice with the derived params (and the current preset/macros). Adjust makeup until output peaks fall in −6…−3 dBFS, at most 4 iterations; if it doesn't converge, use the closest.
- [ ] Failures as listed. Clipping = > 0.5 % of samples ≥ −0.3 dBFS. Not enough speech = < 1.5 s voiced. Too noisy = SNR < 10 dB.
- [ ] Tilt is reported (a suggestion for Body) and not auto-applied.

## Acceptance criteria
- `noisy_quiet` fixture → gate within noise RMS + 6 ±0.5 dB, denoise 0.85, simulated peaks in −6…−3 dBFS.
- Each failure triggered by a dedicated synthetic input, with the right variant and payload.
- Deterministic: bit-identical report for identical input.
- `push` allocation-free; `finish` < 50 ms for 5 s of 48 kHz audio (perf profile).

## Verification
```bash
cargo nextest run -p tine-dsp calibrator
```
