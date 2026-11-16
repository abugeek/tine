# T18 — UI Foundation: View Model, Theme, Headless Harness

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: T18-worker) |
| Lane | D — UI |
| Depends on | T03 (recipe), T04 |
| Unblocks | T19, T20 |
| Owns | `crates/tine-ui/**` (except files later owned by T19/T20: `src/views/{header,canvas}.rs` → T19, `src/views/{pro_rack,autotune,onboarding}.rs` → T20) |
| Size | M |

## Goal
A UI crate that is **pure and engine-agnostic**. It has a view model, messages, theme tokens, a layout with placeholder regions, and the automated headless verification loop from [`AGENTS.md` §6](../../AGENTS.md#mandatory-headless-verification-protocol-for-coding-agents). Lane D can then build every screen from fake data, without waiting for the engine.

## Read first
[05-ui-ux.md](../project/05-ui-ux.md) · T03 `## Result` (scene recipe) · Rune `crates/rune/README.md` ("Agent tooling", focus/keys, motion) and `examples/settings`, `examples/hello` at the pinned rev · [01-vision.md](../project/01-vision.md) principles

## Contract (provides — the boundary `tine-app` wires in T21)
```rust
pub struct UiModel { pub endpoints: Vec<EndpointRow>, pub selected: Option<usize>, pub apo: ApoBadge, pub archetype: Archetype,
                     pub macros: MacroPositions, pub pins: PinSet, pub params: DspParams, pub bypass: bool,
                     pub monitor: MonitorUi, pub calibration: Option<CalibrationUi>, pub autotune: AutoTuneUi, pub first_run: bool }
pub struct LiveFrame { pub telemetry: AudioTelemetryFrame, pub spectrum: [f32; 32] }
pub struct AppState { pub model: UiModel, pub live: LiveFrame, pub ui: UiLocal /* drawer open, hover, focus, wizard step */ }
pub enum Intent { SelectEndpoint(usize), SetArchetype(Archetype), SetMacro(Macro, f32), SetParam(ParamId, f32), Unpin(ParamId), UnpinAll,
                  ToggleBypass, ToggleMonitor, PickMonitorDevice(String), StartAutoTune, CancelAutoTune, ApplyCalibration{keep_pins: bool},
                  DiscardCalibration, InstallApo, RepairApo, OpenSettings, CloseWindow }
pub trait IntentSink: 'static { fn send(&self, i: Intent); }   // implemented by tine-app; a recording fake in tests
pub fn app_spec(sink: impl IntentSink, init: AppState) -> rune::AppSpec<AppState>;
pub fn view(s: &AppState, cx: &mut rune::Cx<AppState>) -> rune::El;
```
`tine-ui` may depend on `tine-protocol` and `tine-dsp` (pure math: `curve`, `MacroMapper::explain`). It must **not** depend on `tine-engine`, `cpal` or `windows`.

## Steps
- [ ] `model.rs`, `intent.rs`, `state.rs` as above. Optimistic UI: macro drags update `model.macros` locally *and* send the `Intent`; the engine's authoritative model overwrites it on the next update.
- [ ] `theme.rs`:
  - tokens (bg, surface, surface-raised, border, text, text-dim, accent, accent-glow, meter green/amber/red, danger)
  - spacing/radius/type scale
  - `contrast_ratio()`
  - dark only for v0.1, built on Rune `tok` where possible
- [ ] `views/root.rs`: header / canvas / drawer regions with placeholders; `views/` files created as stubs for T19/T20 ownership.
- [ ] `fake.rs`: deterministic fake `LiveFrame` generator (speech-like meters and spectrum from a seeded RNG + time) for scenes and demo mode.
- [ ] Scenes (Rune scene mechanism per T03 recipe): `golden_line`, `pro_drawer_open`, `auto_tune_wizard` (one per step: prepare, silence, speak, analyzing, results, failed), `apo_not_installed`, `onboarding`. Each scene is a pure function building an `AppState`.
- [ ] `tests/headless.rs`:
  - for every scene × {1280×800, 960×640}, render a PNG (`target/ui-snapshots/`) and dump the tree JSON
  - assert: interactive siblings don't overlap; nothing is clipped by its parent (unless it's inside a scroll container); every interactive element has `focusable`, `role` and `label`; focus order matches [05-ui-ux.md](../project/05-ui-ux.md#interaction); text/surface contrast ≥ 4.5:1
  - compare against `tests/snapshots/*.png` with per-pixel tolerance; `UPDATE_SNAPSHOTS=1` blesses
- [ ] `examples/ui_demo.rs`: runs the UI standalone with fake data and a logging `IntentSink` (debug tooling).

## Acceptance criteria
- `cargo nextest run -p tine-ui` renders all scenes and all checks pass in CI (Windows runner).
- `cargo tree -p tine-ui` shows no `tine-engine`, `cpal` or `windows`.
- The demo example runs at 60 Hz with < 3 % CPU.

## Verification
```bash
cargo nextest run -p tine-ui
UPDATE_SNAPSHOTS=1 cargo nextest run -p tine-ui
cargo run -p tine-ui --example ui_demo
```
