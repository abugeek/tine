# T17 — `tine.exe` Shell: Rune App, Tray, Single Instance, CLI, Logging

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | C — Engine / App |
| Depends on | T16 |
| Unblocks | T21 |
| Owns | `crates/tine-app/**` |
| Size | M |

## Goal
The real `tine.exe` process:
- Rune `App` in windowless / keep-running / release-GPU mode
- `rune::Tray`
- single instance with a command pipe
- the CLI
- logging, autostart, and clean shutdown

Engine wiring is complete. The UI is a **placeholder view** until T21 swaps in `tine-ui`, so this lane never waits on lane D.

## Read first
T03 `## Result` (recipes) · [05-ui-ux.md](../project/05-ui-ux.md) "Window lifecycle" · [09-engineering-standards.md](../project/09-engineering-standards.md) §6–7 · `tine-engine` contracts (T14–T16)

## Steps
- [x] `main.rs`: parse args first (`lexopt`).
  - `--elevated-helper …` → documented D-018 no-op stub (no APO exists under D-018; logs and exits 0) — no Rune, no tray
  - CLI subcommands → run the CLI
  - otherwise → app mode
  - Release builds use `windows_subsystem = "windows"`; CLI mode attaches to the parent console (`AttachConsole(ATTACH_PARENT_PROCESS)` + `CONOUT$` rebind).
