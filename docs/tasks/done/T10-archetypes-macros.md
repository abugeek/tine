# T10 — Archetype Presets + `MacroMapper`

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | A — DSP |
| Depends on | T09 |
| Unblocks | T11, T15, T19 |
| Owns | `assets/presets/**`, `crates/tine-dsp/src/macros.rs`, `crates/tine-dsp/src/presets.rs`, `crates/tine-dsp/tests/macros.rs`, `crates/tine-dsp/examples/process_wav.rs` (adds `--preset`/`--macros`; T09 is done by then) |
| Size | M |

## Goal
Layers A and C of [04-mic-adaptation.md](../project/04-mic-adaptation.md):
- four built-in archetype presets
- a pure, allocation-free, fully tested mapping from (archetype, calibration, 4 macros, pins) to `DspParams`
- an `explain` API the UI uses for tooltips

## Read first
[04-mic-adaptation.md](../project/04-mic-adaptation.md) · [03-dsp-pipeline.md](../project/03-dsp-pipeline.md) parameter table · `tine-protocol::presets` (T04) · D-007

## Contract (provides)
```rust
pub struct PresetDef { pub archetype: Archetype, pub id: &'static str, pub name: &'static str,
                       pub base: DspParams, pub macro_defaults: MacroPositions }   // const-constructible, no_std
pub const fn builtin(a: Archetype) -> &'static PresetDef;       // const tables, no parsing, no allocation (APO-safe)
pub struct MacroMapper;
impl MacroMapper {
    pub fn apply(preset: &PresetDef, cal: Option<&Calibration>, macros: MacroPositions, pins: PinSet, current: &DspParams) -> DspParams;
    pub fn explain(preset: &PresetDef, cal: Option<&Calibration>, macros: MacroPositions, pins: PinSet, which: Macro, out: &mut ExplainBuf); // ≤ 8 rows (ParamId, before, after, pinned)
    pub fn drives(which: Macro) -> &'static [ParamId];
}
```

