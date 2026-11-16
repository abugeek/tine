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
