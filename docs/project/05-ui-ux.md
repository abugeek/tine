# 05 — UI / UX

Built with [Rune](https://github.com/anomalyco/rune) (MIT, pre-release, pinned to a `dev` revision). Views are pure
functions `fn view(s: &AppState, cx: &mut Cx) -> El`. Every view has a headless scene that renders a PNG and dumps the
element tree for automated checks.

## Window lifecycle

- Opened from tray (left-click or "Open Tine"), on first run, or by launching `tine.exe` while it is already running (single-instance; second launch signals the first via `\\.\pipe\tine-<user-sid>`).
- Per **D-017**, the parent `tine.exe` hosts the tray (`rune::Tray`), control plane, and virtual-cable audio router; opening the window spawns `tine.exe --ui` as a short-lived child process, and closing the window exits `tine.exe --ui` so all GPU/DX12 memory is released.
- Tray (`rune::Tray`; flat menu, so checked state is shown in the label): Open Tine · ✓ Processing / Bypassed · ✓ Hear Myself · — · Dynamic · Condenser · Headset · Laptop (current marked ✓) · — · Start with Windows ✓ · Quit. Left-click opens the window.

## Layout ("Tine Deck 2.3" DJ / Broadcast Mixer Console)

```text
┌ Header ──────────────────────────────────────────────────────────────────────────────────────┐
│ [◉ Tine DECK 2.3] │ [🎙 Shure SM7B ▾]         (Dynamic|Condenser|Headset|Laptop) [✦ Auto-Tune]│
│ [● Cable: Ready] [RNNoise ▾] [Mic Check] [☾ Dark / ☀ Light]   [○ Hear Myself] [Mute] [⏻ TINE]│
├ Mixer Console (6 Channel Strips) ────────────────────────────────────────────────────────────┤
│ USB/MIC   │ CUE · GATE   │ CUE · HPF    │ DE-ESS OFF   │ CUE · COMP   │ TINE-750MK2  IN  OUT │
│ [LEVEL ◯] │ [PURITY ◯]   │ [BODY ◯]     │ [PRESENCE ◯] │ [LEVELER ◯]  │ ┌─────────────┐ ┌──┐ │
│ [HPF   ◯] │ [DENOISE ○]  │ [LOW ○]      │ [HI/AIR ○]   │ [THRESH ○]   │ │ Live EQ &   │ │VU│ │
│ VOICE FX: │ [GATE    ○]  │ [MUD ○]      │ [CLARITY ○]  │ [CEILING ○]  │ │ 32-band FFT │ │  │ │
│ Broadcast │ GATE GR ───  │ HPF 80 Hz    │ DE-ESS GR ── │ COMP GR ───  │ └─────────────┘ └──┘ │
│ Natural   │              │              │              │              │ [DARK⇄LIGHT] [MUTE⇄H]│
│ Crisp     │              │              │              │              │ [◀] Dynamic [▶]      │
├ [Pro Controls ▾] ────────────────────────────────────────────────────────────────────────────┤
│  Drawer: one row per stage (1–6) with sliders + numeric fields + enable toggles,             │
│  GR meters for Gate, De-esser, Compressor. Pinned params show a pin.                         │
└──────────────────────────────────────────────────────────────────────────────────────────────┘
```

Note: the header has **no output-target selector** — under **D-018**, `tine-engine` automatically routes processed audio to the virtual audio cable (`CABLE Input`). It shows the routing status badge for the selected mic instead: `Active` · `Cable missing` · `Inactive` · `Error`.
The "Hear Myself" device picker lives in a small settings popover (in v0.1, "Hear Myself" monitors the microphone *input* signal per D-029).

## Interaction

| Input | Action |
| :--- | :--- |
| Drag any rotary dial vertically | Adjust macro (`0…1`) or linked Pro parameter (sub-knobs spin live with macros) |
| Click `CUE` stage pad | Enable/disable Gate, HPF, De-Esser, or Compressor directly |
| Click `VOICE FX` / `VOICE PROFILE` | Apply Broadcast/Natural/Crisp/Neutral macro scenes or step archetypes |
| Click `Dark` / `Light` or `CONSOLE` slide switch | Switch live between Decky Midnight (Dark) and Decky Daylight (Light) |
| Arrow keys on focused macro dial | ±0.01 (Shift: ±0.1) |
| `Space` (window focused, not in a text field) | Toggle A/B bypass |
| `Tab` order | Mic selector → archetype pills → Auto-Tune → Hear Myself → A/B → Purity → Body → Presence → Leveler → Pro toggle → drawer controls |
| Double-click rotary dial | Reset to archetype/spec default |

Hover/drag tooltips and rotary caps use Rune spring motion (`Spring::SNAPPY` rotation, `Spring::BOUNCY` hover scale, `press_scale` tactile keycaps). Meters and spectrum update at 60 Hz from telemetry; when the window is
unfocused, redraw drops to 30 Hz; when minimized, 0.

## Auto-Tune overlay

Modal sheet over the canvas: step indicator (Silence 3 s → Speak 5 s → Results), live level meter, a suggested sentence
to read, countdown ring, and a results card listing every parameter it changed (before → after) with **Apply** / **Discard**.

## Visual tokens & Light / Dark Themes

Dual theme system ("Decky Midnight" Dark Mode & "Decky Daylight" Light Mode), switched live via `ThemeMode` in `tine-ui::theme`. Shared hardware controls
(porcelain-white rotary dials with 11-dot radial LED collars and orbiting cobalt-blue rotors, `CUE` pads, slide switches, segmented controls, status chips, the brand mark, the line icons) live in `tine-ui::views::kit`.

| Token | Role |
| :--- | :--- |
| `bg`, `surface`, `surface_raised`, `surface_hover`, `well` | Chassis, faceplate, raised keycaps, hover tint, recessed OLED/meter wells |
| `text`, `text_dim` | High-contrast primary foreground and muted secondary labels (≥ 4.5:1) |
| `accent`, `accent_hi`, `accent_lo`, `accent_soft`, `accent_glow`, `accent_text` | Tactile cue orange (`CUE` active pads, primary CTA, EQ curve, compressor GR) |
| `knob_outer`, `knob_inner`, `knob_ring_active`, `knob_dot`, `knob_tick_*` | Porcelain-white rotary caps, powder-blue wells, radial LED tick collars |
| `deck_blue`, `deck_blue_soft`, `deck_blue_text` | Electric cobalt blue for active FX pills, switch thumbs, and rotary arcs |
| `spectrum`, `meter_*`, `success`, `danger` | Spectrum bars; semantic meter and status colors |

Contrast: text ≥ 4.5:1 against its surface in both Dark and Light modes, verified in headless tests.

## Brand assets

`assets/brand/` holds the icon set, rendered by `python -I scripts/brand/render_icons.py` (Pillow + NumPy): the SVG
master, PNGs 16–1024 px, `tine.ico` (linked into `tine.exe` by `crates/tine-app/build.rs`, used for the window and
taskbar and by the MSI) and the per-state tray glyphs (`tray-{active,bypassed,error}-32.rgba`).

## Headless verification (required for every UI change)

Scenes: `scene_golden_line`, `scene_pro_drawer_open`, `scene_auto_tune_wizard`, plus `scene_apo_not_installed`.
For each: render PNG at 1280×800 and 960×640, dump element tree JSON, then assert:
1. No two sibling interactive elements have overlapping bounds; nothing is clipped by its parent.
2. All macro nodes, toggles and pills are `focusable(true)` with labels/roles; focus order matches the table above.
3. Text/surface contrast ≥ 4.5:1 using theme tokens.
Golden PNGs live in `crates/tine-ui/tests/snapshots/` and are compared with a small per-pixel tolerance.
