# 03 — DSP Pipeline Reference

Mono `f32`, nominal 48 kHz (see architecture §6 for other rates). Deterministic order; every stage is bypassable
by its own enable flag *and* by the global A/B bypass. All user-facing parameters pass through a one-pole smoother
(τ = 10 ms, per sample) before reaching coefficients. Biquad coefficients are recomputed at most once per 32 samples
from smoothed values (cheap and still zipper-free).

```text
in ─► [1 Gain · DC block · HPF] ─► [2 RNNoise mix · Expander] ─► [3 5-band EQ] ─► [4 De-esser]
   ─► [5 Compressor] ─► [6 Lookahead limiter] ─► [7 Meter tap] ─► out
```

## Stage details

### 1. Input gain, DC blocker, high-pass
- Gain in dB → linear (smoothed).
- DC blocker: one-pole HPF `y = x - x1 + R*y1`, R = 0.995 at 48 kHz (≈ 4 Hz corner; R derived from fs).
- HPF: 2nd-order Butterworth (Q = 0.7071) from `tine-dsp::primitives::biquad` (in-house RBJ designs, D-010), cutoff 20–200 Hz.

### 2. Denoiser + expander
- `nnnoiseless::DenoiseState` on 480-sample frames via input/output FIFOs. Scale: RNNoise expects 16-bit range, so multiply by 32768 in / divide out.
- Output = `dry_delayed * (1 - mix) + wet * mix` — dry path delayed by the same 480 samples so the blend is phase-aligned.
- Downward expander (soft knee 6 dB, hysteresis): detector = peak envelope (attack 1 ms, release from param).
  Opens at `threshold`, closes at `threshold - hysteresis`. Gain below threshold: `(ratio - 1) * (level - threshold)`, floor −80 dB.
  Reported as `gate_attenuation_db`.

### 3. Five-band parametric EQ
Cascaded Transposed Direct Form II biquads (in-house, coefficients designed in `f64`, run in `f32`; D-010). Default roles:

| Band | Default type | Default freq | Role |
| :--- | :--- | :--- | :--- |
| 1 | Low-shelf | 110 Hz | Warmth / body |
| 2 | Peaking | 320 Hz | Mud cut |
| 3 | Peaking | 950 Hz | Nasal / mid |
| 4 | Peaking | 4.2 kHz | Presence |
| 5 | High-shelf | 8.5 kHz | Air |

Every band's type is selectable in Pro mode (LowShelf, HighShelf, Peaking, Notch, LowPass, HighPass, BandPass).
Bands at 0 dB gain (peak/shelf) are skipped to save CPU.

### 4. Split-band de-esser
- Sidechain: band-pass 2nd order centred on `deesser_freq_hz` (5.5–9.5 kHz, default 6.8 kHz), fast RMS (attack 0.5 ms, release 40 ms).
- Split: complementary Linkwitz–Riley crossover at `deesser_freq_hz`; gain reduction applied to the high band only, so vowels are untouched.
- GR = min(`deesser_amount_db`, ratio-4 reduction above `deesser_thresh_db`). Reported as `deesser_reduction_db`.

### 5. Compressor
- Detector: blend of RMS (≈ 10 ms window) and peak (`fast transient catch`), log-domain, soft knee (default 6 dB).
- Attack / release in ms, program-dependent release (release shortens by up to 50 % after short transients).
- Makeup: manual dB, or auto = `-(threshold - threshold/ratio) * 0.5` (half the static GR at 0 dBFS), overridable.
- Reported as `comp_gain_reduction_db`.

### 6. Brickwall lookahead limiter
- 1.5 ms (72 samples @ 48 kHz) delay line; gain computed on a sliding-window max of |x| over the lookahead window
  (monotone deque on a fixed ring, O(1) amortised), attack ramp across the lookahead, release 50 ms.
- Ceiling −1.0 dBFS default (range −12 … −0.1). Sample-peak by default; optional 4× oversampled true-peak detection (Pro).
- Guarantee: no output sample exceeds the ceiling (tested with full-scale square waves and impulses).

### 7. Metering tap
- Per block: input/output peak & RMS (dBFS), the three GR values. Published ~60 Hz via the telemetry triple buffer.
- Post-limiter mono samples pushed to the tap ring; **spectrum (32 log bands, 20 Hz–20 kHz) is computed in tine.exe**, not in audiodg.

## Parameter table (`DspParams`)

Defaults below are neutral; archetypes and macros overwrite them (see [04-mic-adaptation.md](04-mic-adaptation.md)).

| Field | Range | Default | Unit |
| :--- | :--- | :--- | :--- |
| `bypass` | bool | false | |
| `input_gain_db` | −24 … +36 | 0 | dB |
| `hpf_enabled` / `hpf_freq_hz` | 20 … 200 | true / 80 | Hz |
| `denoise_mix` | 0 … 1 | 0.5 | |
| `gate_enabled` | bool | true | |
| `gate_threshold_db` | −80 … −20 | −50 | dBFS |
| `gate_ratio` | 1 … 10 | 2 | :1 |
| `gate_hysteresis_db` | 0 … 12 | 4 | dB |
| `gate_attack_ms` | 0.1 … 20 | 1 | ms |
| `gate_release_ms` | 10 … 1000 | 150 | ms |
| `eq_type[5]` | enum | see table | |
| `eq_freq_hz[5]` | 20 … 20000 | see table | Hz |
| `eq_gain_db[5]` | −18 … +18 | 0 | dB |
| `eq_q[5]` | 0.1 … 10 | 0.707 shelves / 1.0 peaks | |
| `deesser_enabled` | bool | false | |
| `deesser_freq_hz` | 5500 … 9500 | 6800 | Hz |
| `deesser_thresh_db` | −60 … 0 | −30 | dBFS |
| `deesser_amount_db` | 0 … 12 | 6 | dB max GR |
| `comp_enabled` | bool | true | |
| `comp_thresh_db` | −60 … 0 | −20 | dBFS |
| `comp_ratio` | 1 … 20 | 3 | :1 |
| `comp_attack_ms` | 0.1 … 100 | 5 | ms |
| `comp_release_ms` | 10 … 1000 | 120 | ms |
| `comp_knee_db` | 0 … 12 | 6 | dB |
| `comp_makeup_db` / `comp_auto_makeup` | 0 … 24 / bool | 0 / true | dB |
| `limiter_ceiling_db` | −12 … −0.1 | −1.0 | dBFS |
| `limiter_true_peak` | bool | false | |

All ranges are enforced by `tine-protocol::ranges` (single source of truth used by DSP clamping, UI sliders and preset validation).

## Performance contract

- 480-sample frame, all stages on, release build, single core: **< 150 µs** (target < 60 µs). Measured by `cargo bench -p tine-dsp`.
- Zero heap allocations after `DspPipeline::new(sample_rate, max_block)`.
- Denormals: flush-to-zero set on RT thread entry (MXCSR FTZ/DAZ) and tiny DC offset (1e-20) added in feedback paths as a backstop.
- Output is always finite: a NaN/Inf guard at the end resets state and latches pass-through.
