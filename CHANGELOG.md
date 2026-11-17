# Changelog

All notable changes to Tine are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adapts [Semantic Versioning](https://semver.org/) (pre-1.0 versions may
still move faster; audio CLI/param contracts move only deliberately).

## [0.1.0] - Unreleased (beta)

First public beta. The complete engineering history lives in
[`docs/tasks/`](docs/tasks/); this section is the user-facing summary.

### Added — the app

- **`tine.exe`**: a single native binary (≈21 MB, no installer-time services)
  — tray-driven control plane that is always on, with the tuning window
  ("Rune" UI) opening on demand and freeing all GPU memory when closed
  (idle parent: 0 MB GPU, < 12 MB working set target).
- **First-run onboarding**: pick the microphone, validate the virtual-cable
  routing, run the **Auto-Tune wizard** (8-second measurement) and apply
  measured, verified corrections to the whole chain.
- **Golden Line vs Pro mode**: one-body macro control for daily use, or a
  full rack (48 kHz, per-stage parameters) with pinning against future
  Auto-Tune runs.
- **CLI**: `tine status` (live instance state incl. autostart), `tine diag`
  (privacy-safe stored-ZIP diagnostics: logs, device/format state; no audio,
  hashed device ids), `--version`.
- **Autostart**: registry-based (`HKCU\…\Run`), toggleable from the tray.
- **"Hear Myself" monitoring** of your microphone input on your headphones
  (v0.1 monitors the input signal; D-029).

### Added — audio & DSP

- **7-stage DSP pipeline** (mono f32, rate-agnostic with 48 kHz native):
  noise gate → spectral denoise (RNNoise-family, `nnnoiseless`) →
  high-pass → EQ/archetype curve → de-esser/harmonic control → compressor
  with makeup → true-peak limiter; bypass path time-aligned bit-exactly with
  the wet path.
- **Hardware archetypes** (dynamic, condenser, headset, laptop/webcam) as base
  curves, refined per-mic and per-room by Auto-Tune.
- **Virtual-cable output routing** (decision D-018): after measuring that
  stock Windows 11 will not load a third-party unsigned audio processing
  object (APO), v0.1 instead plays the processed mic into a virtual audio
  cable (e.g. VB-Cable). Apps pick "CABLE Output" as their microphone;
  nothing is registered inside `audiodg.exe` and no elevation is needed at
  runtime.
- **Fault-safe real-time core**: all audio callbacks allocation-free,
  lock-free and panic-free (enforced with `assert_no_alloc` and denied
  lints); any fault latches the pipeline to pass-through rather than
  dropping audio.

### Added — packaging & project

- **MSI installer** (WiX v5, per-machine): installs `tine.exe` + license
  texts, creates `C:\ProgramData\Tine\endpoints` (`Users`-modify ACL) for
  per-endpoint boot parameters, Start-menu shortcut, autostart value, and a
  "Run Tine now" exit option; upgrades replace files in place; uninstall
  keeps user data by default (`REMOVESAVEDDATA=1` removes it explicitly).
- **Release pipeline** (`.github/workflows/release.yml`): tag `v*` → clippy,
  full test suite, headless UI checks, release build, MSI, portable zip and
  PDB zip, SHA-256 sums, changelog-driven draft GitHub Release.
- **Open-source docs**: README (usage/FAQ), CONTRIBUTING (real-time rules,
  dependency/Rune policy), SECURITY (policy + architecture notes), Code of
  Conduct, issue templates, and this changelog; dual MIT OR Apache-2.0
  license texts bundled with the installer.

### Known limitations (by design, beta)

- ~22 ms added DSP latency at 48 kHz (the denoise stage's STFT framing —
  D-019); "Hear Myself" monitors the input signal with endpoint buffering
  latency. A low-latency mode (denoiser bypassed, < 2 ms added) is planned.
- Apps that open the microphone in exclusive mode bypass the shared capture
  path; they must use the virtual cable like everything else.
- No in-app update check (D-031) and beta binaries are unsigned (SmartScreen
  Flow documented in the README) until the signing runbook executes.
- The tested hardware matrix is dynamic USB/XLR + gaming headset; condenser,
  laptop, Bluetooth HFP and vendor-APO endpoints rely on beta feedback
  (`tine diag`).
