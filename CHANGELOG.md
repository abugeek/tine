# Changelog

All notable changes to Tine are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adapts [Semantic Versioning](https://semver.org/) (pre-1.0 versions may
still move faster; audio CLI/param contracts move only deliberately).

## [0.1.0] - 2026-11-20

First public beta. The complete engineering history lives in
[`docs/tasks/`](docs/tasks/); this section is the user-facing summary.

### Added — the app

- **`tine.exe`**: a single native binary (≈21–28 MB with embedded
  DeepFilterNet3 weights, < 30 MiB CI release gate per D-037; no installer-time
  services) — tray-driven control plane that is always on, with the tuning
  window ("Rune" UI, Ember brand mark & palette per D-036) opening on demand
  and freeing all GPU memory when closed (D-017; idle parent: 0 MB GPU,
  ~3.9–5.5 MB private bytes via delay-loaded window-stack DLLs per D-033,
  ~0.16 % CPU when parked).
- **First-run onboarding**: pick the microphone, validate the virtual-cable
  routing, run the **Auto-Tune wizard** (8-second measurement) and apply
  measured, verified corrections to the whole chain.
- **5-second A/B Mic Check modal**: record 5 seconds once and toggle
  sample-aligned Raw vs. Tine playback in-app (`MicCheckRunner`, D-039).
- **Raw-mic app detector banner**: surfaces active apps still capturing the
  physical microphone directly instead of the virtual cable (`raw_mic_apps`,
  D-039).
- **Golden Line vs Pro mode**: one-body macro control for daily use, or a
  full rack (48 kHz, per-stage parameters) with pinning against future
  Auto-Tune runs.
- **Post-DSP "Hear Myself" monitoring**: monitors your **post-DSP processed
  microphone signal** in real time via the `VirtualCableProcessor` post-DSP
  monitor tap, with automatic click-free mute and drift compensation.
- **Click-free 5 ms RT mute & global hotkeys**: lock-free 5 ms output mute
  ramp (`Ctrl+Shift+M`, input meters remain live) and global A/B bypass
  (`Ctrl+Shift+B`) via Win32 `RegisterHotKey` (D-040).
- **Event-driven WASAPI listener gate & Keep Warm (PTT)**: automatically parks
  the routing loop when no app records the virtual cable using
  `IAudioSessionNotification` / `IAudioSessionEvents` (D-034, D-039), with an
  optional **Keep Warm (PTT)** toggle (`tine ptt on|off`) to disable idle
  parking for instant push-to-talk transmission.
- **USB hot-plug & sleep/resume auto-recovery**: `DeviceEvent::StateChanged`
  and `PowerWatcher` recover streams cleanly after USB replugs or system
  sleep/resume (300 ms / 600 ms settle, D-040).
- **Multi-vendor virtual-cable auto-discovery**: discovers VB-Cable, Hi-Fi
  Cable, Cables A–D, VoiceMeeter VAIO/AUX, and Virtual Audio Cable (VAC)
  automatically and pairs the corresponding capture device (D-041).
- **CLI**: `tine status` (live instance state incl. autostart & xruns),
  `tine diag` (privacy-safe stored-ZIP diagnostics: logs, device/format state,
  denoiser/PTT config, virtual-cable pairing; no audio, hashed device ids),
  `tine mute [on|off|toggle]`, `tine bypass on|off`,
  `tine denoise rnnoise|deepfilter|off`, `tine ptt on|off`,
  `tine archetype <name>`, `tine macro <name> <val>`, `tine set <key> <val>`,
  `tine monitor on|off [--device ID]`, `tine calibrate`, `tine meters`,
  `tine apo status|install|repair`, `--version`.
- **Autostart**: registry-based (`HKCU\…\Run`), toggleable from the tray.

### Added — audio & DSP

- **7-stage DSP pipeline** (mono f32, rate-agnostic with 48 kHz native):
  noise gate (with VAD-assisted threshold boost) → selectable spectral
  denoiser → high-pass → EQ/archetype curve → de-esser/harmonic control →
  compressor with 110 Hz sidechain HPF and makeup → true-peak limiter; bypass
  path time-aligned bit-exactly with the wet path.
