# 06 — Decision Log

Format: ID · date · status · decision · why · consequences. Newest at the bottom. Never edit an accepted decision;
supersede it with a new entry.

---

### D-001 · 2026-11-16 · Accepted — Windows first
**Decision:** v0.1 targets Windows 10 22H2+ / Windows 11 (x86_64, MSVC toolchain) only.
**Why:** The routing story (APO) is OS-specific and the largest risk; one platform done well beats three done badly.
**Consequences:** `tine-dsp` and `tine-protocol` stay platform-neutral (no `windows` crate). `tine-devhost` uses cpal so
macOS/Linux can be added later. ARM64 Windows is a stretch goal (APO DLL must match OS architecture).

### D-002 · 2026-11-16 · Accepted — Single user-side process
**Decision:** One `tine.exe` hosts tray, control plane and the on-demand Rune window. No separate UI executable.
**Why:** Simplest lifecycle, no IPC between UI and control, matches in-process atomics for UI ⇄ control.
**Consequences:** Window close must drop the wgpu device/surface while the event loop and tray survive (Risk R-04).

### D-003 · 2026-11-16 · Accepted — Open-source release
**Decision:** Public repository, free binaries via GitHub Releases. Licensed `MIT OR Apache-2.0` (D-015).
**Why:** Trust (it runs inside the system audio engine), community archetype presets, contrast with closed vendor suites.
**Consequences:** Dependency licenses must be compatible (cpal Apache-2.0, nnnoiseless BSD-3, Rune MIT, biquad MIT/Apache). Needs
CONTRIBUTING, issue templates, reproducible release builds. Code signing for an OSS project: see R-02.

### D-004 · 2026-11-16 · Accepted — Native APO from v0.1 (supersedes AGENTS.md §7 Phase 1)
**Decision:** Ship the DSP as a Windows APO (`tine_apo.dll`) inserted on the user's physical capture endpoint from the first
release. No virtual-cable dependency.
**Why:** Zero-setup routing for every app; no extra latency hop; it is the product's main differentiator vs Voicemeeter.
**Consequences:**
- The live audio path runs in `audiodg.exe`, not in `tine-engine`. `tine-engine` becomes the control plane.
- A feasibility spike (T02) must prove an APO built in Rust loads and processes on a capture endpoint on stock Windows 11
  **before** the DSP work is wired to it. If the spike fails, fall back to the VB-Cable plan from `AGENTS.md` §7 Phase 1 (new decision required).
- Requires admin (UAC) for install/repair, careful registry handling, and wrapping of any vendor SFX APO.

### D-005 · 2026-11-16 · Accepted — Cross-process shared memory replaces `triple_buffer` for APO telemetry
**Decision:** Params and telemetry cross the process boundary via a versioned `#[repr(C)]` named file mapping with hand-rolled
atomic triple buffer and SPSC ring. *(In-process use of `triple_buffer`/`rtrb` was later dropped by D-009.)*
**Why:** Those crates allocate on the heap with in-process pointers; they cannot live in shared memory.

### D-006 · 2026-11-16 · Accepted — Rate-agnostic DSP with RNNoise-only resampling
**Decision:** All stages design coefficients from the actual sample rate. Only the denoiser resamples to/from 48 kHz when needed.
**Why:** An APO cannot force the endpoint to 48 kHz; resampling the whole chain adds latency and CPU for no benefit.

### D-007 · 2026-11-16 · Accepted — Pro edits pin parameters
**Decision:** Editing a parameter in Pro mode pins it; macros no longer change pinned parameters until unpinned.
**Why:** Avoids the "my precise edit got overwritten by a macro" surprise while keeping one shared `DspParams`.

### D-008 · 2026-11-16 · Accepted — Spec corrections
- ~~`cpal` current release is 0.19.0~~ **Corrected same day:** 0.19.0 exists only on cpal's `master`; the latest *published* release is **0.18.2**, as the spec said. Use `cpal = "0.18.2"` with `features = ["realtime"]` (only `tine-engine`/`tine-devhost`).
- Rune: depend by pinned `rev` on `dev`, not `branch = "dev"` (upstream README recommends pinning; upstream accepts no PRs).
- `SharedDspState` in the spec lacks fields required by Pro mode (EQ type, gate ratio/hysteresis/attack, de-esser frequency,
  compressor knee, stage enables, auto-makeup). The authoritative list is the parameter table in [03-dsp-pipeline.md](03-dsp-pipeline.md).
