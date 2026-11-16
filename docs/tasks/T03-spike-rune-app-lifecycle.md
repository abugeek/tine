# T03 — Spike: Rune Windowless App, Tray, GPU Release, Memory

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | C — Engine / App |
| Depends on | T01 |
| Unblocks | T12, T17, T18 |
| Owns | `spikes/rune-spike/**` (throwaway) |
| Size | M |

## Goal
Measure that D-002 (one process, on-demand window) meets the budgets with Rune's built-in lifecycle:

```rust
App::new(spec).windowless().keep_running().release_gpu_when_closed()
```

The app also uses `rune::Tray` and an `AppHandle` fed from a background thread. Also confirm the headless tooling works in CI, and fix the pattern T17/T18 will copy.

## Read first
- Rune `crates/rune/README.md` at the pinned rev: "App lifetime", "Compact tray-controlled windows", "App hooks", "Agent tooling".
- Rune `examples/background`, `examples/hello`, `crates/rune/src/tray.rs`.
- [05-ui-ux.md](../project/05-ui-ux.md) · [09-engineering-standards.md](../project/09-engineering-standards.md) §7.

## Steps
- [ ] `rune-spike` bin: windowless app with `release_gpu_when_closed()`; `rune::Tray` with flat menu (Open, Bypass ✓/✗ via label text, Quit); `TrayEvent::Activate` → `handle.open()`; a background thread calls `handle.update` at 60 Hz with fake meter data **only while** `handle.is_visible()`.
- [ ] Simple view: a few text rows + one animated bar driven by the fake meter.
- [ ] Single instance: named mutex `Local\Tine.SingleInstance` + named pipe `\\.\pipe\tine-<user-sid>`; a second launch sends `open` and exits.
- [ ] Measure (record in `## Result`, Release build without `tooling`):
  - cold start → tray visible (ms)
  - first window open (ms) and reopen (ms)
  - with the window closed, after 5 min: Private Bytes, Working Set, GPU dedicated/shared memory (PerfMon "GPU Process Memory"), CPU %, thread count, handle count
  - 50 open/close cycles: growth in each of the above
  - window open at 60 Hz: CPU % and frame time (`on_frame_drawn`)
  - `tine.exe` binary size with and without `tooling`
- [ ] Headless: run `--snap out.png` and `--tree tree.json` with the `tooling` feature in CI (`windows-latest`; record which adapter is used — WARP is acceptable).
- [ ] Find out how headless scenes are declared (`--list-scenes`; how an app registers named scenes with fixed state) and write the recipe for T18.

## Acceptance criteria
- Window closed: GPU process memory ≈ 0; CPU < 0.5 %; RSS recorded against the 8 MB target (update R-09 with real numbers and a realistic target).
- 50 cycles: no monotonic growth (> 2 MB RSS, any threads/handles).
- Tray works with the window closed; menu events reach the app thread.
- Headless PNG + tree JSON produced in CI.
- Recipes for T17 (app shell) and T18 (scenes) written in `## Result`.
- If a budget can't be met in-process: measure the `tine.exe --ui` child-process alternative and record a decision.

## Verification
```bash
cargo run -p rune-spike --release
cargo run -p rune-spike --features tooling -- --snap target/spike.png
cargo run -p rune-spike --features tooling -- --tree target/spike-tree.json
```

## Out of scope
Real UI, engine integration.

## Pitfalls
- `rune::Tray` runs on its own thread: in its callback, only forward to `AppHandle` and do no work.
- `rune::Tray` menus are flat (Action and Separator only). Show checked state in the label text, and the archetype choice as four flat items.
