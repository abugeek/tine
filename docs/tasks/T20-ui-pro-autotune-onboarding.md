# T20 — Pro Rack, Auto-Tune Overlay, Onboarding

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] `[Pro Controls ▾]` pill toggles the drawer with spring motion; the open state is kept in `UiLocal` and persisted through an `Intent`.
- [x] One section per stage 1–6. Widgets are **generated from `SPECS`**:
  - slider (log scale where `Scale::Log`), numeric field with unit, stage enable switch, EQ band type select
  - numeric input clamps to range and shows an inline message
- [x] Editing a parameter sends `SetParam`, so the param becomes pinned. The pin icon unpins on click; the section header has **Unpin all**.
- [x] Live GR meters: gate, de-esser, compressor. The output meter shows a ceiling marker.
- [x] Drawer content scrolls vertically at 960×640, with no clipping outside the scroll viewport.

### Auto-Tune overlay
- [x] Rune `.modal()` overlay; steps: Prepare (tips: "stay silent", "speak normally at your usual distance") → Silence 3 s (countdown ring + live level) → Speak 5 s (suggested sentence + level) → Analyzing → Results or Failed.
- [x] Results card:
  - measured noise floor, speech level, SNR
  - table of changed params (before → after)
  - "Keep my Pro pins" checkbox (D-007)
  - **Apply** / **Discard** / **Retry**
- [x] Failure card per `CalFailure`, with a concrete fix (for example: "Your mic is clipping — lower its hardware gain or move back").
- [x] `Esc` cancels (`CancelAutoTune`); focus is trapped in the modal and returns to the Auto-Tune button afterwards.

### Onboarding (first run)
- [x] Step 1: pick the mic (suggested archetype pre-selected from form factor).
- [x] Step 2: install the APO (explains the one UAC prompt and what changes on the system), with progress, success, or a typed error with remedies.
- [x] Step 3: Auto-Tune (skippable).
- [x] Step 4: "Open Discord/OBS and pick your normal mic, it's processed now", with a Hear Myself test button.

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

## Result

**T11 alignment.** After T11 merged (08c4914) the wizard was re-aligned with the *real* `tine_dsp::calibrator` semantics: `AutoTuneStep::Prepare { remaining_secs }` mirrors the 1 s `Countdown` phase (ring), `AutoTuneUi::voiced_ms` mirrors `CalProgress::voiced_ms` (Speak step shows captured-seconds-so-far), `CalibrationUi` carries the report's offline-verified `simulated_peak_db`/`simulated_rms_db` (results card shows "Verified peak −4.2 dBFS"), and `SILENCE_SECS`/`SPEAK_SECS` are the real `NOISE_S`/`VOICE_S` constants (3 s + 5 s measured; the countdown second is discarded, hence "8-second wizard"). The bridge is **one narrow adapter in `views/autotune.rs`**: `autotune_from_progress(&CalProgress, Option<CalFailure>) -> AutoTuneUi`, `From<CalFailure> for CalFailureUi`, `calibration_from_report(&CalibrationReport) -> CalibrationUi`. Nothing else in `tine-ui` knows the DSP types; `tine-ui` still depends only on `tine-protocol` + `tine-dsp` (never `tine-engine`). The single `TODO(T11)` left is engine-side: T16/T21's capture runner owns the `push` thread, keeps the `finish()` `Err(CalFailure)` payload, and replays the report's changes on `Intent::ApplyCalibration { keep_pins }` (pinned params stay un-driven, D-007).

**Adapter proven against the real engine.** `tine-ui` lib test `adapter_tracks_the_real_calibrator_state_machine` drives the *actual* `AutoCalibrator` at 48 kHz in host-sized 480-sample blocks: 1 s countdown → Prepare ring ≈1.0 s; 3 s noise (deterministic LCG, ~−51 dBFS RMS) → Silence ring tracks `phase_progress` (1.5 s after half the segment, overall progress 0.1875); 5 s of 150 Hz sine at −23 dBFS → Speak with `voiced_ms` ≥ 1500; 9 s → Analyzing latches (extra audio ignored, progress 1.0); `finish()` → Done → Results, and the synthetic capture passes every real `check_failures` gate (SNR ~28 dB, gain in range, no clipping) with `simulated_peak_db` proven inside −6..−3 dBFS; a digital-silence capture fails exactly like the engine (`TooQuiet` → the Failed card, remedy names the mic). `calibration_from_report` is asserted exact per field (noise/speech RMS, simulated peak/RMS, 5 labelled before→after diffs).

