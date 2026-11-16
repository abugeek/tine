# Tine Task Status

| ID | Title | Lane | Status | Branch | PR | Agent |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| T01 | Workspace, toolchain, CI, dependency baseline | 0 — Foundation | done | `task/T01-workspace` | #1 | T01-worker |
| T02 | Spike (go/no-go): Rust APO on a capture endpoint | B — Windows / APO | done (NO-GO → D-018) | `task/T02-apo-spike` | #4 | T02-worker |
| T03 | Spike: Rune windowless app, tray, GPU release, memory | C — Engine / App | done | `task/T03-rune-spike` | #3 | T03-worker |
| T04 | `tine-protocol`: params, lock-free primitives, shm layout | 0 — Foundation | in-progress | `task/T04-protocol` | — | T04-worker |
| T05 | DSP primitives, test/bench harness, `DspPipeline` skeleton | 0 — Foundation | todo | — | — | — |
| T06 | Stage 1–2: gain/DC/HPF, RNNoise + expander | A — DSP | todo | — | — | — |
| T07 | Stage 3–4: 5-band EQ, split-band de-esser | A — DSP | todo | — | — | — |
| T08 | Stage 5–6: compressor, lookahead limiter | A — DSP | todo | — | — | — |
| T09 | Wire stages, fixtures, golden regression, perf gate | A — DSP | todo | — | — | — |
| T10 | Archetype presets + `MacroMapper` | A — DSP | todo | — | — | — |
| T11 | `AutoCalibrator` | A — DSP | todo | — | — | — |
| T12 | cpal audio I/O module + `tine-devhost` | C — Engine / App | todo | — | — | — |
| T13 | `tine_apo.dll` production APO | B — Windows / APO | todo | — | — | — |
| T14 | APO install / health / repair / uninstall | B — Windows / APO | todo | — | — | — |
| T15 | Engine core: shm client, controller, persistence, devices | C — Engine / App | todo | — | — | — |
| T16 | Engine audio services: spectrum, Auto-Tune runner, Hear Myself | C — Engine / App | todo | — | — | — |
| T17 | `tine.exe` shell: Rune App, tray, single instance, CLI, logging | C — Engine / App | todo | — | — | — |
| T18 | UI foundation: view model, theme, headless harness | D — UI | todo | — | — | — |
| T19 | Header + Golden Line canvas | D — UI | todo | — | — | — |
| T20 | Pro rack, Auto-Tune overlay, onboarding | D — UI | todo | — | — | — |
| T21 | End-to-end integration in `tine.exe` | Z — Integration & ship | todo | — | — | — |
| T22 | QA: soak, perf, mic/app matrix, listening tests | Z — Integration & ship | todo | — | — | — |
| T23 | Installer, signing, release, OSS docs | Z — Integration & ship | todo | — | — | — |
