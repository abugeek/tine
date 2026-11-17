# 05 — UI / UX

Built with [Rune](https://github.com/anomalyco/rune) (MIT, pre-release, pinned to a `dev` revision). Views are pure
functions `fn view(s: &AppState, cx: &mut Cx) -> El`. Every view has a headless scene that renders a PNG and dumps the
element tree for automated checks.

## Window lifecycle

- Opened from tray (left-click or "Open Tine"), on first run, or by launching `tine.exe` while it is already running (single-instance; second launch signals the first via `\\.\pipe\tine-<user-sid>`).
- Per **D-017**, the parent `tine.exe` hosts the tray (`rune::Tray`), control plane, and virtual-cable audio router; opening the window spawns `tine.exe --ui` as a short-lived child process, and closing the window exits `tine.exe --ui` so all GPU/DX12 memory is released.
- Tray (`rune::Tray`; flat menu, so checked state is shown in the label): Open Tine · ✓ Processing / Bypassed · ✓ Hear Myself · — · Dynamic · Condenser · Headset · Laptop (current marked ✓) · — · Start with Windows ✓ · Quit. Left-click opens the window.

## Layout

```text
┌ Header ───────────────────────────────────────────────────────────────────────────────┐
│ [Mic: Shure SM7B ▾]  [● Active]   (Dynamic|Condenser|Headset|Laptop)  [Auto-Tune (8s)]│
│                                         [🎧 Hear Myself ⚠ latency]   [A/B: Tine ⇄ Raw] │
├ Golden Line canvas ──────────────────────────────────────────────────────────────────┤
│   32-band live spectrum (bars, dim)                                                   │
│   ~~~~~~~~ glowing response spline ~~~~●Purity~~~~●Body~~~~~●Presence~~~~●Leveler~~~~ │
│   tooltip on hover/drag: "Body 0.62 → +2.9 dB @110 Hz, −3.1 dB @320 Hz"               │
│   In/Out meters (right edge)                                                          │
├ [Pro Controls ▾] ─────────────────────────────────────────────────────────────────────┤
│  Drawer: one row per stage (1–6) with sliders + numeric fields + enable toggles,     │
│  GR meters for Gate, De-esser, Compressor. Pinned params show a pin.                  │
└──────────────────────────────────────────────────────────────────────────────────────┘
```

Note: the header has **no output-target selector** — under **D-018**, `tine-engine` automatically routes processed audio to the virtual audio cable (`CABLE Input`). It shows the routing status badge for the selected mic instead: `Active` · `Cable missing` · `Inactive` · `Error`.
The "Hear Myself" device picker lives in a small settings popover (in v0.1, "Hear Myself" monitors the microphone *input* signal per D-029).

## Interaction

| Input | Action |
| :--- | :--- |
| Drag macro node vertically | Change macro 0…1 (horizontal position fixed per node) |
| Arrow keys on focused node | ±0.01 (Shift: ±0.1) |
| `Space` (window focused, not in a text field) | Toggle A/B bypass |
| `Tab` order | Mic selector → archetype pills → Auto-Tune → Hear Myself → A/B → Purity → Body → Presence → Leveler → Pro toggle → drawer controls |
| Double-click node | Reset to archetype default |

Hover/drag tooltips use Rune spring motion. Meters and spectrum update at 60 Hz from telemetry; when the window is
unfocused, redraw drops to 30 Hz; when minimized, 0.

## Auto-Tune overlay

Modal sheet over the canvas: step indicator (Silence 3 s → Speak 5 s → Results), live level meter, a suggested sentence
to read, countdown ring, and a results card listing every parameter it changed (before → after) with **Apply** / **Discard**.

## Visual tokens

Dark theme only for v0.1. Tokens defined once in `tine-ui::theme` (background, surface, accent glow, text primary/secondary,
meter green/amber/red). Contrast: text ≥ 4.5:1 against its surface, verified in headless tests.

## Headless verification (required for every UI change)

Scenes: `scene_golden_line`, `scene_pro_drawer_open`, `scene_auto_tune_wizard`, plus `scene_apo_not_installed`.
For each: render PNG at 1280×800 and 960×640, dump element tree JSON, then assert:
1. No two sibling interactive elements have overlapping bounds; nothing is clipped by its parent.
2. All macro nodes, toggles and pills are `focusable(true)` with labels/roles; focus order matches the table above.
3. Text/surface contrast ≥ 4.5:1 using theme tokens.
Golden PNGs live in `crates/tine-ui/tests/snapshots/` and are compared with a small per-pixel tolerance.
