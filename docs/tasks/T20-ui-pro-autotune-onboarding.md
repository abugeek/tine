# T20 — Pro Rack, Auto-Tune Overlay, Onboarding

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | D — UI |
| Depends on | T11, T19 |
| Unblocks | T21 |
| Owns | `crates/tine-ui/src/views/{pro_rack,autotune,onboarding}.rs`, `crates/tine-ui/tests/snapshots/{pro_drawer*,auto_tune*,onboarding*}` |
| Size | L |

## Goal
The expert controls for every parameter, the guided 8-second calibration, and the first-run flow that gets a new user to a good sound in under 2 minutes.

## Read first
[05-ui-ux.md](../project/05-ui-ux.md) · [04-mic-adaptation.md](../project/04-mic-adaptation.md) (pins D-007, Auto-Tune failures) · `tine-protocol::SPECS` · `CalibrationReport`/`CalFailure` (T11)

## Steps
### Pro rack
- [ ] `[Pro Controls ▾]` pill toggles the drawer with spring motion; the open state is kept in `UiLocal` and persisted through an `Intent`.
- [ ] One section per stage 1–6. Widgets are **generated from `SPECS`**:
  - slider (log scale where `Scale::Log`), numeric field with unit, stage enable switch, EQ band type select
  - numeric input clamps to range and shows an inline message
- [ ] Editing a parameter sends `SetParam`, so the param becomes pinned. The pin icon unpins on click; the section header has **Unpin all**.
- [ ] Live GR meters: gate, de-esser, compressor. The output meter shows a ceiling marker.
- [ ] Drawer content scrolls vertically at 960×640, with no clipping outside the scroll viewport.

### Auto-Tune overlay
- [ ] Rune `.modal()` overlay; steps: Prepare (tips: "stay silent", "speak normally at your usual distance") → Silence 3 s (countdown ring + live level) → Speak 5 s (suggested sentence + level) → Analyzing → Results or Failed.
- [ ] Results card:
  - measured noise floor, speech level, SNR
  - table of changed params (before → after)
  - "Keep my Pro pins" checkbox (D-007)
  - **Apply** / **Discard** / **Retry**
- [ ] Failure card per `CalFailure`, with a concrete fix (for example: "Your mic is clipping — lower its hardware gain or move back").
- [ ] `Esc` cancels (`CancelAutoTune`); focus is trapped in the modal and returns to the Auto-Tune button afterwards.

### Onboarding (first run)
- [ ] Step 1: pick the mic (suggested archetype pre-selected from form factor).
- [ ] Step 2: install the APO (explains the one UAC prompt and what changes on the system), with progress, success, or a typed error with remedies.
- [ ] Step 3: Auto-Tune (skippable).
- [ ] Step 4: "Open Discord/OBS and pick your normal mic, it's processed now", with a Hear Myself test button.

## Acceptance criteria
- Scenes `pro_drawer_open`, each `auto_tune_*` step, and each `onboarding_*` step pass every headless check at both sizes.
- A test iterates `SPECS` and asserts that every parameter has a reachable, labelled control in the `pro_drawer_open` tree.
- Scripted run: open drawer → change `comp_ratio` → the intent log shows `SetParam(CompRatio, …)`, and the pin icon appears in the tree.
- Scripted run of the wizard through every step with fake progress.

## Verification
```bash
cargo nextest run -p tine-ui
cargo run -p tine-ui --example ui_demo
```
