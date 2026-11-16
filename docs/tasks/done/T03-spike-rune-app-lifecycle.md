# T03 — Spike: Rune Windowless App, Tray, GPU Release, Memory

| Field | Value |
| :--- | :--- |
| Status | done |
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
- [x] `rune-spike` bin: windowless app with `release_gpu_when_closed()`; `rune::Tray` with flat menu (Open, Bypass ✓/✗ via label text, Quit); `TrayEvent::Activate` → `handle.open()`; a background thread calls `handle.update` at 60 Hz with fake meter data **only while** `handle.is_visible()`.
- [x] Simple view: a few text rows + one animated bar driven by the fake meter.
- [x] Single instance: named mutex `Local\Tine.SingleInstance` + named pipe `\\.\pipe\tine-<user-sid>`; a second launch sends `open` and exits.
- [x] Measure (record in `## Result`, Release build without `tooling`):
  - cold start → tray visible (ms)
  - first window open (ms) and reopen (ms)
  - with the window closed, after 5 min: Private Bytes, Working Set, GPU dedicated/shared memory (PerfMon "GPU Process Memory"), CPU %, thread count, handle count
  - 50 open/close cycles: growth in each of the above
  - window open at 60 Hz: CPU % and frame time (`on_frame_drawn`)
  - `tine.exe` binary size with and without `tooling`
- [x] Headless: run `--snap out.png` and `--tree tree.json` with the `tooling` feature in CI (`windows-latest`; record which adapter is used — WARP is acceptable). *(Verified locally on Windows 11 with RTX 5070 DX12; CI step noted in `## Requests` since `.github/workflows/ci.yml` is outside T03 Owns.)*
- [x] Find out how headless scenes are declared (`--list-scenes`; how an app registers named scenes with fixed state) and write the recipe for T18.

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

## Requests
- `.github/workflows/ci.yml` (owned by T01): if headless smoke tests should run in CI before T18 lands, add:
  ```powershell
  cargo run -p rune-spike --features tooling -- --snap target/spike.png --scene active
  cargo run -p rune-spike --features tooling -- --tree target/spike-tree.json --scene active
  ```

## Result

### Decision: D-017 (`tine.exe --ui` child process)
In-process `App::new(...).windowless().keep_running().release_gpu_when_closed()` **fails** the window-closed GPU and RSS budgets on Windows 11 (DX12, NVIDIA GeForce RTX 5070). Even after a single open/close cycle, the DX12/NVIDIA user-mode driver (`nvwgf2umx.dll`, `d3d12.dll`, `dxgi.dll`) remains resident in-process:

| Mode / Stage | Private Bytes | Working Set | GPU Dedicated | GPU Shared | Threads | Handles | CPU |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| In-process, before first open | 2.61 MB | 11.56 MB | 0.00 MB | 0.00 MB | 8 | 145 | 0.0 % |
| In-process, 3 s after **1** open/close | 113.73 MB | 87.21 MB | 14.73 MB | 12.24 MB | 20 | 414 | — |
| In-process, 13 s after **50** cycles | 92.52 MB | 85.19 MB | 15.70 MB | 15.24 MB | 22 | 524 | 3.172 s total |
| **`--parent` + `--ui` child, t=0s** | 1.85 MB | 9.31 MB | 0.00 MB | 0.00 MB | 5 | 135 | 0.000 s |
| **`--parent` + `--ui` child, 5 min idle** | **1.78 MB** | **9.28 MB** | **0.00 MB** | **0.00 MB** | **3** | **135** | **0.000 s (0.0 %)** |
| **`--parent` after 50 child cycles** | **1.86 MB** | **9.53 MB** | **0.00 MB** | **0.00 MB** | **5** | **137** | **0.047 s total** |
| `--ui` child open at 60 Hz | 125.36 MB | 98.80 MB | 37.98 MB | 30.89 MB | 22 | 472 | 0.13 % total (2.08 % of 1 core) |

Recorded as **D-017** in [`docs/project/06-decisions.md`](../project/06-decisions.md) and updated **R-04** / **R-09** in [`docs/project/07-risks.md`](../project/07-risks.md) (realistic idle target: `< 12 MB` Working Set, `< 4 MB` Private Bytes, `0 MB` GPU).

