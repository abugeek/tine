# T19 — Header + Golden Line Canvas

| Field | Value |
| :--- | :--- |
| Status | in-progress (agent: tine-worker) |
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
- [ ] **Header:**
  - mic dropdown (Rune listbox/popup) and APO badge (`Active` green · `Not installed` with **Install** button · `Inactive` with **Repair** · `Error` with details)
  - archetype segmented pills and `Auto-Tune (8s)` button
  - Hear Myself toggle with latency badge (`~23 ms`) and a device popover
  - A/B toggle (`Tine` / `Raw`)
- [ ] **Keys:** `Space` toggles A/B when no text input or dropdown has focus.
- [ ] **Canvas:**
  - 32 spectrum bars (dim, smoothed)
  - response curve from `curve::response_curve` over 128 log-spaced points (computed only when params change; memoized on a `DspParams` hash)
  - glow spline
  - 0 dB reference line and frequency grid labels (100, 1k, 10k)
- [ ] **Macro nodes:**
  - fixed x positions (Purity ~ 150 Hz zone, Body ~ 250 Hz, Presence ~ 4 kHz, Leveler at the right edge as a "level" node)
  - y = value
  - vertical drag (1 px = 0.004); arrows ±0.01, Shift ±0.1; double-click resets to the preset default
  - role `slider`, label `"Body 0.62"`, `focusable(true)`
- [ ] **Tooltip:** on hover or drag, spring-animated, rows from `MacroMapper::explain` (`+2.9 dB @ 110 Hz`). Pinned params show as `pinned (Pro)`.
- [ ] **Meters:** input and output vertical meters (peak + RMS, peak hold 1.5 s), clip LED, GR mini-bar for the compressor.
- [ ] **Performance:**
  - view builds only what changed (Rune memo/cache)
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