- **3 selectable denoiser engines**:
  - **RNNoise** (default, `nnnoiseless`): 21.58 ms added latency (1036 samples @ 48 kHz, D-019).
  - **Studio Clean** (`dfn3_rs`, embedded DeepFilterNet3 weights): 41.58 ms added latency (1996 samples @ 48 kHz, D-037).
  - **Low-Lat** (`DenoiseEngine::Off`): bypasses the denoiser FIFO/resampler for **1.58 ms** total added DSP latency (76 samples @ 48 kHz, D-039).
- **Voice quality & performance core upgrades** (D-038): fast IEEE-754 dB
  math (`tine_dsp::primitives::db`), neural VAD-assisted gate threshold, and
  110 Hz Butterworth compressor sidechain HPF.
- **Hardware archetypes** (dynamic, condenser, headset, laptop/webcam) as base
  curves, refined per-mic and per-room by Auto-Tune.
- **Virtual-cable output routing** (decision D-018): after measuring that
  stock Windows 11 will not load a third-party unsigned audio processing
  object (APO), v0.1 instead plays the processed mic into a virtual audio
  cable (VB-Cable, Hi-Fi Cable, VoiceMeeter, VAC). Apps pick the cable's
  output (e.g. "CABLE Output") as their microphone; nothing is registered
  inside `audiodg.exe` and no elevation is needed at runtime.
- **Resilient streaming**: classified `cpal` stream errors (`xruns` counter
  without tearing down streams on benign discontinuities, D-032), explicit 2×
  start-up prime drain without PI windup (D-035), and fault-safe real-time
  callbacks (allocation-free, lock-free, panic-free; any fault latches
  pass-through).

### Added — packaging & project

- **MSI installer** (WiX v5, per-machine): installs `tine.exe` + license
  texts, creates `C:\ProgramData\Tine\endpoints` (`Users`-modify ACL) for
  per-endpoint boot parameters, Start-menu shortcut, autostart value, and a
  "Run Tine now" exit option; upgrades replace files in place; uninstall
  keeps user data by default (`REMOVESAVEDDATA=1` removes it explicitly).
- **Package manager manifests & cable bootstrap**: WinGet manifests
  (`packaging/winget/`), Scoop manifest (`packaging/scoop/tine.json`),
  manifest updater (`packaging/update-manifests.ps1`), and assisted
  virtual-cable verification/setup script (`scripts/setup-virtual-cable.ps1`).
- **Release pipeline** (`.github/workflows/release.yml`): tag `v*` → clippy,
  full test suite, headless UI checks, release build (< 30 MiB gate),
  pre-packaging `tine.exe` signing (D-040), MSI, portable zip and PDB zip,
  SHA-256 sums, cryptographic GitHub Build Provenance Attestations
  (`actions/attest-build-provenance@v2`, D-041), changelog-driven draft
  GitHub Release.
- **Open-source docs**: README (usage/FAQ), CONTRIBUTING (real-time rules,
  dependency/Rune policy), SECURITY (policy + architecture notes + provenance),
  Code of Conduct, issue templates, and this changelog; dual MIT OR Apache-2.0
  license texts bundled with the installer.

### Known limitations (by design, beta)

- Denoiser latency depends on the active mode at 48 kHz: **1.58 ms** in
  `Low-Lat` (`Off`), **21.58 ms** in `RNNoise` (D-019), and **41.58 ms** in
  `Studio Clean` (DeepFilterNet3, D-037).
- Apps that open the microphone in exclusive mode bypass the shared capture
  path; they must use the virtual cable like everything else.
- No in-app update check (D-031) and beta binaries are unsigned (SmartScreen
  flow + GitHub Build Provenance verification documented in README/SECURITY)
  until Authenticode signing lands.
- The tested hardware matrix is dynamic USB/XLR + gaming headset; condenser,
  laptop, Bluetooth HFP and vendor-APO endpoints rely on beta feedback
  (`tine diag`).