**Evidence (all measured, this worktree, branch tip after rebase onto fbd6d3d):**
- `cargo nextest run -p tine-ui`: **33 passed** (14 headless + 19 lib), 0 failed.
- `cargo nextest run --workspace`: **194 passed, 2 skipped**, 0 failed.
- `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo fmt --all --check`: clean.
- Scenes at 1280×800 **and** 960×640 all pass PNG + tree-JSON checks (no overlap/clipping, contrast ≥ 4.5:1, focus order): `pro_drawer_open` (2), `auto_tune_*` wizard steps (14), `onboarding_*` steps (12) — 28 new/uniquely re-blessed PNGs; the 6 pre-existing T19 scenes remain byte-identical. New scenes: `onboarding`, `onboarding_install`, `onboarding_installing`, `onboarding_install_error`, `onboarding_autotune`, `onboarding_finish`.
- `pro_rack_every_param_reachable_and_labelled`: iterates `tine_protocol::SPECS` and asserts every parameter (incl. header/bypass) has a reachable, labelled control in the `pro_drawer_open` tree — passes.
- `pro_rack_edit_pins_param_and_emits_intent`: scripted open → drag comp ratio → tree shows `SetParam(CompRatio, …)` in the intent sink and the pin icon; unpin via pin pill and via per-section **Unpin all**; drawer toggling emits `SetProDrawerOpen`.
- `auto_tune_wizard_scripted_run_through_every_step`: Prepare (inert background, focus trap) → Silence → Speak → Analyzing → Results (Apply carries `keep_pins: false` after unchecking) → Failed(Calipping remedy) → Retry re-emits `StartAutoTune`; Esc emits `CancelAutoTune` and focus returns to `header/autotune`.
- `onboarding_flow_walks_every_step`: steps 1–4, APO install states, Auto-Tune hand-off, Finish dismisses.
- `cargo run -p tine-ui --example ui_demo`: window opens, 300 frames drawn; profile `mean build+encode = 1.150 ms, ≈ 6.9 % of one core at 60 Hz` (T19 budget: < 4 ms) — headless budget test `headless_frame_build_and_encode_budget` also passes.

**Decisions taken.**
- Pro rack scrolls: `col().key("pro/scroll-viewport").grow(1).min_h(120).scroll(...)` + drawer `.grow(1).min_h(0)` when open — `el()` viewports never overflow (row container stretches), so `col()` is required; the tree-checker exempts `pro/scroll-viewport` descendants from window-clipping (horizontal + top still enforced, the viewport itself must fit).
- Collapsed drawer DOM is byte-identical to T19's (spring motion on open, `pill_button` fixed), so the 6 pre-existing snapshots did not change.
- Numeric field edit (`ProFieldEdit`) is one shared live-edit in `UiLocal` (only one active at a time; Enter commits parse/clamp, Esc cancels); the rack double-click gesture resets a slider to default.
- The T11 contract integration is exactly the three adapter functions + `CalFailureUi` in `model.rs`; `state.ui.autotune_keep_pins` (default keep, D-007) flows into `Intent::ApplyCalibration { keep_pins }`.

## Requests
- Touched shared tine-ui files beyond nominal Owns (same precedent as T19): `src/model.rs` (`CalFailureUi`, `Prepare { remaining_secs }`, `voiced_ms`, `simulated_peak/rms_db`), `src/state.rs` (`onboarding_step`, `apo_installing`, `autotune_keep_pins`, `ProFieldEdit`), `src/intent.rs` (`SetProDrawerOpen(bool)`), `src/lib.rs` (re-exports `CalFailureUi`), `src/scenes.rs` (T20 scenes + payload updates), `src/views/root.rs` (pro drawer + modal wiring into T19's placeholder), `tests/headless.rs` (T20 tests + scroll exemption). All edits are additive to T19's wiring and kept minimal.
- Engine hand-off (belongs to T16/T21, noted as TODO(T11) in `views/autotune.rs` module docs): the capture thread pushes mic blocks into `AutoCalibrator`, forwards `CalProgress` → `autotune_from_progress`, keeps `finish()`'s `Err(CalFailure)` for the Failed card, and replays the report's `changes` on `Intent::ApplyCalibration { keep_pins }`.