- Spectrum is computed in `tine.exe` from a tap ring, not inside the RT thread.
- The header's "Output Target Selector" is replaced by an APO status badge.

### D-009 · 2026-11-16 · Accepted — One in-house lock-free implementation; drop `rtrb` and `triple_buffer` (supersedes D-005's in-process note)
**Decision:** `tine-protocol::sync` provides `ParamBlock`, `Triple<T>` and `SpscRing<N>`. They are `#[repr(C)]`, position-independent, and loom-tested,
and they're used both in shared memory and in-process. `rtrb` and `triple_buffer` are not dependencies.
**Why:** We must write and verify the shared-memory versions anyway. Two implementations of the same pattern double the
risk. `triple_buffer` 9.x moved to MPL-2.0, which is avoidable for ~100 lines of code.

### D-010 · 2026-11-16 · Accepted — In-house biquads; drop the `biquad` crate
**Decision:** `tine-dsp::primitives::biquad` implements RBJ cookbook designs (computed in `f64`), TDF-II processing in `f32`,
Linkwitz–Riley crossovers, analytic magnitude for the UI curve, and change-detected retuning.
**Why:** The hottest code in the product. We need control over coefficient smoothing, block loops that auto-vectorize,
extra designs (LR4), and the exact same math for the UI curve. It's ~200 lines, verified against analytic responses.

### D-011 · 2026-11-16 · Accepted — Use Rune's app lifetime and tray; drop `tray-icon`
**Decision:** `tine.exe` uses `rune::App::new(spec).windowless().keep_running().release_gpu_when_closed()` and `rune::Tray`
(Shell_NotifyIcon on its own thread, flat menu).
**Why:** Rune already implements the exact lifecycle D-002 needs (closing drops window + GPU device, state is kept), and its tray
works without the app's event loop. One fewer dependency (and its `muda`/`winit` coupling). Lowers R-04.
**Consequences:** The tray menu is flat (Action and Separator only): checked state goes in the label text, and archetypes are four items.

### D-012 · 2026-11-16 · Accepted — Toolchain and build baseline
**Decision:**
- Rust 1.99.1 pinned (same as Rune), edition 2024, resolver 3
- `x86_64-pc-windows-msvc` with `target-cpu=x86-64-v2`
- release: fat LTO, `codegen-units = 1`
- Rune pinned at `631e8a67cb3c40225efa3f6c4f57b80e7b18f190`, with its `[patch.crates-io]` entries mirrored in our workspace (Rune requires consumers to apply its patched `winit`, `wgpu-hal`, `parley_engine`, `accesskit_windows`)

**Why:** Reproducible builds that match Rune's CI. x86-64-v2 is guaranteed on Windows 11 hardware and enables SSE4.2 codegen.

### D-013 · 2026-11-16 · Accepted — Small CLI parser and no convenience crates in the binary
**Decision:** `lexopt` for argument parsing. Known folders through `SHGetKnownFolderPath` (no `dirs`/`directories`). `thiserror` for errors, no `anyhow`.
**Why:** The binary-size and startup budgets ([09-engineering-standards.md](09-engineering-standards.md) §7). Each of these is a few lines of our own code.

### D-014 · 2026-11-16 · Accepted — Parallel-agent workflow
**Decision:**
- Every module is stubbed by T01 according to the module map in [02-architecture.md](02-architecture.md); each task owns a disjoint set of files.
- Contracts (`tine-protocol`, the `DspPipeline` skeleton, `tine-ui`'s `UiModel`/`Intent`) land first.
- `tine-ui` is engine-agnostic; `tine-app` bridges the two in T21.

**Why:** Four lanes (DSP, APO, Engine/App, UI) can then progress at the same time without merge conflicts or waiting on each other.

### D-015 · 2026-11-16 · Accepted — License: `MIT OR Apache-2.0` (resolves Q-01)
**Decision:** Tine is dual-licensed under MIT or Apache-2.0, at the user's option (`license = "MIT OR Apache-2.0"` in every crate).
Contributions are accepted under the same terms (inbound = outbound, stated in CONTRIBUTING.md; no CLA).
**Why:** The Rust ecosystem standard. It's compatible with every dependency (cpal Apache-2.0, nnnoiseless BSD-3-Clause, Rune MIT,
windows MIT/Apache), maximally contributor-friendly, and the Apache side adds an explicit patent grant.
**Consequences:** T01 adds `LICENSE-MIT` and `LICENSE-APACHE`; the README carries the standard dual-license notice. The `deny.toml`
allow-list stays permissive (no GPL dependencies may be added, since they would make the combined binary GPL).

