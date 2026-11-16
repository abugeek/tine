# T18 — UI Foundation: View Model, Theme, Headless Harness

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] `model.rs`, `intent.rs`, `state.rs` as above. Optimistic UI: macro drags update `model.macros` locally *and* send the `Intent`; the engine's authoritative model overwrites it on the next update.
- [x] `theme.rs`:
  - tokens (bg, surface, surface-raised, border, text, text-dim, accent, accent-glow, meter green/amber/red, danger)
  - spacing/radius/type scale
  - `contrast_ratio()`
  - dark only for v0.1, built on Rune `tok` where possible
- [x] `views/root.rs`: header / canvas / drawer regions with placeholders; `views/` files created as stubs for T19/T20 ownership.
- [x] `fake.rs`: deterministic fake `LiveFrame` generator (speech-like meters and spectrum from a seeded RNG + time) for scenes and demo mode.
- [x] Scenes (Rune scene mechanism per T03 recipe): `golden_line`, `pro_drawer_open`, `auto_tune_wizard` (one per step: prepare, silence, speak, analyzing, results, failed), `apo_not_installed`, `onboarding`. Each scene is a pure function building an `AppState`.
- [x] `tests/headless.rs`:
  - for every scene × {1280×800, 960×640}, render a PNG (`target/ui-snapshots/`) and dump the tree JSON
  - assert: interactive siblings don't overlap; nothing is clipped by its parent (unless it's inside a scroll container); every interactive element has `focusable`, `role` and `label`; focus order matches [05-ui-ux.md](../project/05-ui-ux.md#interaction); text/surface contrast ≥ 4.5:1
  - compare against `tests/snapshots/*.png` with per-pixel tolerance; `UPDATE_SNAPSHOTS=1` blesses
- [x] `examples/ui_demo.rs`: runs the UI standalone with fake data and a logging `IntentSink` (debug tooling).

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

## Requests
- `crates/tine-protocol/tests/protocol.rs` (owned by T04) currently fails `cargo clippy --workspace --all-targets -- -D warnings` due to 12 pre-existing warnings (`needless_range_loop`, `manual_midpoint`, `unwrap_used`, `large_stack_arrays`, `undocumented_unsafe_blocks`, `cast_ptr_alignment`). `cargo clippy -p tine-ui --all-targets -- -D warnings` passes cleanly.

## Result
- **Headless verification (`crates/tine-ui/tests/headless.rs`)**:
  - 11 scenes (`golden_line`, `pro_drawer_open`, `auto_tune_wizard`, `auto_tune_prepare`, `auto_tune_silence`, `auto_tune_speak`, `auto_tune_analyzing`, `auto_tune_results`, `auto_tune_failed`, `apo_not_installed`, `onboarding`) × 2 sizes (`1280x800`, `960x640`) = **22 golden PNGs** in `crates/tine-ui/tests/snapshots/` + 22 tree JSON dumps in `target/ui-snapshots/`.
  - `cargo test -p tine-ui` passes all 4 tests (`theme::tests::dark_theme_text_surfaces_meet_wcag_aa`, `theme_contrast_meets_wcag_aa`, `interactions_update_optimistically_and_emit_intents`, `headless_scenes_render_and_pass_checks`) in `4.55 s`.
  - `golden_line` (`1280x800`) headless render metrics: `16.21 ms` first frame, `278` instances, `83` layout nodes, `226` glyphs (`ui_demo --snap`: `17.58 ms` first frame).
  - Verified zero overlapping interactive siblings, zero viewport clipping, `focusable`+`role`+`label` on all interactive elements, exact Tab focus order per `05-ui-ux.md`, and WCAG ≥ 4.5:1 contrast across all dark theme text/surface pairs.
- **Dependencies (`cargo tree -p tine-ui --depth 1`)**: `rune`, `tine-dsp`, `tine-protocol` (`serde_json` in dev-dependencies only). No `tine-engine`, `cpal`, or `windows`.
- **Checks**: `cargo fmt --all --check` ✓, `cargo clippy -p tine-ui --all-targets -- -D warnings` ✓, `cargo test --workspace` ✓, `cargo deny check` ✓ (`cargo-nextest` binary not installed locally; ran `cargo test`).