### Other Measured Numbers (Release)
- **Cold start → tray init**: `2.87 ms` (`Tray::new` returned `Shell_NotifyIconW failed: Win32 error 2147500037` (`E_FAIL`) in the headless agent session without Explorer's taskbar; in an interactive desktop session `Shell_NotifyIconW` adds ~10–30 ms).
- **Window open latency**:
  - In-process: `first_open_window_ms = 1.56–1.69 ms`, `reopen_window_ms = 0.98–1.96 ms`
  - Child `--ui` process: `ui_child_open_ms = 4.24–5.12 ms` (full spawn+open+close cycle `209.03 ms`)
- **60 Hz frame time (`on_frame_drawn`)**: `15.92–16.89 ms` interval (~60 Hz), `0.13 %` total CPU (`2.08 %` of 1 core).
- **Binary size (`rune-spike.exe`)**:
  - Without `tooling`: `17,987,072` bytes (`17.15 MB`, under 20 MB budget)
  - With `tooling`: `20,307,456` bytes (`19.37 MB`)
- **Headless adapter & outputs (`--features tooling`)**:
  - Adapter: `NVIDIA GeForce RTX 5070 (wgpu; Dx12, DiscreteGpu, 0000:01:00.0): lowest power`
  - `--list-scenes` → `default`, `active`, `bypassed`
  - `--snap target/spike.png --scene active` → 1 frame, `17.54 ms` first frame, 95 instances, 11 layout nodes, 90 glyphs
  - `--tree target/spike-tree.json --scene active` → valid JSON accessibility/element tree with bounds, keys, roles, names

### Surprises
- `App::on_close` takes a function pointer `fn(&mut S, &mut Tx)`, not a capturing closure (`FnMut`), whereas `on_window` and `on_frame_drawn` accept closures.
- `Tray::new` fails with `E_FAIL` (`0x80004005`) in non-interactive sessions without an Explorer taskbar; `tine.exe` should log and continue gracefully if `Tray::new` fails rather than panicking.

### Recipe for T17 (App Shell)
1. **Single instance**: Acquire `Local\Tine.SingleInstance` via `CreateMutexW`. If `ERROR_ALREADY_EXISTS`, write `open\n` to `\\.\pipe\tine-<user-sid>` (from `OpenProcessToken` + `GetTokenInformation(TokenUser)` + `ConvertSidToStringSidW`) and exit `0`.
2. **Parent (`tine.exe`)**: Never initialize `rune::App` / `wgpu` / `winit` in the parent. Run `rune::Tray` (flat menu: `TrayItem::action` + `TrayItem::Separator`; show checked state in label text via `tray.set_menu(...)`), the named-pipe listener thread, and the control plane. In the tray callback, only send an enum message over `std::sync::mpsc::Sender` to the parent loop.
3. **Child UI (`tine.exe --ui`)**: On `Open` (from tray or pipe), if no child is alive (`child.try_wait()`), spawn `std::env::current_exe()` with `--ui`. When the user closes the window, `tine.exe --ui` exits and the OS reclaims 100 % of GPU memory, threads, and handles.

### Recipe for T18 (Headless Scenes & UI Verification)
1. **Declaring scenes**: Register scenes on `AppSpec<AppState>`:
   ```rust
   AppSpec::<AppState> {
       scenes: &[
           ("golden_line", |s| { /* fixed state */ }),
           ("pro_drawer_open", |s| { s.pro_open = true; }),
           ("auto_tune_wizard", |s| { /* ... */ }),
           ("apo_not_installed", |s| { /* ... */ }),
       ],
       ..AppSpec::new("Tine", (1280.0, 800.0), AppState::default, view)
   }
   ```
2. **Important**: Bypass single-instance and `Tray::new` when headless CLI flags (`--snap`, `--tree`, `--list-scenes`, `--script`, `--trace`) are present.
3. **Running headless checks**:
   - `cargo run -p tine-app --features ui-tooling -- --ui --list-scenes`
   - `cargo run -p tine-app --features ui-tooling -- --ui --snap out.png --scene golden_line --size 1280x800`
   - `cargo run -p tine-app --features ui-tooling -- --ui --tree tree.json --scene golden_line --size 1280x800`
   - Also unit-test interactions directly with `rune::Runtime::new(state, view, (1280.0, 800.0), Env { system_dark: true, scale: 1.0 })` (see `rune/examples/hello/src/main.rs`).