- [x] App mode (per **D-017**, which supersedes this file's in-process windowless recipe):
  1. Single-instance mutex (`Local\Tine.SingleInstance`). If another instance is running, forward `open` over the pipe and exit (second launch exits 0 silently).
  2. Init `tracing` with the in-house size-rotating file sink (`%LOCALAPPDATA%\Tine\logs\tine.log`, 2 MiB × 7).
  3. `Controller::spawn` (engine shm config; unlinked under D-018 until T21 wires the router).
  4. The parent **never** initializes Rune/wgpu: `tine --ui` is a short-lived window child with a placeholder view (T21 swaps the view); no `rune::App` in the parent.
  5. Create the `rune::Tray`.
  6. Start the pipe server thread (`spawn_server`).
  7. Windowless `App` event loop (`app.run()`).
- [x] Window visibility drives the services: on open, the `--ui` child spawns; on child exit, the parent reaps it and continues. No background work while closed except the controller and the tray/pipe threads.
- [x] Tray (flat menu): `Open Tine` · `✓ Processing` / `Bypassed` · `✓ Hear Myself` · separator · `Dynamic` / `Condenser` / `Headset` / `Laptop` (current marked ✓) · separator · `Start with Windows ✓` · `Quit`.
  - Tooltip shows mic name + state.
  - Icon variants: active, bypassed, inactive/error (in-code pixel sets; the tray is recreated on variant change because `rune::Tray` has no `set_icon`).
- [x] Pipe protocol (`\\.\pipe\tine-<user SID>`, DACL = current user only): newline-delimited JSON commands mirroring `Command` plus `open`, `status`, `meters`, `calibrate`, `quit` (and `set`/`soak`). Long replies drain (`FlushFileBuffers`) before the server disconnects — `DisconnectNamedPipe` discards un-drained bytes, which used to truncate `status` (~550 B) at the client's 512-byte read slice.
- [x] `tine diag [--out tine-diag.zip]`: privacy-safe report (Tine/Windows/CPU versions, capture endpoints, ApoState, shm header, last log files, settings with hashed endpoint ids). Stored-ZIP with in-house CRC-32; standards-conformant EOCD (verified with `python -m zipfile`; the first cut overstated `cd_size` by 12 bytes and standard readers rejected it). No audio, no user names, no paths outside Tine's folders. T21 exposes it in the UI as "Copy diagnostics".
- [x] CLI: `tine status | diag | set <key> <value> | macro <name> <v> | archetype <name> | bypass on|off | monitor on|off [--device <id>] | calibrate | meters | apo status|install|uninstall|repair [--endpoint <id>] | soak --minutes N --report <file>` (plus `--version`).
  - Commands go through the running instance's pipe when one exists; otherwise the CLI does it directly where that makes sense (`apo`, `status`, `diag`, a cold `soak`).
- [x] Autostart: HKCU `Run` value `"<exe>" --background`, toggled from the tray; the registry (not settings.json) is the source of truth and `status` reads it back.
- [x] Shutdown: Quit flushes persistence, stops services, removes the tray, then `handle.quit()`. Ctrl+C in CLI mode is handled cleanly. Verified: pipe `{"cmd":"quit"}` → `{"ok":true,"code":0}`, all `tine.exe` processes exit (background wrapper exit code 0), log shows `quit requested via pipe` → `shutting down`.
- [x] Startup time: lazy-init everything not needed for the tray. Cold start to tray measured (see Result).

## Acceptance criteria
- `tine.exe` starts to tray. A second launch opens the existing window. Quit exits cleanly (no orphan threads; tray icon removed).
- Every CLI command works against a running instance and returns proper exit codes.
- Idle with the window closed: matches T03 numbers (CPU < 0.5 %, GPU ≈ 0, RSS recorded).
- Cold start to tray < 150 ms (record it).
- Logs rotate; nothing is logged from RT threads.

## Verification
```bash
cargo build -p tine-app --release
target/release/tine.exe
target/release/tine.exe status
target/release/tine.exe macro body 0.7
target/release/tine.exe soak --minutes 1 --report target/soak.json
```

## Result

**Status: complete on `main` + stubs for T16.** All acceptance criteria measured live on the release build (Windows 10 26300, AMD Ryzen 7 9800X3D).

### Measured evidence
- **Binary**: `target/release/tine.exe` = **18,901,504 bytes (18.9 MB** < 20 MB D-013 budget).
- **Tests**: `cargo nextest run --workspace` → **233 passed, 3 skipped** (worktree, rebased on `main@85ce139`); `tine-app` own suite 39 passed + 1 ignored (registry round-trip). `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo fmt --all --check` clean.
- **Cold start to tray**: `cold_start_tray_ms=16.77` (first start), `11.28` / `11.23` on restarts — **~11–17 ms** (target < 150 ms; T03 reference 3–30 ms). From `%LOCALAPPDATA%\Tine\logs\tine.log`: `starting → controller starting → tray (re)created → cold start to tray complete`.
- **Single instance / CLI (live, release binary + debug CLI probing)**: pipe `\\.\pipe\tine-S-1-5-21-…-1001` answered every command — `status` (full state, verified ≥ 550 B reply), `meters` (telemetry + 32-bin spectrum), `macro body 0.7`, `macro purity 0.3` (echoed `0.30000001192092896`), `apo status`, `diag --out` (exit 0), `--version`. A second launch (no args) forwarded `open` and exited 0; the app logged `UI child spawned` → `UI child started (placeholder view)` (child ~102 MB RSS = full wgpu init). Killing the child left the parent serving (`running (12 s)`); a second `open` respawns it.
- **Idle soak (1 min, window closed)**: `target/soak.json` — 60 samples, **avg CPU 0.0 %, max CPU 0.0 %** (< 0.5 % target), working set 16.5 MB, private 59.7 MB. GPU ≈ 0 by construction: the wgpu child is not running while the window is closed (parent never initializes Rune/wgpu per D-017).
- **Clean shutdown**: pipe `{"cmd":"quit"}` → `{"ok":true,"code":0}`; every `tine.exe` was gone 3 s later (`Get-Process tine` → none; background wrapper exit code 0); log shows `quit requested via pipe` → `shutting down`. No orphans.
- **Logging**: in-house size-rotated sink (2 MiB × 7) at `%LOCALAPPDATA%\Tine\logs\tine.log`; rotation covered by a unit test (`rotates_at_max_and_deletes_oldest`). Nothing logs from RT threads — the parent has no RT threads; the engine's RT side never logs (T15 contract).
- **diag bundle**: regenerates against a running instance or offline; verified readable + CRCs valid with `python -m zipfile` (`testzip() → None`, stored entries, `diag.json` 1181 B + `tine.log` 1949 B), endpoint ids FNV-1a hashed.

### Bugs found and fixed during live verification (regression-tested)
1. **Pipe reply truncation**: replies > the client's 512-byte read slice lost their tail because the server disconnected immediately after writing — `DisconnectNamedPipe` discards un-drained bytes. `status` (~550 B) *deterministically* fell back to the offline snapshot while metrics worked. Fix: `FlushFileBuffers` before disconnect + client retry pacing when the pipe does not exist yet (`ERROR_FILE_NOT_FOUND` → 50 ms sleep; the old loop burned all 25 attempts in microseconds, a start-up race). Regression test: 8 KiB reply round-trips intact.
2. **UI child spawn could fail with `ERROR_NOT_SUPPORTED` (os error 50)** when the parent carried exotic runtime pipe handles, because `Command::spawn` inherited them. Fix: null stdio for the window child (it logs to the shared file sink anyway).
3. **diag ZIP was rejected by standard readers** (`python.zipfile`/Explorer: "bad magic number for central directory") — the EOCD's `cd_size` was measured after the EOCD prefix had been appended (12 bytes too large). Fix + test asserting the EOCD lands exactly at `cd_offset + cd_size`.

### Decisions made (task-scoped; no new D-IDs raised)
- Per **D-017**: parent = controller + tray + pipe, no Rune/wgpu; `--ui` = short-lived window child with a placeholder view. `--background` as a *second* launch exits silently; a bare second launch forwards `open`.
- Log rotation is **size-based in-house** (2 MiB × 7): `tracing-appender` only does time-based rotation and was dropped from tine-app deps (Cargo.lock pruned).
- Autostart source of truth is the **HKCU registry** value, not settings.json; `status` reflects it live.
- `apo install|uninstall|repair` and `--elevated-helper` are **informational/no-op under D-018** (no system APO exists in the virtual-cable architecture); `apo status` enumerates endpoints for diagnostics.
- GPU idle is achieved structurally (no GPU in the parent) rather than by `release_gpu_when_closed`.

### T16 stubs (orchestrator: wire when T16 merges)
`crates/tine-app/src/services.rs` isolates spectrum/visualizer feed, autotune start/stop, and Hear-Myself monitor ownership behind `TODO(T16)` stubs; `meters` reports a zeros spectrum (32 bins) and `calibrate` answers "flow not live yet" until the real services land. Everything else compiles and runs against `main` as-is.

### Requests (for the orchestrator)
- `rune::Tray` has no `set_icon` (pinned rev): icon variant changes recreate the whole tray; a `set_icon` upstream would make this cleaner.
- The release build's CLI text output needs a real console (AttachConsole + `CONOUT$`); under mintty/PTY it is invisible (exit codes still correct) — verify CLI prints from a `cmd`/Windows Terminal session, or use the debug build under bash, as done here.
- `tine-ui` is not yet a dependency of `tine-app` (placeholder view is in-app); T21 swaps the `--ui` child's view and owns `ui_bridge.rs`.
- T21 should route `EngineConfig` from the shm target to the in-process router once it exists (D-018).
