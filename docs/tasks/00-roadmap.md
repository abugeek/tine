# 00 — Roadmap & Task Index

Goal: a public **Tine v0.1** for Windows. A user installs it, picks their mic, runs Auto-Tune, and every app (Discord,
games, OBS, browsers) hears processed audio through a native APO, with no virtual cable.

## Lanes (assign one agent per lane; the DSP lane can take up to 3 agents during T06–T08)

| Lane | Focus | Tasks (in order) |
| :--- | :--- | :--- |
| **0 — Foundation** | Repo, toolchain, CI, contracts | T01 → T04 → T05 |
| **A — DSP** | `tine-dsp` stages, adaptation | T06 ‖ T07 ‖ T08 → T09 → T10 → T11 |
| **B — Windows / APO** | `tine-apo`, registration | T02 → T13 → T14 |
| **C — Engine / App** | `tine-engine`, `tine-app`, devhost | T03 → T12 → T15 → T16 → T17 |
| **D — UI** | `tine-ui` | T18 → T19 → T20 |
| **Z — Integration & ship** | everything | T21 → T22 → T23 → T24 → T25 ‖ T26 ‖ T27 ‖ T28 |

## Task index

| ID | Title | Lane | Depends on | Size |
| :--- | :--- | :--- | :--- | :--- |
| [T01](T01-workspace-bootstrap.md) | Workspace, toolchain, CI, dependency baseline | 0 | — | M |
| [T02](T02-spike-apo-feasibility.md) | **Spike (go/no-go):** Rust APO on a capture endpoint | B | T01 | L |
| [T03](T03-spike-rune-app-lifecycle.md) | **Spike:** Rune windowless app, tray, GPU release, memory | C | T01 | M |
| [T04](T04-protocol-contracts.md) | `tine-protocol`: params, lock-free primitives, shm layout | 0 | T01 | M |
| [T05](T05-dsp-foundation.md) | DSP primitives, test/bench harness, `DspPipeline` skeleton | 0 | T04 | M |
| [T06](T06-dsp-stage-cleanup.md) | Stage 1–2: gain/DC/HPF, RNNoise + expander | A | T05 | M |
| [T07](T07-dsp-stage-tone.md) | Stage 3–4: 5-band EQ, split-band de-esser | A | T05 | M |
| [T08](T08-dsp-stage-dynamics.md) | Stage 5–6: compressor, lookahead limiter | A | T05 | M |
| [T09](T09-dsp-integration.md) | Wire stages, fixtures, golden regression, perf gate | A | T06, T07, T08 | M |
| [T10](T10-archetypes-macros.md) | Archetype presets + `MacroMapper` | A | T09 | M |
| [T11](T11-auto-calibrator.md) | `AutoCalibrator` | A | T10 | M |
| [T12](T12-audio-io-devhost.md) | cpal audio I/O module + `tine-devhost` | C | T03, T05 | M |
| [T13](T13-tine-apo.md) | `tine_apo.dll` production APO | B | T02, T05 | L |
| [T14](T14-apo-admin.md) | APO install / health / repair / uninstall | B | T13 | L |
| [T15](T15-engine-core.md) | Engine core: shm client, controller, persistence, devices | C | T10, T12 | L |
| [T16](T16-engine-audio-services.md) | Engine audio services: spectrum, Auto-Tune runner, Hear Myself | C | T11, T15 | M |
| [T17](T17-app-shell.md) | `tine.exe` shell: Rune App, tray, single instance, CLI, logging | C | T16 | M |
| [T18](T18-ui-foundation.md) | UI foundation: view model, theme, headless harness | D | T03, T04 | M |
| [T19](T19-ui-main-screen.md) | Header + Golden Line canvas | D | T10, T18 | L |
| [T20](T20-ui-pro-autotune-onboarding.md) | Pro rack, Auto-Tune overlay, onboarding | D | T11, T19 | L |
| [T21](T21-end-to-end-integration.md) | End-to-end integration in `tine.exe` | Z | T14, T17, T20 | M |
| [T22](T22-qa-soak-perf.md) | QA: soak, perf, mic/app matrix, listening tests | Z | T21 | M |
| [T23](T23-packaging-release.md) | Installer, signing, release, OSS docs | Z | T22 | M |
| [T24](done/T24-perf-competitor-followup.md) | Competitor benchmark follow-up: stream health, idle cost, memory (`done`, #32) | Z | T22 | M |
| [T25](done/T25-cli-cable-ux.md) | CLI & diagnostics virtual-cable completeness (`done`) | Z | T24 | S |
| [T26](done/T26-package-managers-and-cable-bootstrap.md) | WinGet & Scoop package manifests + virtual-cable setup script (`done`) | Z | T24 | M |
| [T27](done/T27-audio-demo-and-traction-kit.md) | Reproducible audio A/B showcase & launch/SignPath playbook (`done`) | Z | T24 | M |
| [T28](done/T28-docs-release-sync.md) | Documentation, changelog, security & roadmap release sync (`done`, D-032..D-041) | Z | T24 | S |

## Dependency graph

The arrows mean "must be done before".

```text
Lane 0   T01 ──► T04 ──► T05
Lane A                    T05 ──► T06 ┐
                          T05 ──► T07 ├──► T09 ──► T10 ──► T11
                          T05 ──► T08 ┘
Lane B   T01 ──► T02 ─────────────────────────► T13 ──► T14          (T13 also needs T05)
Lane C   T01 ──► T03 ──► T12 ──► T15 ──► T16 ──► T17                 (T12 needs T05; T15 needs T10; T16 needs T11)
Lane D   T03 + T04 ──► T18 ──► T19 ──► T20                           (T19 needs T10; T20 needs T11)
Lane Z   T14 + T17 + T20 ──► T21 ──► T22 ──► T23 ──► T24 ──► T25 ‖ T26 ‖ T27 ‖ T28
```

## Maximum parallelism by stage

| When | Agents can run at once |
| :--- | :--- |
| After T01 | T02 (B), T03 (C), T04 (0) |
| After T04 | + T18 (D) once T03 is done |
| After T05 | T06, T07, T08 (A ×3), T12 (C), T13 (B, after T02) |
| After T09 | T10 (A), T13/T14 (B), T12→T15 (C), T18 (D) |
| After T11 | T16 (C), T20 (D), T14 (B) |

## Go / no-go gates

- **T02 fails** (Windows won't load our APO on stock Win11): record a new decision that supersedes D-004, then replace T13/T14 with
  "cpal output to virtual cable" tasks. Lanes A, C and D are unaffected because the DSP and control plane are the same.
- **T03 fails** (Rune can't release the GPU or keep a tray while closed): record a decision for the `tine.exe --ui` child-process fallback; only T17/T21 change.

## Milestones

| Milestone | Definition |
| :--- | :--- |
| **M0 — Feasible** | T01–T05 done; both spikes green; contracts frozen. |
| **M1 — Sounds good offline** | T09–T11 done; fixtures processed through each archetype measurably improve. |
| **M2 — Sounds good live** | T13–T17 done; Discord hears processed audio with no virtual cable, controlled from tray/CLI. |
| **M3 — Usable by anyone** | T18–T21 done; full UI flow headless-verified and running live. |
| **M4 — v0.1 released** | T22–T28 done; installer, package manifests, provenance, and docs synced for GitHub Releases. |
