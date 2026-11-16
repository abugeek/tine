# T10 — Archetype Presets + `MacroMapper`

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: tine-worker) |
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
- [ ] `assets/presets/{dynamic,condenser,headset,laptop}.json` (schema 1) exactly per the archetype table, with `macro_defaults`.
- [ ] `presets.rs`: four `const PresetDef`s, the only source the code uses (APO-safe, no parsing). The JSON files are the user-facing and documentation form. A test (with `serde_json` as a **dev**-dependency, parsing through `tine-protocol::presets::Preset`) asserts JSON == const table, so the two can't drift.
- [ ] Macro curves per [04-mic-adaptation.md](../project/04-mic-adaptation.md#layer-c--macros-golden-line-and-pro-mode). EQ gains are added to the preset base, centred so the macro at 0.5 gives the preset value. Clamp through `SPECS`.
- [ ] Calibration: when present, its noise-based gate threshold, gain and compressor threshold replace the preset values. Purity/Leveler then move *relative* to those calibrated values.
- [ ] Pins: pinned params are copied from `current`.
- [ ] `explain`: before/after values for the params a macro drives, marking pinned ones.

## Acceptance criteria
- Property tests (proptest):
  - every output is in range for any inputs in [0, 1]
  - each macro is monotone on each param it drives (in the documented direction)
  - pinned params never change
  - `apply` is idempotent for the same inputs
- Golden: each archetype at its `macro_defaults` gives exactly the archetype table values.
- JSON ↔ const table equality test passes.
- Offline check with `process_wav` on each archetype's fixture: integrated RMS within ±3 dB of −18 dBFS and peaks in −6…−1 dBFS. Record the numbers.
- `apply` is allocation-free and < 2 µs.

## Verification
```bash
cargo nextest run -p tine-dsp macros
cargo run -p tine-dsp --profile perf --example process_wav -- crates/tine-dsp/tests/fixtures/dynamic_speech.flac target/dyn.wav --preset dynamic
```
(Add `--preset` and `--macros` flags to `process_wav`; coordinate with the T09 owner if T09 is still in review.)
