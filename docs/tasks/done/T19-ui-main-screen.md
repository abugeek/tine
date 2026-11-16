# T19 — Header + Golden Line Canvas

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | D — UI |
| Depends on | T10, T18 |
| Unblocks | T20 |
| Owns | `crates/tine-ui/src/views/{header,canvas}.rs`, `crates/tine-ui/tests/snapshots/{golden_line*,apo_not_installed*}` |
| Size | L |

## Goal
The screen a normal user lives in:
- choose the mic and see its APO state
- pick an archetype, run Auto-Tune, toggle Hear Myself and A/B
- shape the sound with 4 macro nodes over a live spectrum and a response line that matches what they hear

## Read first
[05-ui-ux.md](../project/05-ui-ux.md) · [04-mic-adaptation.md](../project/04-mic-adaptation.md) Layer C · `tine_dsp::curve`, `MacroMapper::explain` · Rune docs for drag (`on_drag`), canvas drawing, motion, focus/keys, tooltips

## Steps
- [x] **Header:**
  - mic dropdown (Rune native popup menu via `Tx::popup_menu` + `PopupMenu::picker`) and APO badge (`Active` green · `Not installed` with **Install** button · `Inactive` with **Repair** · `Error` with details)
  - archetype segmented pills and `Auto-Tune (8s)` button
  - Hear Myself toggle with latency badge (`~23 ms` budget; amber, red when over) and a device popover
  - A/B toggle (`Tine` / `Raw`)
- [x] **Keys:** `Space` toggles A/B when no text input or dropdown has focus (macro nodes own Space/arrows via `El::on_key`; focused buttons keep native Space-presses).
- [x] **Canvas:**
  - 32 spectrum bars (dim, smoothed)
  - response curve from `curve::response_curve` over 128 log-spaced points (computed only when params change; memoized on a `DspParams` hash + Rune `memo`)
  - glow spline
  - 0 dB reference line and frequency grid labels (100, 1k, 10k)
- [x] **Macro nodes:**
  - fixed x positions (Purity ~ 150 Hz zone, Body ~ 250 Hz, Presence ~ 4 kHz, Leveler at the right edge as a "level" node)
  - y = value
  - vertical drag (1 px = 0.004); arrows ±0.01, Shift ±0.1; double-click resets to the preset default
  - role `slider`, label `"Body 0.62"`, `focusable(true)`
- [x] **Tooltip:** on hover or drag, spring-animated (`El::anchored` above the node + `Transition::offset`/`opacity`), rows from `MacroMapper::explain` (`+2.5 dB @ 110 Hz`). Pinned params show as `pinned (Pro)`.
- [x] **Meters:** input and output vertical meters (peak + RMS, peak hold 1.5 s), clip LED, GR mini-bar for the compressor.
- [x] **Performance:**
  - view builds only what changed (Rune `memo` for grid + curve layers)
  - static layers (grid, labels) cached with `.cache()`
  - live layers (spectrum, meters) are the only per-frame work
  - the curve recomputes only on param change

## Acceptance criteria
- `golden_line` and `apo_not_installed` scenes pass every headless check at both sizes; a `golden_line_tooltip` scene (node hovered) has no overlap.
- Scripted headless interaction (`--script`): drag Body up 100 px → the intent log shows a monotonic `SetMacro(Body, …)` sequence, and the final tree label reads the new value.
- Keyboard-only run of the focus order passes (scripted).
- Frame build + encode < 4 ms at 1280×800 in the demo (Rune frame profile); demo CPU < 3 % at 60 Hz.

## Verification
```bash
cargo nextest run -p tine-ui
cargo run -p tine-ui --example ui_demo
```
Both pass locally (see Result).

## Result

All acceptance criteria met; evidence:

- **Headless scenes** (test `headless_scenes_render_and_pass_checks`): every scene in
  `SCENES` — including `golden_line`, `golden_line_tooltip` and `apo_not_installed` — renders
  at 1280×800 and 960×640, passes the element-tree invariants (no interactive-sibling overlap,
  nothing clipped by the window, `focusable` implies role + label, ≥ 4.5:1 contrast is asserted
  by `theme_contrast_meets_wcag_aa` + a `tine-ui` theme unit test), and byte-matches the
  blessed PNGs (re-blessed with `UPDATE_SNAPSHOTS=1`; 24 scene PNGs + 2 new tooltip PNGs).
  `golden_line` and `golden_line_tooltip` also assert the exact 13-step focus order from
  `05-ui-ux.md`.
- **Scripted drag** (`drag_body_up_100px_is_monotonic_and_updates_label`): 10 pointer steps
  up 100 px on `macro/body` emit ≥ 10 `SetMacro(Body, …)` intents, strictly monotonic
  (0.62 → 1.00), final element label `"Body 1.00"`. The `--script` CLI path is covered by
  `scripted_drag_updates_tree_label_via_cli` (script file `{"type":"drag","key":"macro/body",
  "dx":0,"dy":-100}` → tree JSON label `"Body 1.00"`).
