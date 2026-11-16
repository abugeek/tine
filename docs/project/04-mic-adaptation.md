# 04 — Microphone Adaptation (Archetypes, Auto-Tune, Macros)

Tine never ships one fixed EQ. Final parameters are produced by three layers, applied in order:

```text
DspParams = MacroMapper( archetype_base  ⊕  calibration_result ,  macro_positions )  ⊕  pro_overrides
```

## Layer A — Archetype base profiles

Stored as JSON in `assets/presets/*.json`, validated against `tine-protocol` ranges at build time (test).

| Archetype | Problem | Base curve |
| :--- | :--- | :--- |
| **Dynamic USB/XLR** (Fifine AM8, SM7B, PodMic) | Dark highs, boxy 300–400 Hz, needs gain | HPF 80 Hz · LS +2.0 dB @110 Hz · Peak −3.0 dB @340 Hz Q1.4 · HS +3.5 dB @5.5 kHz |
| **Studio condenser** (QuadCast, Yeti, AT2020) | Room echo, sibilance 6–8 kHz, plosives | HPF 95 Hz · LS +0.5 dB @120 Hz · Peak −2.0 dB @280 Hz · De-esser on @6.8 kHz · HS +1.0 dB @10 kHz |
| **Gaming headset / boom** | Thin bass, nasal 800–1200 Hz, breath bursts | HPF 100 Hz · LS +4.0 dB @130 Hz · Peak −3.5 dB @950 Hz Q1.2 · HS +2.0 dB @4.5 kHz |
| **Laptop / webcam / lav** | Distant, roomy, noisy, no warmth | HPF 110 Hz · RNNoise mix 0.90 · Expander 4:1 · Peak +3.0 dB @2.5 kHz |

Preset JSON shape (versioned):

```json
{ "schema": 1, "id": "dynamic", "name": "Dynamic USB / XLR",
  "params": { "hpf_freq_hz": 80, "eq": [ {"type":"low_shelf","freq_hz":110,"gain_db":2.0,"q":0.707}, ... ] },
  "macro_defaults": { "purity": 0.4, "body": 0.5, "presence": 0.5, "leveler": 0.5 } }
```

## Layer B — Auto-Tune (8 seconds)

State machine `AutoCalibrator` (pure, in `tine-dsp`; fed audio by `tine.exe`). The APO is put in **bypass** for the
duration so measurements see the raw mic.

| Phase | Duration | Measures | Sets |
| :--- | :--- | :--- | :--- |
| Countdown | 1 s (not measured) | — | — |
| **1. Room noise** | 3 s silence | Noise peak & RMS dBFS (median of 100 ms windows, robust to a single click) | `gate_threshold_db = noise_rms + 6`; `denoise_mix = 0.85` if `noise_rms > −48` else `0.50` |
| **2. Voice** | 5 s normal speech | Integrated RMS of voiced windows only (windows > noise + 10 dB), crest factor, spectral tilt = E(<350 Hz) / E(1–6 kHz) | `input_gain_db` so speech RMS → −18 dBFS; `comp_thresh_db = vocal_rms_after_gain + 2`; makeup so output peaks land in −6 … −3 dBFS |
| Verify | instant | Simulate the full pipeline offline over the captured 5 s | If peaks miss −6…−3, adjust makeup and re-simulate (max 4 iterations) |

Spectral tilt optionally nudges `Body` (too much low energy → lower Body) — shown as a suggestion, not auto-applied, in v0.1.

**Rejections** (shown in the wizard, offer retry): voice segment < 1.5 s of voiced audio; SNR < 10 dB; clipping (> −0.3 dBFS) in > 0.5 % of samples
(tell user to lower hardware gain); input gain needed > +36 dB.

## Layer C — Macros ("Golden Line") and Pro mode

Four normalized nodes `0.0 … 1.0`. Each mapping is a pure function in `tine-dsp::macros`, tested for monotonicity and range.
Positions are **centred on the effective base** (preset Layer A value, or the calibrated value when one exists), so `0.5` always reproduces the archetype exactly — EQ-gain offsets are *added* to the preset base (D-021). `a`/`b` below denote that effective base value; `u(m) = max(0, 2m − 1)` (upper half).

| Macro | Drives |
| :--- | :--- |
| **Purity** | `denoise_mix` = a ± 0.5; `gate_threshold_db` = a ± 15 dB (both rise: more purity ⇒ more AI wet, less gating) |
| **Body** | Band 1 (eq.0) low-shelf @110 Hz gain = b ± 4 dB; Band 2 (eq.1) mud @320 Hz gain = b − 5 dB · u(m) (cut deepens only above centre) |
| **Presence** | Band 4 (eq.3) peak @4.2 kHz gain = b + 5 dB · u(m); Band 5 (eq.4) shelf @8.5 kHz gain = b + 6 dB · u(m); de-esser enabled = base_enabled ∨ m > 0.6; amount = max(b, 3 + 5·clamp((m − 0.6)/0.4)) dB (3 → 8 dB over 0.6 → 1.0) |
| **Leveler** | `comp_ratio` = b ± 1.75; `comp_thresh_db` = a − 10 dB · u(m) ("calibrated threshold − 0…10 dB"); auto-makeup on |

A calibration replaces the preset's `input_gain_db`, `gate_threshold_db`, `denoise_mix`, `comp_thresh_db` and `comp_makeup_db` (its verified manual makeup disables auto-makeup); Purity/Leveler then move relative to those anchors. Pinned parameters are not driven at all (D-007). All outputs clamp through the `tine-protocol` `SPECS` ranges.

### Pro mode interaction rule (D-007)

- Pro edits write the same `DspParams`. A parameter edited in Pro becomes **pinned** (small pin icon); macros stop driving pinned parameters.
- Moving a macro shows which pinned parameters it is no longer controlling. "Unpin all" returns control to macros.
- Changing archetype or re-running Auto-Tune asks whether to keep pins.
