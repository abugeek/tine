# T28 — Documentation, Changelog, Security & Roadmap Release Synchronization

| Field | Value |
| :--- | :--- |
| Status | done |
| Lane | Z — Integration & ship |
| Depends on | T24 |
| Unblocks | v0.1 release |
| Owns | `README.md`, `CHANGELOG.md`, `SECURITY.md`, `docs/tasks/STATUS.md`, `docs/tasks/00-roadmap.md`, `docs/tasks/done/T24-perf-competitor-followup.md`, `docs/tasks/done/T28-docs-release-sync.md` |
| Size | S |

## Goal

Synchronize all user-facing and project-tracking documentation (`README.md`,
`CHANGELOG.md`, `SECURITY.md`, `docs/tasks/STATUS.md`, `docs/tasks/00-roadmap.md`,
and `docs/tasks/done/T24-perf-competitor-followup.md`) with decisions **D-032
through D-041** and the T24–T28 ship deliverables.

## Changes Made

1. **`README.md`**:
   - **`## Why`**: Added the 3 selectable denoiser engines (`RNNoise`, `Studio Clean` DeepFilterNet3, and `1.58 ms` `Low-Lat` mode), the 5-second A/B **Mic Check**, the **Raw-Mic App Detector** banner, and updated footprint metrics (`~21–28 MB` binary under the 30 MiB release gate, `~3.9–5.5 MB` idle private bytes, `0.16 %` parked CPU).
   - **`## Install`**: Documented `pwsh -File scripts/setup-virtual-cable.ps1 -CheckOnly` / `-Install`, WinGet (`packaging/winget/`) and Scoop (`packaging/scoop/`) manifests, and GitHub Build Provenance attestation verification (`gh attestation verify`).
   - **`## Everyday use`**: Replaced the outdated `"Hear Myself"` note with post-DSP processed microphone monitoring (`VirtualCableProcessor` post-DSP monitor tap with click-free mute and drift compensation); added bullets for the live denoiser selector, 5-second A/B Mic Check, global hotkeys (`Ctrl+Shift+M` / `Ctrl+Shift+B`), and **Keep Warm (PTT)** (`tine ptt on|off`); expanded the CLI block to document all commands (`status`, `diag`, `mute`, `bypass`, `denoise`, `ptt`, `archetype`, `macro`, `set`, `monitor`, `calibrate`, `meters`, `apo`, `--version`).
   - **`## FAQ`**: Updated the latency explanation to cover all three modes at 48 kHz (`Low-Lat` `Off`: `1.58 ms` / 76 samples; `RNNoise`: `21.58 ms` / 1036 samples; `Studio Clean` DeepFilterNet3: `41.58 ms` / 1996 samples).

2. **`CHANGELOG.md`**:
   - Updated `[0.1.0] - Unreleased (beta)` binary size (`≈21–28 MB`, `< 30 MiB` gate per D-037) and idle footprint (`~3.9–5.5 MB` private bytes via delay-loaded window-stack DLLs per D-033, `~0.16 %` CPU parked).
   - Added all D-032..D-041 features across `### Added — the app`, `### Added — audio & DSP`, and `### Added — packaging & project` (DeepFilterNet3 Studio Clean, `1.58 ms` Low-Lat mode, fast IEEE-754 dB math + VAD-assisted gate + 110 Hz compressor sidechain HPF, 5s A/B Mic Check modal, raw-mic app detector banner, post-DSP Hear Myself monitoring, event-driven WASAPI listener gate + Keep Warm PTT, click-free 5 ms RT mute, global hotkeys `Ctrl+Shift+M`/`Ctrl+Shift+B`, USB hot-plug & sleep/resume auto-recovery, multi-vendor virtual-cable auto-discovery for VB-Cable/Hi-Fi Cable/Cables A–D/VoiceMeeter/VAC, classified `cpal` xrun tracking, explicit 2× start-up prime drain, Ember brand mark & palette, pre-packaging signing order, GitHub Actions build provenance attestation, and WinGet/Scoop manifests + virtual-cable setup script).
   - Updated `### Known limitations (by design, beta)` to remove the obsolete claims that Hear Myself only monitors raw input and that low-latency mode was still planned.

3. **`SECURITY.md`**:
   - Updated `## Signing` to document that `.github/workflows/release.yml` publishes `SHA256SUMS.txt` alongside cryptographic GitHub Build Provenance Attestations (`actions/attest-build-provenance@v2`, verifiable with `gh attestation verify`) and references `D-041` for the SignPath Foundation re-application / Azure Artifact Signing path.

4. **Task & Roadmap Tracking**:
   - Copied `docs/tasks/T24-perf-competitor-followup.md` to `docs/tasks/done/T24-perf-competitor-followup.md` with `Status` set to `done (#32)`.
   - Updated `docs/tasks/STATUS.md` and `docs/tasks/00-roadmap.md` to mark `T24` (`#32`) and `T25`–`T28` as `done`.

## Verification

- Verified exact file ownership with `git status -sb` (only the 7 owned documentation files touched).
- Cross-checked every parameter, latency figure, hotkey, CLI subcommand, and decision reference against `docs/project/06-decisions.md` (`D-032`–`D-041`), `crates/tine-app/src/cli.rs`, and `.github/workflows/release.yml`.