- **Keyboard-only walk** (`keyboard_only_focus_order_walk`): Tab visits the documented order
  and wraps; Up = +0.01, Shift+Up = +0.1; Space with a node focused toggles A/B while focus
  stays on the node. `mic_selector_opens_native_dropdown_and_picks_endpoint` verifies the
  native mic dropdown (2 endpoints, pick → `SelectEndpoint(1)`),
  `double_click_resets_macro_to_preset_default` verifies reset on `macro/presence`.
- **Performance** (`headless_frame_build_and_encode_budget`, headless 120 frames @ 60 fps,
  1280×800): mean build+encode **0.479 ms** (dev build), max 17.3 ms (first frame);
  release `ui_demo` window profile: mean **0.528–0.636 ms** per drawn frame, ≈ 3.2 % of one
  core for build+encode at 60 Hz. Live-process CPU (release demo, window open, fake telemetry
  @ 60 Hz): **0.34 % of all cores** (16-core machine) over a 10 s window — under the 3 % budget
  in `09-engineering-standards.md` §7.
- **Curve memoization**: unit test `curve_db_memoizes_on_params_hash` proves exactly one DSP
  evaluation for repeated `curve_db` calls and one more only when a curve-relevant EQ param
  changes; `curve_hash_tracks_curve_relevant_params` proves non-curve params (e.g. gate
  release) don't invalidate it. Peak hold behavior proven by `peak_hold_holds_then_falls`.

Decisions taken (flagged for review):
- Plot x-axis is log-frequency 80 Hz–20 kHz (not 20 Hz): avoids the 80 Hz HPF response diving
  off the bottom of the plot while keeping the 100/1k/10k grid and the documented macro zones
  (~150 / ~250 / ~4 kHz).
- Peak hold lives in the view via a `thread_local` keyed to `AudioTelemetryFrame::sequence`
  (HOLD_FRAMES = 90 = 1.5 s @ 60 Hz): Rune views are pure functions with no clock, and keying
  the hold on the telemetry sequence keeps headless runs deterministic. Follow-up for T16/T20:
  the engine side can publish held peaks and this tracker can be dropped.
- Spectrum bars draw whatever telemetry gives (engine-side smoothing is the real path; the
  fake generator in `fake.rs` is already smooth).
- Rune consumes Space on any focused non-text control (native "Space presses the control").
  Macro nodes therefore own Space and arrows through `El::on_key` (Space toggles A/B, arrows
  nudge), and the window-level `app_spec.on_key` Space toggle covers the nothing-focused case.
  A focused header button keeps native Space behavior (standard on every desktop platform).
- The latency badge colors amber up to the ~23 ms budget (D-019) and red above it.
- Tooltip explanation rows call `MacroMapper::explain` with `cal: None`: `UiModel::calibration`
  (`CalibrationUi`) is a UI-side view model without the derived-parameter fields the protocol
  `Calibration` carries; wiring auto-tune results into tooltips belongs to T20/T21.

Files touched outside the nominal **Owns** row (integration glue over T18's scaffolding;
T20 depends on this task and owns the drawer/modal views, so no in-flight conflict):
`views/root.rs` (call the new header/canvas views; placeholders removed),
`src/lib.rs` (arrow handling moved into nodes; Space-only app handler exported),
`src/scenes.rs` (new `golden_line_tooltip` scene), `tests/headless.rs` (new checks),
`examples/ui_demo.rs` (frame-profile reporting), and the re-blessed snapshot PNGs for the
scenes that embed the header/canvas. No `tine-dsp`/`tine-protocol` files were touched.

## Requests
- None blocking. For the record (see Result): this task needed to edit shared `tine-ui`
  integration points beyond the nominal `Owns` row — `root.rs` wiring, `lib.rs` key routing,
  `scenes.rs`, `tests/headless.rs`, `examples/ui_demo.rs`, and the re-blessed snapshots of
  every scene embedding the header/canvas. These are T18-owned scaffolding whose task is done
  and were required to meet this task's acceptance criteria.
- Follow-up suggestions for other tasks (not blocking T19):
  - T16/T20: engine-published peak hold could replace the view-side `thread_local` tracker.
  - T20: pass the auto-tune `Calibration` into `macro_explanation_rows` once the wizard UI
    keeps the protocol `Calibration` around.
  - CI (T01 owner): the Rune `frame-profile` feature would give phase-level frame logs
    (`RUNE_FRAME_LOG`); enabling it is a root `Cargo.toml` change, so I measured via
    `App::on_frame_encoded` instead.