### D-016 · 2026-11-16 · Accepted — Test hardware and fixture strategy (resolves Q-07)
**Decision:** The maintainer has a **dynamic USB/XLR mic** and a **gaming headset** on one main Windows machine. There's no condenser, no laptop/webcam mic, no Bluetooth headset, no Realtek jack, and no spare PC or VM. Therefore:
- **Self-recorded fixtures** (raw, 48 kHz, through `tine-devhost --record-raw`): `dynamic_speech`, `headset_speech`, `noisy_quiet`, `sibilant`, `plosive`, `clipping`, all following one recording script ([T09](../tasks/T09-dsp-integration.md#recording-script)).
- **Condenser proxy:** a public-domain LibriVox excerpt (typically recorded on USB condensers in untreated rooms), with provenance noted.
- **Laptop proxy:** CC0 clips from Mozilla Common Voice chosen for laptop-like sound, plus a *simulated* laptop fixture (clean self-recorded speech + CC0 room impulse response + fan/keyboard noise).
- **APO spike safety without a VM:** a System Restore point, a `.reg` export of the whole `MMDevices\Audio\Capture` key, and a recovery script that works from Safe Mode, prepared *before* the first install (see T02).
- **Coverage gaps** (condenser, laptop, Bluetooth HFP, Realtek/vendor APOs) are closed by a **community beta** before v0.1. Testers run `tine diag`, which produces a privacy-safe report (formats, APO state, logs, no audio), and file it on GitHub (T17, T22).

**Why:** Real-mic recordings for the two archetypes we own; legally clean proxies for the rest. The riskiest hardware paths (vendor APOs, Bluetooth) need many machines, which an open-source beta provides.

### D-017 · 2026-11-16 · Accepted — Spawn `tine.exe --ui` child process for on-demand window (supersedes D-002)
**Decision:** Keep the always-on tray (`rune::Tray`), single-instance guard (`Local\Tine.SingleInstance` + `\\.\pipe\tine-<user-sid>`), and control plane in `tine.exe`, and spawn `tine.exe --ui` as a short-lived child process when the user opens the window. Closing the window exits `tine.exe --ui`. (`tine.exe` still uses `rune::Tray`, preserving D-011's `tray-icon` removal.)
**Why:** T03 measured Rune's in-process `App::new(...).windowless().keep_running().release_gpu_when_closed()` on Windows 11 (DX12, NVIDIA RTX 5070): even after a single open/close cycle, the DX12/NVIDIA user-mode driver (`nvwgf2umx.dll`, `d3d12.dll`, `dxgi.dll`) remains resident in-process, leaving **14.73 MB dedicated + 12.24 MB shared GPU memory**, **87.21 MB Working Set / 113.73 MB Private Bytes**, **20 threads** (+12), and **414 handles** (+269); after 50 cycles: **15.70 MB dedicated + 15.24 MB shared GPU**, **85.19–173.65 MB WS / 92.52–197.57 MB Private Bytes**, **22–23 threads**, **524–544 handles**. This violates the 0 GPU memory and 20 MB RSS hard budgets (R-04). With `tine.exe --ui` as a child process, the parent stays at **1.74–1.88 MB Private Bytes / 9.25–9.53 MB Working Set**, **0.00 MB GPU memory**, **2–6 threads**, **135–137 handles**, and **0.0 % CPU** over 5 min idle and across 50 open/close cycles; the child opens in **~4.2–5.1 ms** to `on_window`.
**Consequences:** T17 (`tine-app`) implements the parent tray + control plane + `--ui` child spawner; UI ⇄ engine state crosses via `tine-protocol` shared memory or a local pipe.

### D-018 · 2026-11-16 · Accepted — Virtual-cable output from `tine-engine` for v0.1 (supersedes D-004)
**Decision:** Drop the native Windows APO (`tine_apo.dll`) for v0.1 and route processed microphone audio from `tine-engine` (via `cpal` WASAPI) to a virtual audio cable (`CABLE Input (VB-Audio Virtual Cable)` or equivalent), per the T02 NO-GO fallback plan.
**Why:** T02 tested an unsigned Rust COM APO (`apo_spike.dll`, 194,560 bytes) on stock Windows 11 Build `10.0.26300.0` (24H2+) on both a dynamic USB mic (`fifine Microphone`) and a gaming headset (`AB13X USB Audio`) via `FxProperties` (`PKEY_FX_Association`, `PKEY_FX_StreamEffectClsid`, `PKEY_SFX_ProcessingModes_Supported_For_Streaming`, `PKEY_AudioEndpoint_Disable_SysFx = 0`) followed by a full `Restart-Service audiosrv -Force`. Although `audiodg.exe` read `Disable_SysFx = 0` (switching `AB13X` from 1ch to 2ch), it never loaded `apo_spike.dll` (`DllMain` never ran, `Global\TineSpike.v1` was never created, `OpenFileMappingW` returned `0x80070002`, and zero CodeIntegrity events were logged). Both endpoints carry driver-enumerated `VocaEffectPack` (`SWD\DRIVERENUM\...#VocaEffectPack`), leaving no reliable unsigned `FxProperties` SFX registration path on stock Win11 24H2+.
**Consequences:**
- Replan Lane B (`T13`, `T14`) from `tine_apo.dll` + `FxProperties` admin to `tine-engine` `cpal` virtual-cable output + virtual-cable detection/onboarding.
- Lanes A (`tine-dsp`), C (`tine-engine`/`tine-app` shell), and D (`tine-ui`) are unaffected; `tine-ui` header shows virtual-cable routing status instead of APO status.
- Re-evaluating a driver-packaged/signed APO (e.g. via INF + SignPath/Azure Trusted Signing) is deferred to post-v0.1.

### D-019 · 2026-11-16 · Accepted — Measured pipeline latency is 1036 samples at 48 kHz; the ≤ 552-sample (11.5 ms) added-latency budget is not achievable with `nnnoiseless` (resolves T09 latency criterion)
**Decision:** The 7-stage `DspPipeline`'s added latency at 48 kHz is **1036 samples (21.58 ms)**: Stage 2 contributes **960** (480-sample FIFO framing for arbitrary host block sizes + a **480-sample synthesis delay inherent to `nnnoiseless`**: `DenoiseState::process_frame` overlap-adds a 960-sample STFT window, so the output frame corresponds to the *previous* input frame — verified in nnnoiseless `features.rs::frame_synthesis` and by impulse measurement), and Stage 6 contributes **76** (1.5 ms lookahead = 72 samples + 4-sample true-peak FIR group delay; the FIR delay is always in the audio path so toggling true-peak keeps latency constant). `latency_samples()` = Σ stage latencies, wet and bypass-dry paths are exactly time-aligned (impulse bit-exact at `latency_samples()`; A/B cross-correlation peak at lag 0). Update `09-engineering-standards.md` §7 "Added latency ≤ 11.5 ms" to ≤ **21.6 ms at 48 kHz (1036 samples)** — kept ≤ 552 only if the denoiser stage is disabled at runtime in a future mode.
**Why:** The 11.5 ms budget assumed the denoiser adds one frame (10 ms); it actually adds two (20 ms) before any output emerges, and the parameter "fixed 10 ms FIFO latency" in AGENTS §4 understates this. A constant, block-size-independent latency of < 960 samples is impossible with the 480-frame model while accepting arbitrary host callback sizes; even with perfectly 480-aligned blocks the OLA synthesis delay alone is 480.
**Consequences:**
- T01 owner updates the `09-engineering-standards.md` §7 budget row (requested via T09's `## Requests`); the T09 task-file criterion "Added latency at 48 kHz ≤ 552 samples" is recorded as a measured deviation, not an implementation defect.
- "Hear Myself" self-monitoring (tine-engine + virtual cable, D-018) stacks ~21.6 ms DSP latency on top of endpoint buffering → see R-15.
- Options if the budget must return: a lower-latency denoiser (shorter-window model), or a "low-latency mode" that routes the denoiser stage out of the chain (its latency is in `CleanupStage::latency()`, so a future short-window design drops total to ≤ 96+76 samples).