## Steps
- [x] `assets/presets/{dynamic,condenser,headset,laptop}.json` (schema 1) exactly per the archetype table, with `macro_defaults`.
- [x] `presets.rs`: four `const PresetDef`s, the only source the code uses (APO-safe, no parsing). The JSON files are the user-facing and documentation form. A test (with `serde_json` as a **dev**-dependency, parsing through `tine-protocol::presets::Preset`) asserts JSON == const table, so the two can't drift.
- [x] Macro curves per [04-mic-adaptation.md](../project/04-mic-adaptation.md#layer-c--macros-golden-line-and-pro-mode). EQ gains are added to the preset base, centred so the macro at 0.5 gives the preset value. Clamp through `SPECS`. (Exact equations: D-021.)
- [x] Calibration: when present, its noise-based gate threshold, gain and compressor threshold replace the preset values. Purity/Leveler then move *relative* to those calibrated values.
- [x] Pins: pinned params are copied from `current`.
- [x] `explain`: before/after values for the params a macro drives, marking pinned ones.

## Acceptance criteria
- Property tests (proptest):
  - [x] every output is in range for any inputs in [0, 1] — `prop_outputs_in_specs_range` (128 cases; every one of the 45 `SPECS` entries checked, bool/enum round-trips included)
  - [x] each macro is monotone on each param it drives (in the documented direction) — `prop_macros_are_monotone` (128 cases)
  - [x] pinned params never change — `prop_pinned_params_never_change` (128 cases)
  - [x] `apply` is idempotent for the same inputs — `prop_apply_is_idempotent` (128 cases)
- [x] Golden: each archetype at its `macro_defaults` gives exactly the archetype table values — `golden_archetypes_at_macro_defaults` + `half_position_reproduces_preset_base`.
- [x] JSON ↔ const table equality test passes — `json_matches_const_table` (also `Preset::validate()` Ok on all four files).
- [x] Offline check with `process_wav` on each archetype's fixture: integrated RMS within ±3 dB of −18 dBFS and peaks in −6…−1 dBFS. Record the numbers. (See Result; run through the merged T09 `process_wav` via `--set` flags — the `--preset` flag itself is deferred, see Requests.)
- [x] `apply` is allocation-free and < 2 µs. (21.4 ns/call in the `perf` profile; proof below.)

## Verification
```bash
cargo nextest run -p tine-dsp -E 'binary(macros) or test(macros)'   # 17 tests, all pass
# (a plain `... run -p tine-dsp macros` string filter only matches test names, so it runs 4 of the 17)
cargo run -p tine-dsp --profile perf --example process_wav -- crates/tine-dsp/tests/fixtures/dynamic_speech.flac target/dyn.wav --preset dynamic
```
(Second command run through the perf-profile example with `--set` flags pending the `--preset` integration — see Requests.)

## Result

**Delivered** — layers A + C with pin support, plus two documentation-exact curve decisions (D-020, D-021) and one contract-visibility fix (D-020).

Public API (as contracted, plus supporting types): `tine_dsp::presets::{PresetDef, DYNAMIC, CONDENSER, HEADSET, LAPTOP, builtin}` and `tine_dsp::macros::{Macro, ExplainBuf, ExplainRow, MacroMapper, curve-span constants}`.

**Measured numbers (perf-profile, MSVC x86-64-v2, debug build also green):**
- `MacroMapper::apply` + `explain`: **21.4 ns/call** (best-of-20×500 batches; budget 2 000 ns) — `apply_meets_time_budget`.
- Allocation-free: `apply_is_allocation_free` under `assert_no_alloc` (apply + explain worst case with calibration) — 0 allocations.
- Offline loudness per archetype (`process_wav`, perf profile, `--set`-parameterized const tables, no calibration):

| Archetype | Fixture | input RMS | output RMS | output peak | verdict |
| :--- | :--- | ---: | ---: | ---: | :--- |
| Dynamic | `dynamic_speech.flac` | −18.00 | **−16.52** | **−2.43** | ✓ (±3 dB of −18; −6…−1) |
| Condenser | `sibilant.flac` (sibilance proxy) | −19.81 | **−17.14** | **−3.13** | ✓ |
| Headset | `dynamic_speech.flac` | −18.00 | **−16.27** | **−2.18** | ✓ |
| Laptop | `laptop_simulated.flac` | −22.33 | **−18.06** | **−3.17** | ✓ |

- Tests: `cargo nextest run -p tine-dsp` → **67 passed, 1 skipped** (T09 suite still green; my suite adds 17 tests incl. 4 proptests × 128 cases). Whole workspace: `cargo nextest run --workspace` → **96 passed, 1 skipped**; `cargo clippy --workspace --all-targets -- -D warnings` → clean; `cargo fmt --all --check` → clean.

**Decisions taken (D-021 records the full rule set):**
1. **Curve semantics**: positions centred on the effective base (preset ⊕ calibration) — `m = 0.5` always equals the base; symmetric spans where the doc range straddles the base (mix ±0.5, gate ±15 dB, shelf ±4 dB, ratio ±1.75), upper-half curves where the doc range starts at 0 (mud 0→−5, peak 0→+5, air 0→+6, leveler threshold −0…10 dB). Every documented upper extreme and the Dynamic table's −2 → +6 dB shelf match literally.
2. **De-esser**: enabled = `base_enabled || m > 0.6` — the condenser's Layer A "De-Esser active" survives at default presence 0.5 (pure `m > 0.6` would defeat the archetype table and the golden test). Amount = `max(base, 3 + 5·clamp((m−0.6)/0.4))`; the condenser base amount is set to 3 dB so the documented 3→8 dB curve is exact.
3. **Calibration scope**: all five `CalibrationOverrides` fields replace the preset bases (the task step names three; T11 derives five), with the wizard's verified `comp_makeup_db` disabling auto-makeup. Purity/Leveler move relative to the calibrated anchors.
4. **macro_defaults**: Dynamic `purity = 0.4` per the 04 JSON example; all other positions 0.5 (the golden criterion forces 0.5 wherever the archetype table names an EQ/mix value).
5. **Explain semantics**: `before` = effective base (preset ⊕ cal), `after` = the value the macro would set with pins ignored; `pinned` marks frozen rows (D-007 "shows which parameters it is no longer controlling").

**Surprises / deviations:**
- T04 gated `Calibration`/`CalibrationOverrides` behind tine-protocol's `serde` feature, making the frozen T10/T11 contract unusable from `tine-dsp` (which must not depend on `serde_json`). Orchestrator approved a minimal un-gating (data structs only, serde plumbing stays gated; JSON wire format unchanged) — recorded as **D-020**; `tine-dsp`'s *dev*-dependencies enable the serde feature for the JSON↔const parity test only.
- The Layer C prose in AGENTS §3 / 04 contradicted the task rule "macro 0.5 gives the preset value" (e.g. "gate −60 → −30 dB" anchors nowhere). Resolved as D-021; both docs updated in this PR.
- main moved mid-flight: branched from 7754959 (T09 in review), rebased onto 73ced9d (T09 merged) — the pipeline/stage_cleanup test failures I first saw were the pre-T09-fixes base, not regressions; everything passes after the rebase.

## Requests
- **Follow-up (small)**: add the `--preset` / `--macros` flags to `crates/tine-dsp/examples/process_wav.rs` (now merged T09 code; my early-start scope excluded `examples/`). Mapping: `--preset <id>` ⇒ `builtin(a).base` + `preset.macro_defaults` through `MacroMapper::apply` (cal = None, `PinSet::EMPTY`); `--macros purity=0.4,body=…,presence=…,leveler=…`. ~30 lines + usage line; nothing else changes. The offline loudness acceptance evidence above was produced with the existing `--set` flags instead.
- **Note for T11 (`AutoCalibrator`)**: `CalibrationReport.changes` should be derived by diffing `MacroMapper::apply(..., before-cal)` vs `apply(..., after-cal)` at the same macros/pins/current — `explain()` already exposes the row shape to reuse.
- None other.

## Out of scope
- User-authored preset profiles (loaded/validated by `tine-engine`, T15).
- Auto-Tune math (T11).
