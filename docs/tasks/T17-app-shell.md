# T17 — `tine.exe` Shell: Rune App, Tray, Single Instance, CLI, Logging

| Field | Value |
| :--- | :--- |
| Status | todo |
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
- [ ] `main.rs`: parse args first (`lexopt`).
  - `--elevated-helper …` → `apo_admin::run_elevated_helper` and exit (no Rune, no tray)
  - CLI subcommands → run the CLI
  - otherwise → app mode
  - Release builds use `windows_subsystem = "windows"`; CLI mode attaches to the parent console (`AttachConsole(ATTACH_PARENT_PROCESS)`).
- [ ] App mode:
  1. Single-instance mutex. If another instance is running, forward `open` over the pipe and exit.
  2. Init `tracing` with a rolling file appender.
  3. `Controller::spawn`.
  4. Build `rune::App::new(spec).windowless().keep_running().release_gpu_when_closed()`.
  5. Create the `rune::Tray`.
  6. Start the pipe server thread.
  7. `app.run()`.
- [ ] Window visibility drives the services: on open, start `Visualizer`; on close, stop it. No background work while closed except the controller (blocked) and the tray thread.
- [ ] Tray (flat menu): `Open Tine` · `✓ Processing` / `Bypassed` · `✓ Hear Myself` · separator · `Dynamic` / `Condenser` / `Headset` / `Laptop` (current marked ✓) · separator · `Start with Windows ✓` · `Quit`.
  - Tooltip shows mic name + state.
  - Icon variants: active, bypassed, inactive/error (16/20/24/32 px RGBA, generated from SVG at build time or embedded PNGs).
- [ ] Pipe protocol (`\\.\pipe\tine-<user SID>`, DACL = current user only): newline-delimited JSON commands mirroring `Command` plus `open`, `status`, `meters`, `calibrate`, `quit`.
- [ ] `tine diag [--out tine-diag.zip]`: a privacy-safe report for bug reports and the community beta (D-016). It contains:
  - Tine/Windows version, CPU model
  - capture endpoints (name, form factor, shared-mode format, enhancements flag, FxProperties APO CLSIDs, vendor APO names)
  - `ApoState` per endpoint, shm header (heartbeat age, flags, counters)
  - the last 3 log files
  - settings with endpoint ids hashed

  It contains **no audio**, no user names and no paths outside Tine's folders. T21 exposes it in the UI as "Copy diagnostics".
- [ ] CLI: `tine status | diag | set <key> <value> | macro <name> <v> | archetype <name> | bypass on|off | monitor on|off [--device <id>] | calibrate | meters | apo status|install|uninstall|repair [--endpoint <id>] | soak --minutes N --report <file>`.
  - Commands go through the running instance's pipe when one exists; otherwise the CLI does it directly where that makes sense (`apo`, `status`).
- [ ] Autostart: HKCU `Run` value `"<exe>" --background`, toggled from tray/settings.
- [ ] Shutdown: Quit flushes persistence, stops services, removes the tray, then `handle.quit()`. Ctrl+C in CLI mode is handled cleanly.
- [ ] Startup time: lazy-init everything not needed for the tray. Measure cold start to tray (target < 150 ms).

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
