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

### D-020 · 2026-11-17 · Accepted — `tine-protocol` plain data types are never feature-gated (T10 contract visibility fix; orchestrator-approved amendment to T04)
**Decision:** Plain data structs in `tine-protocol` (`Calibration`, `CalibrationOverrides`) compile unconditionally; only the serde plumbing is feature-gated: derives move to the structs as `#[cfg_attr(feature = "serde", derive(...))]` and the allocating helpers (`Calibration::validate`, which builds a `Vec` of `FieldError`s) stay behind the `serde` feature. JSON wire format is unchanged.
**Why:** `tine-dsp` depends on `tine-protocol` with `default-features = false` (it must not depend on `serde_json`, AGENTS §2), yet the T10/T11 contracts pass `tine_protocol::presets::Calibration` into `MacroMapper::apply`/`explain` and `CalibrationReport`. T04 had gated the whole `serde_types` module, making the frozen contract unusable from `no_std` consumers. The feature stays off for the library, so no `serde`/`serde_json` reaches the DSP/APO build; only test binaries enable it via a dev-dependency.

### D-021 · 2026-11-17 · Accepted — Golden Line macro curve semantics (resolves the T10 Layer C ambiguity)
**Decision:** each macro position `m ∈ [0,1]` maps to a linear offset **centred on the parameter's effective base** (preset Layer A value, or the calibrated value when one exists), so `m = 0.5` always reproduces the archetype exactly (as calibrated). The documented macro ranges are spans around that anchor:
- **Purity:** `denoise_mix` = a ± 0.5; `gate_threshold_db` = a ± 15 dB.
- **Body:** eq.0 low-shelf gain = b ± 4 dB; eq.1 mud gain = b − 5 dB·max(0, 2m−1) (cut deepens only above centre, never boosts the mud).
- **Presence:** eq.3 peak = b + 5 dB·max(0, 2m−1); eq.4 air = b + 6 dB·max(0, 2m−1); de-esser enabled = base_enabled OR m > 0.6; amount = max(b, 3 + 5·clamp((m−0.6)/0.4, 0, 1)) dB (3 → 8 dB over 0.6 → 1.0).
- **Leveler:** `comp_ratio` = b ± 1.75; `comp_thresh_db` = a − 10 dB·max(0, 2m−1) ("calibrated threshold − 0…10 dB"); auto-makeup on.
A calibration's derived `input_gain_db`, `gate_threshold_db`, `denoise_mix`, `comp_thresh_db` and `comp_makeup_db` replace the preset bases (manual verified makeup ⇒ auto-makeup off), and the macros then move *relative to* those anchors. Pinned parameters (D-007) are copied from `current` and driven by neither macros nor calibration. All outputs clamp through the `tine-protocol` SPECS. Implemented in `tine-dsp::macros` (`MacroMapper`).
**Why:** The Layer C prose in AGENTS §3 / 04-mic-adaptation listed absolute ranges ("`denoise_mix` 0 → 1", "gate −60 → −30 dB", "mud 0 → −5 dB") that anchor on no consistent base and contradict the T10 task rule "macro at 0.5 gives the preset value". One centred-span rule satisfies every acceptance criterion, is monotone in the documented directions, matches every documented upper extreme (and the dynamic archetype's −2 → +6 dB shelf literally), and keeps all outputs inside SPECS. The mapping-table updates live in 04-mic-adaptation.md and AGENTS §3.

### D-022 · 2026-11-17 · Accepted — APO boot-params file format (`%ProgramData%\Tine\endpoints\<sanitized-id>.json`)
**Decision (from T15):** The file `tine.exe` persists and the APO reads at `LockForProcess` (D-004 boot params) is JSON: `{"schema":1,"endpoint_id":"<raw MMDevice id>","params":{<ParamSpec.key>: f32}}` — all 45 parameters under their stable `tine-protocol` SPECS keys, booleans encoded as `0.0`/`1.0`, EQ band params keyed `eq.<band>.{type,freq_hz,gain_db,q}`, with filter **types stored as their numeric `f32` value (`0.0..=6.0`, `FilterType::as_f32`)** — the same convention `DspParams::get`/`ParamBlock` use, so one `DspParams::set` round-trips everything. The filename uses `SanitizedId::from_raw(endpoint_id)`. Writers: `tine-engine::persist::save_boot_params` (debounced 500 ms + flush on exit); readers get clamping for free (`DspParams::set`), so a hand-edited file can never push a parameter out of SPECS range, and unknown keys are ignored.
**Why:** T13's `boot.rs` had no format yet and needed one contract to code against; SPECS keys are already the machine-stable identifiers everywhere else (CLI, pipe, JSON).
**Consequences:** T13's APO boot loader consumes this file via `tine-engine::persist::load_boot_params` (or reimplements the identical format); `EndpointProfile`/`AppSettings` remain the user-facing schemas (schema `1`, migrated from schema `0` = pre-versioning files, corrupt files backed up as `.corrupt-<ts>` with defaults used).
### D-023 · 2026-11-17 · Accepted — Auto-Calibrator measurement semantics (resolves the T11 spec ambiguities)
**Decision:** The T11 `AutoCalibrator` (Layer B) resolves the spec's under-specified points as follows, implemented in `tine-dsp::calibrator`:
- **SNR** is measured as `RMS(whole 5 s voice segment, gaps included) − noise_rms_db`, *not* the integrated voiced RMS. With voiced windows defined as "> noise + 10 dB", a voiced-window SNR is mathematically ≥ 10 dB and the documented `TooNoisy { snr_db < 10 }` rejection would be unreachable; the whole-segment definition makes it reachable (e.g. +11 dB bursts at 50 % duty ⇒ SNR ≈ 8 dB) and matches the user-facing meaning "your room is too loud for your voice".
- **`TooQuiet`** (no payload) fires when the peak of the *whole measured capture* (noise + voice, countdown excluded) is below **−80 dBFS** — a muted or dead mic (it also covers `finish()` called before any capture). Failure precedence: `Clipping` → `TooQuiet` → `NotEnoughSpeech` → `TooNoisy` → `GainOutOfRange`.
- **Noise RMS** = median of *linear-power* 100 ms window RMS values (30 windows over 3 s at any fs); the median makes the measurement robust to a single click, as documented.
- **Spectral tilt** = `10·log10(E(<350 Hz)/E(1–6 kHz))` over voiced windows, with the low band = low-pass @350 Hz (Q 0.707) and the mid band = RBJ band-pass centred 3.5 kHz with Q = center/bandwidth = 0.7 (skirt-gain form, so the passband covers the 1–6 kHz range at ≈ −3 dB). Reported only (Body suggestion), never auto-applied.
- **`CalibrationReport.changes`** = per-parameter `(id, before, after)` over exactly the five D-021 calibration parameters, both values obtained by running `MacroMapper::apply` *without* vs. *with* the calibration (same macros/pins/current); rows are therefore full macro-shifted parameter values, not raw calibration fields (e.g. with the dynamic preset's purity 0.4, the gate row's `before` is −53 dB, not the −50 dB preset base).
- **Offline verification makeup search:** start at 0 dB, simulate, then set makeup to `makeup + (−4.5 dB − peak)` and re-simulate, at most 4 iterations, keeping the run whose peak is closest to the −6…−3 dBFS window. Because `comp_makeup_db` SPECS are `0..=24 dB`, a peak that is already above −3 dBFS at 0 dB makeup cannot be lowered (makeup only raises peaks); the loop keeps makeup 0 as the closest. Input gain clamps to the SPECS `[−24, +36]` after the `> +36 dB` rejection, and every stored derived value is clamped into SPECS so `Calibration::validate` always passes.
**Why:** The 04-mic-adaptation Layer B prose and the T11 task file leave these points open (see the SNR/TooNoisy contradiction above, and the unspecified meaning of `TooQuiet` and of `changes` under macro shifts). These are the simplest choices that keep every documented rejection reachable and every acceptance criterion satisfiable.
**Consequences:** The wizard (T20) and the Auto-Tune runner (T16) build against these exact semantics: `push()` is allocation-free on a non-RT capture thread feeding any block size, `finish()` is deterministic and bit-reproduces the real `DspPipeline`, and its offline verification deliberately runs stages 1–4 once and re-runs only compressor+limiter per makeup candidate — proven bit-exact against a fresh `DspPipeline` (`offline_verification_matches_full_pipeline_bit_exactly`), which keeps `finish()` under its 50 ms budget while remaining the spec's "run the full pipeline" semantics.

### D-024 · 2026-11-17 · Accepted — One narrow UI adapter for the T11 calibrator contract (T20)
**Decision (from T20):** the `tine_dsp::calibrator` types (`CalPhase`/`CalProgress`/`CalFailure`/`CalibrationReport`) enter `tine-ui` in **exactly one module** — `views/autotune.rs` — through three functions: `autotune_from_progress(&CalProgress, Option<CalFailure>) -> AutoTuneUi` (Countdown→`Prepare { remaining_secs }`, Noise→`Silence`, Voice→`Speak`, per-phase ring derived from the real `phase_progress`; `voiced_ms` and the live level pass through; overall progress = measured / (`NOISE_S`+`VOICE_S`)), `From<CalFailure> for CalFailureUi` (the five failure payloads verbatim, each with a concrete remedy string), and `calibration_from_report(&CalibrationReport) -> CalibrationUi` (measurements + offline-verified `simulated_peak_db`/`simulated_rms_db` + 5 `changes` rows with SPECS labels/units). Everything else in `tine-ui` sees only the plain view-model types (`model.rs`).
**Why:** keeps the purity rule (tine-ui = `tine-protocol` + `tine-dsp`, never `tine-engine`) while the wizard is bit-faithful to the real state machine; the adapter is proven by `tine-ui`'s `adapter_tracks_the_real_calibrator_state_machine` test, which drives the real `AutoCalibrator` at 48 kHz in 480-sample blocks through every phase, a successful `finish()` (synthetic speech passing every `check_failures` gate, `simulated_peak_db` in −6..−3 dBFS), and a digital-silence `TooQuiet` rejection.
**Consequences:** the remaining integration is engine-only: T16/T21's capture runner owns the `push` thread, keeps `finish()`'s `Err(CalFailure)` to pass alongside the `Failed` phase (CalProgress alone doesn't carry it), and replays the report's `changes` on `Intent::ApplyCalibration { keep_pins }` (D-007 pins stay un-driven; `state.ui.autotune_keep_pins` defaults to keep). Rune-modal focus trapping + `focus_return("header/autotune")` is the T20/T19 pattern for all overlays.

### D-025 · 2026-11-17 · Accepted — tine.exe shell utilities are in-house: size-rotating log sink, stored-ZIP diag, registry autostart (T17)
**Decision (from T17):** the `tine.exe` shell ships no new dependencies for its utilities. Logging is a custom `tracing` layer writing `%LOCALAPPDATA%\Tine\logs\tine.log` with **size-based rotation** (2 MiB × 7; replaces tracing-appender, which only offers time-based rotation and was dropped from tine-app). `tine diag` writes a stored-ZIP (in-house CRC-32, FNV-1a id hashing) with a standards-conformant EOCD — verified with `python -m zipfile`. Autostart's source of truth is the **HKCU `Run`** value `"<exe>" --background`, not settings.json; `status` reflects the registry live. Autostart, `apo install|uninstall|repair`, and `--elevated-helper` are informational/no-op under D-018.
**Why:** the D-013 binary budget (measured 18.9 MB) and the deny.toml allow-list; logs compress badly anyway, so stored entries suffice; per-user registry state must never diverge from persisted settings.
**Consequences:** any new file-sink feature (e.g. T22 QA artifacts) reuses `logging.rs`/`zip.rs` rather than adding crates; the pipe protocol's drain contract (see R-17) binds every future pipe client including T21's `ui_bridge`.
### D-026 · 2026-11-17 · Accepted — T16 engine audio service conventions (spectrum, visualizer, Auto-Tune runner, monitor)
**Decision (from T16):**
- **Spectrum band energy** is a *fractional-bin-weighted power* sum: each 2048-pt real-FFT bin contributes power to a band proportional to its overlap with the band's log-frequency span, so a bin-centered full-scale tone whose skirt spans two bands reads ≈ +0…+2.5 dBFS (measured 1.82 dBFS) instead of 0 dBFS, and pink noise stays flat within **2.6 dB across bands 1–31** at the low end the visualizer target (band 1 = ~40–79 Hz) through 20 kHz. Band 0 (20–40 Hz) can sit **−2.2 dB below mid** on an exact-fixture input: with a Hann window at 2048 pts the main lobe is ±2 bins ≈ ±47 Hz, so any tone below ~40 Hz is sub-resolution. A decidicibel (0.1 dB) meter map with a −90 dBFS display floor is used; ballistics are per-hop one-pole (attack 10 ms, decay 300 ms at 60 Hz hop rate).
- **Visualizer liveness/heartbeat status is a `u8`, not `bool`** — the `Triple<T>` POD contract has no bool specialization — and the **bypass flag the UI reads comes from the APO header's `state_flags`** (word-typed), not from a second `Triple`, so the single-reader SPSC invariant of the telemetry `Triple` region is preserved for the visualizer's frame reader.
- **The visualizer/auto-tune timing waits use local Win32 handle wrappers** (`CreateEventW`/`CreateWaitableTimerExW` with `CREATE_WAITABLE_TIMER_HIGH_RESOLUTION`, a 60 Hz tick and 10 ms capture polls) justified by the `tine_engine::shm_client` precedent: named-poll deadlines need OS timer granularity the std library does not expose on Windows, the handles are owned/dropped RAII-style, and every unsafe call is narrow and documented.
- **The Auto-Tune runner contract** (T17/T20 depend on it): `AutoTuneRunner::start(AutoTuneConfig)` takes the capture spec, a `CalibratorFactory` (production `AutoCalibratorFactory` → real `tine_dsp::calibrator::AutoCalibrator`; scripted fakes stay for offline tests), a lease `ShmClient` and an optional `ControllerHandle`; it owns the 15 s bypass lease (renewed every 5 s, cleared by the explicit finish/cancel path so a killed `tine.exe` self-heals in ≤ 15 s), bypass confirmation within 2 s (state flag via the shared-memory header, reusing the visualizer's single-reader rule), 30 Hz `CalProgress` fan-out, routing `finish()`'s `CalibrationReport` into `Controller::Command::ApplyCalibration` with `KeepPins::Clear` semantics, and re-exports the frozen T11 types (`CalPhase`/`CalProgress`/`CalFailure`/`CalibrationReport`) from `tine_engine::autotune`. The `Calibrator`/`CalibratorFactory` seam exists only so integration tests can script capture outcomes. The 9 s acceptance is verified three ways: the scripted-capture state machine (~0.9 s wall at 10x fixture speed), the shared `noisy_quiet` fixture through the real calibrator (0.99 s wall at 10x; T11 acceptance semantics: denoise 0.85, speech RMS + input gain = −18 dBFS, simulated peak −4.5 dBFS within the −6…−3 window), and a real-mic run (~9 s, ignored test).
- **Monitor latency badge** = measured capture block duration (per-callback frame count / negotiated rate) + nominal 10 ms WASAPI shared-mode playback period + `DSP_LATENCY_MS` (21.6, D-019), reported as one `f32`; see R-18 for the "partially nominal" caveat.
**Why:** The band-energy/decibel conventions make the visualizer acceptably flat and tone-accurate on real content (measured: pink spread 2.6 dB bands 1–31, sub-resolution only in band 0), the liveness/bypass word-typed fields keep the SPSC/Triple invariants intact without redesigning the shm protocol, and the Auto-Tune runner structure isolates the only environment-injected behaviors (devices, calibration) behind factory traits so every path of the state machine is testable offline.

### D-027 · 2026-11-17 · Accepted — Monitor playback jitter-buffer headroom = 6 callbacks (T16 follow-up)
**Decision:** `audio_io::drift.rs` centers the playback-side local jitter buffer at `HEADROOM_CALLBACKS = 6` × the *actual* playback callback size (scaled to the source rate), up from the previous 2 callbacks. Priming still waits for the full headroom (silence during warm-up, never counted as an underrun); genuine starvation still sets the underrun flag — nothing is masked; the ±0.1 % PI drift clamp and the allocation-free RT paths are unchanged.
**Why:** T16's 1800 s monitor soak measured 2 underruns after warm-up (max callback 283 µs, overruns 0): at a 2-callback (~20 ms) center, any ≥ 20 ms WASAPI capture hiccup drains the buffer — the RT work was fine, the scheduler simply delivered capture late (T16 `## Requests`). Six callbacks (~60 ms) absorbs hiccups up to ~5 callbacks deep; the orchestrator-approved band was 4–6 and the top of the band maximizes the 0-underrun soak margin.
**Consequences:** Monitor and `devhost` end-to-end latency grows by ~40 ms of buffered audio (the 41.6 ms latency badge does not model the headroom — R-20 tracks folding it in). The prime delay grows to ~60 ms (~irrelevant next to the 21.6 ms DSP latency + device periods). Unit tests pin the semantics: headroom band pin, priming threshold (no underrun flag while warming), a 4-callback (40 ms) hiccup absorbed while a truly-draining gap still reports, drift-clamp bounds, allocation-free RT paths (negative-controlled), and `LocalRing` invariants.
**Verification:** 1800 s monitor soak on the physical endpoints (`Microphone (AB13X USB Audio)` -> `Speakers (AB13X USB Audio)`, quiet machine): **underruns 0, overruns 0**, 360,432 callbacks (399 during the 2 s warm-up prime, itself 0 underruns), max callback 236 µs, latency badge 41.6 ms — interval prints at +300/600/900/1200/1500/1800 s all clean, closing T16's monitor acceptance criteria.
### D-028 · 2026-11-17 · Accepted — T21 integration topology: one bridge module, two hosts (D-017 split realized headlessly and over the pipe)
**Decision:** `tine-app`'s `ui_bridge.rs` owns *all* engine ⇄ UI mapping and hosts both T21 topologies behind one set of adapters. (1) `EngineBridge` is the in-process, headless "full app": `Intent` → `ControllerHandle::send` / T16 `AutoTuneRunner` / T16 `Monitor` / `apo_admin` (behind `EndpointAdmin`/`MonitorHost`/`AutotuneHooks` seams), engine `Event::ModelChanged` → `UiModel`, `Visualizer` frames → `LiveFrame`, first-run detection, and error surfacing; it is the artifact the integration suite drives (`tests/e2e_flow.rs`) and the reference implementation for every future in-process embed. The Rune window is never allowed into the parent (D-017), so (2) the `--ui` child compiles the same adapters but talks to the parent's authoritative controller over the existing command pipe (`status` at ~0.5 Hz, `meters` at ≤ 60 Hz while visible, intent forwarding); endpoint list, `DeviceWatcher`, the D-018 virtual-cable badge (`apo_admin::health`), and first-run detection run locally in the child, and admin checks run on a worker thread. To make extern tests possible, `tine-app` gained a `tine_app` lib target; `main.rs` stayed a thin binary.
**Why:** The T21 spec predates D-017 (in-process window) and D-018 (no APO): its one-process `IntentSink → ControllerHandle` sketch cannot be wired literally without a second controller (double persistence) or Rune/wgpu in the tray parent. The two-host split keeps a single authoritative engine in the parent, preserves every service contract, and still exercises the spec's full flows headlessly.
**Consequences:**
- Four intents are not yet routable through the frozen pipe protocol and surface as warnings in the live child (`SelectEndpoint`, `Unpin`/`UnpinAll`, the Auto-Tune *progress/apply* dialog; `StartAutoTune` still reaches the parent's runner): see R-20 for the exact additive `PipeRequest` patch proposal.
- Under D-018 the parent's controller keeps `EngineConfig::shm` (unlinked in production), so the shell's router wiring (D-018 audio) remains the T17-requested wiring recorded in T21's `## Requests`.
- `tine.exe` grew to **21.44 MB** (release) because the real feature content linked in (`tine-ui` views + the `AutoCalibrator`'s offline DSP simulation with the `nnnoiseless` weights): the §7 "< 20 MB" row is superseded by this task's measurement — see R-21 for the budget request.

### D-029 · 2026-11-17 · Accepted — T21 production D-018 wiring: in-process routing loop, app-hosted bypass lease, tap-fed calibrator

**Decision:** The tray parent (`shell.rs`, retargeted under the orchestrator's authorization) spawns the controller against `EngineConfig::block` on a process-wide `services::param_block` and starts `VirtualCableRouter` in the same process (mic capture → `DspPipeline` → the discovered virtual-cable sink), with `services::router_follow` restarting the loop on endpoint changes after a successful new start (never through a failed restart). The router's health *is* the "linked" badge in this topology (`Target::Block` never emits `ApoLink`). Auto-Tune keeps the unmodified T16 runner semantics through an app-hosted lease: with no APO mapping, `services` creates a session-local `Local\Tine.v1.autotune-shim-*` mapping plus a mirror thread that (a) sets the mapping's `BYPASS` state flag while the runner's absolute-time lease is hot (the `confirm_bypass` handshake), and (b) asserts `bypass` on the shared `ParamBlock` (restoring the user's pre-run value on lease end) — never through the controller model, so persisted state can never record a calibration-induced bypass and a mid-run crash self-heals (the block was provocatively bypassed only in memory). The calibrator's capture is a `SampleSource` that drains the routing tap while the pump is parked (single tap reader preserved): the machine's USB mic refuses a second shared-mode capture client, and while the lease asserts DSP bypass the post-DSP tap *is* the raw (latency-aligned, ms-crossfaded) signal — the exact APO-world contract for a bypassed calibration. Meters reads keep single-reader invariants: one `UiFrame` triple read per pull (`spectrum_on`, under the services lock, caching telemetry + bands in `FrameSnapshot`); `telemetry_live` serves from that cache only.

**Why:** D-018 removed the APO but T16's runner, monitor, and visualizer were all built against its contract; replacing them with engine changes would break ownership boundaries. The shim realizes the APO's lease-reader side inside the app; the tap source keeps the engine's RT loop the mic's only capture client; the cached-snapshot meters path keeps `Triple`/`SpscRing` single-reader guarantees intact.

**Consequences:**
- The four formerly parked intents (endpoint switch, pin control, the whole Auto-Tune wizard) are routable live in the two-process mode (`Endpoint`/`Unpin`/`UnpinAll`/`Calibrate*` pipe requests, R-20 resolved); `tine calibrate` and the in-window wizard run the real 8-second flow against the live audio path (live smoke: Noise/Voice phases with real room levels, typed rejection `NotEnoughSpeech { voiced_ms: 0 }` on the silent 6 AM room — the honest outcome).
- During a calibration the meters spectrum stalls at the cached floor for ~9 s (the tap's single reader switches to the calibrator); playback of the cable continues (bypassed raw), matching APO-world behavior.
- "Hear Myself" monitors the *input* signal (processed-capture needs WASAPI render-loopback, outside T12's audio IO) — documented limitation.
- `tine.exe` grows to 21,627,392 bytes; the trims were measured and recorded (R-21) without touching the T01-owned budget.

### D-030 · 2026-11-17 · Accepted — Installer: WiX v5 MSI, per-machine (resolves Q-04; adapted to the D-018 virtual-cable topology)

**Decision:** The v0.1 installer is a **WiX v5 (`wix` CLI 5.0.2, .NET tool) per-machine x64 MSI** with a minimal `WixUI_InstallDir` UI, a fixed `UpgradeCode` + `MajorUpgrade` (upgrade = file replace + settings migration on the app side, no reboot), and — under D-018, there being **no APO DLL and no COM registration** — exactly these effects:

- copy `tine.exe` + the repo's `LICENSE-MIT` and `LICENSE-APACHE` texts (the "license texts bundled for the installer" of the spec — there is no NOTICE file) to `%ProgramFiles64Folder%\Tine\license\` alongside the exe;
- create `%ProgramData%\Tine\endpoints\` with a `util:PermissionEx` ACL — **Users: modify** — so the *unelevated* user-session app can persist per-endpoint boot params there (D-022); these are the "ProgramData ACLs" of the old spec, minus the LocalService reader that existed only for `audiodg`;
- an all-users Start-menu shortcut and the exit-dialog "Launch Tine now" checkbox (first-run onboarding, incl. virtual-cable validation);
- autostart per **D-025**: the MSI writes the `HKCU\…\Run` value `Tine` = `"<exe>" --background` (for the installing/elevating user) and removes it on uninstall;
- `util:CloseApplication` stops a running `tine.exe` so upgrades replace the file without a reboot;
- uninstall removes app files, shortcut and the `Run` key; **user data is kept by default** — `msiexec /x {ProductCode} REMOVESAVEDDATA=1` opts into deleting `%APPDATA%\Tine`, `%LOCALAPPDATA%\Tine` and `%ProgramData%\Tine` (WiX itself never prompts; "ask before deleting settings" = delete-never-by-default + an explicit, documented opt-in).

**Why:** Per-machine remains right even without an APO: only an elevated install can create the `ProgramData` folder with the Users-modify ACL cheaply, the Start-menu shortcut is for all users, and MSI gives the native Apps & Features uninstall path and transactional rollback. WiX v5 is the spec's recommendation, FOSS, scriptable in CI (`wix build`), and needed no system-level install (a `dotnet tool install --global wix`, user-profile only).

**Consequences:** `packaging/tine.wxs` is the single source of MSI truth; `packaging/build-msi.ps1` wraps build + SHA-256; `.github/workflows/release.yml` builds it on `v*` tags. Signing (SignPath Foundation / Azure Trusted Signing) applies to `tine.exe` and the MSI only (R-02) and is a maintainer runbook step; until then the beta ships unsigned with documented SmartScreen steps + published SHA-256 sums.

### D-031 · 2026-11-17 · Accepted — No update check in v0.1; manual "check Releases" until beta feedback (resolves Q-06)

**Decision:** v0.1 ships **no in-app update check**. The README documents checking GitHub Releases manually; every release carries its SHA-256 sums. The option is revisited after the beta (D-016) with real feedback; the natural seam (`version::tine_version()` / `tine status`) exists and stays untouched.

**Why:** The check would need the shell's first HTTP client (GitHub Releases API), which D-025 explicitly avoids adding (in-house shell utilities policy), inflates the R-21-stressed binary, and expands the attack surface of a process that autostarts for every user. The version seam already exists and a manual check costs the user one page load.

**Consequences:** The spec's "opt-in update check" step is dropped for v0.1 (documented in the task file's adapted steps and the README FAQ); if beta feedback demands it, it lands as an opt-in `settings.json` flag + one "new version available" tray notice, per the original Q-06 recommendation.

### D-032 · 2026-11-18 · Accepted — `cpal` stream errors are classified; benign xruns never rebuild a stream (T24)

**Decision:** `audio_io` error callbacks call `AtomicStreamStats::record_stream_error(kind)`. `ErrorKind::Xrun` (WASAPI `AUDCLNT_BUFFERFLAGS_DATA_DISCONTINUITY`), `DeviceChanged` (stream rerouted, still active) and `RealtimeDenied` (audio keeps running) increment `StreamStats::xruns`; every other kind latches `error_flag` as before. `tine status` reports `router.xruns`.

**Why:** cpal 0.18 delivers the discontinuity flag through the error callback. Latching it made the shell tear down and rebuild the routing loop twice on every start (125 rebuilds in one dev log), turned a one-buffer glitch into a rebuild gap, aborted `devhost record-raw`, and made SteelSeries Sonar's virtual mic unusable as an input (it raises the flag right after start).

**Consequences:** genuine device loss still rebuilds (unchanged recovery path). Xruns are visible as a counter instead of being invisible restarts.

### D-033 · 2026-11-18 · Accepted — `tine.exe` delay-loads its window-stack DLLs (T24)

**Decision:** `crates/tine-app/build.rs` links `tine.exe` (MSVC) with `/DELAYLOAD` for `dxgi.dll`, `dwrite.dll`, `uiautomationcore.dll`, `dwmapi.dll`, `imm32.dll` plus `delayimp.lib`.

**Why:** one binary hosts the always-on parent and the `--ui` child (D-017), so the parent statically imported DXGI. Overlay/OSD hook DLLs (RivaTuner was present on the dev machine) treat a DXGI-loaded process as a 3D app and set up a 51 MB private commit at its first window (the tray): parent Private Bytes 58 MB against a < 4 MB target. Bisected in-process to the first `CreateWindowExW`; the same block appeared in two unrelated DXGI-importing apps on that machine.

**Consequences:** parent Private Bytes 58 → 4.6 MB, Working Set ~16–18 MB; the `--ui` child loads the DLLs on first call (verified: window renders, DX12 + DXGI load in the child only). Binary 20.63 → 20.75 MiB (inside the R-23 24 MiB gate). A new eager import of a graphics DLL in the parent path would undo this — keep the list in `build.rs` in sync.

### D-034 · 2026-11-18 · Accepted — Listener gate: park the routing loop while nobody records the virtual mic (T24)

**Decision:** after the first successful route, a `tine-listener-gate` thread (own COM MTA) polls the audio sessions of the sink's capture side (`CABLE Input` ↔ `CABLE Output`, `listeners::capture_name_for_sink`). When no other process has an *active* capture session and nothing internal needs the signal (Hear Myself wanted/running, window meters pulled within 10 s, Auto-Tune or its lease) for 10 s continuously, the router stops ("parked"). Parked, it polls every 200 ms and resumes through the normal `router_follow` path on the first listener; Hear Myself, meters and Auto-Tune call `services::router_wake` synchronously. Any probe failure or missing capture endpoint counts as "listening".

**Why:** with no listener the loop was pure waste: ~1 % of a core in `tine.exe`, ~2 % in `audiodg.exe` for the cable streams, ~300 wakeups/s and the measured frame-pacing cost while gaming. Sonar already idles this way.

**Consequences:** an app that opens `CABLE Output` while Tine is parked gets audio after ≤ ~0.3 s (R-24). `status` reports `router.parked`. The tray/link stays "Active" while parked (routing is healthy, just idle). A user-facing opt-out is not in v0.1.

### D-035 · 2026-11-18 · Accepted — Start-up prime is 2× the steady jitter target, drained explicitly (T24, amends D-027)

**Decision:** `AdaptiveResampler` waits for `STARTUP_PRIME_FACTOR (2) × target` before the *first* render, then drains the surplus at a fixed `STARTUP_DRAIN_RATE` (+0.5 % playback speed, ~0.09 semitone, ~4 s) with the drift PI loop frozen; when the fill reaches the target the loop takes over from zeroed state. The steady-state target (D-027 / router 2 callbacks, monitor 3) is unchanged.

**Why:** when the cable sink already had an active reader, the shared engine drained a 1× prime in a burst: 1 underrun in 5 cold starts and in 2 of 3 resumes. A first attempt that let the PI loop drain the surplus failed the re-benchmark: its ±0.1 % clamp needs ~20 s, the integral winds up, and the fill then falls ~45 % below target for tens of seconds — 164 underruns between 20 s and 50 s after start under scheduler jitter (modelled and reproduced). The explicit drain has no windup and lands exactly on target.

**Consequences:** up to +20 ms latency for ~4 s after a (re)start only. Measured: 0 underruns in 8 cold starts with a listener, 0 in 5 park/resume cycles, 0/0/1 in three 90 s runs under sampler jitter (background level). Unit tests pin the doubled prime, burst absorption after priming, drain-without-undershoot over 90 simulated seconds, and unchanged honest-underrun semantics.
