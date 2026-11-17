# Tine Task Status

| ID | Title | Lane | Status | Branch | PR | Agent |
| :--- | :--- | :--- | :--- | :--- | :--- | :--- |
| T01 | Workspace, toolchain, CI, dependency baseline | 0 — Foundation | done | `task/T01-workspace` | #1 | T01-worker |
| T02 | Spike (go/no-go): Rust APO on a capture endpoint | B — Windows / APO | done (NO-GO → D-018) | `task/T02-apo-spike` | #4 | T02-worker |
| T03 | Spike: Rune windowless app, tray, GPU release, memory | C — Engine / App | done | `task/T03-rune-spike` | #3 | T03-worker |
| T04 | `tine-protocol`: params, lock-free primitives, shm layout | 0 — Foundation | done | `task/T04-protocol` | #5 | T04-worker |
| T05 | DSP primitives, test/bench harness, `DspPipeline` skeleton | 0 — Foundation | done | `task/T05-dsp-foundation` | #7 | T05-worker |
| T06 | Stage 1–2: gain/DC/HPF, RNNoise + expander | A — DSP | done | `task/T06-dsp-cleanup` | #8 | T06-worker |
| T07 | Stage 3–4: 5-band EQ, split-band de-esser | A — DSP | done | `task/T07-dsp-tone` | #10 | T07-worker |
| T08 | Stage 5–6: compressor, lookahead limiter | A — DSP | done | `task/T08-dsp-dynamics` | #11 | T08-worker |
| T09 | Wire stages, fixtures, golden regression, perf gate | A — DSP | done | `task/T09-dsp-integration` | #15 | t09 (GLM) |
| T10 | Archetype presets + `MacroMapper` | A — DSP | done | `task/T10-archetypes-macros` | #16 | t10 (GLM) |
| T11 | `AutoCalibrator` | A — DSP | done | `task/T11-auto-calibrator` | #20 | t11 (GLM) |
| T12 | cpal audio I/O module + `tine-devhost` | C — Engine / App | done | `task/T12-audio-io` | #9 | T12-worker |
| T13 | Virtual-cable DSP routing loop (`D-018`) | B — Virtual Cable | done | `task/T13-virtual-cable` | #13 | T13-worker |
| T14 | Virtual cable discovery, DeviceWatcher & endpoint admin (`D-018`) | B — Virtual Cable | done | `task/T14-endpoint-admin` | #12 | T14-worker |
| T15 | Engine core: shm client, controller, persistence, devices | C — Engine / App | done | `task/T15-engine-core` | #17 | t15 (GLM) |
| T16 | Engine audio services: spectrum, Auto-Tune runner, Hear Myself | C — Engine / App | done | `task/T16-engine-audio-services` | #23 | t16 (GLM) |
| T17 | `tine.exe` shell: Rune App, tray, single instance, CLI, logging | C — Engine / App | done | `task/T17-app-shell` | #22 | t17 (GLM) |
| T18 | UI foundation: view model, theme, headless harness | D — UI | done | `task/T18-ui-foundation` | #6 | T18-worker |
| T19 | Header + Golden Line canvas | D — UI | done | `task/T19-ui-main-screen` | #18 | t19 (GLM) |
| T20 | Pro rack, Auto-Tune overlay, onboarding | D — UI | done | `task/T20-ui-pro-autotune-onboarding` | #21 | t20 (GLM) |
| T21 | End-to-end integration in `tine.exe` | Z — Integration & ship | done | `task/T21-end-to-end` | #24 | t21 (GLM) |
| T22 | QA: soak, perf, mic/app matrix, listening tests | Z — Integration & ship | todo | — | — | — |
| T23 | Installer, signing, release, OSS docs | Z — Integration & ship | done | `task/T23-packaging-release` | #29 | t23 (GLM) |
