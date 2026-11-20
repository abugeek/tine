<p align="center"><img src="assets/brand/tine-128.png" width="96" height="96" alt="Tine icon"></p>

# Tine

**An ultra-lightweight, native, open-source microphone DSP engine for Windows.**
Tine makes *any* microphone — dynamic USB/XLR, condenser, headset, lapel,
webcam or laptop mic — sound broadcast-ready, without a gaming suite's bloat
or a studio mixer's complexity.

> **v0.1 beta.** Tine routes audio through a virtual audio cable (see
> [How it works](#how-it-works)). It does **not** install a Windows audio
> effect (APO) and registers nothing inside `audiodg.exe` — that approach was
> measured and rejected on stock Windows 11 (decision
> [D-018](docs/project/06-decisions.md)). The app runs in your user session,
> without elevation.

<!-- Demo GIF: to be added once a 1280x800 (~15 s, < 5 MB) screen capture is recorded at assets/demo.gif. -->

## Why

Consumer suites (SteelSeries GG Sonar) are heavy and closed; studio mixers
(Voicemeeter Potato) are complex. Tine is one small, fast, native binary:

- **Auto-Tune wizard**: 8 seconds of measurement and Tine adapts the whole
  chain (gate, EQ, dynamics, true-peak limiter, noise reduction) to *your*
  room, *your* voice, *your* mic.
- **3 selectable denoiser engines**: pure-Rust **RNNoise** (default, 21.58 ms),
  **Studio Clean** powered by embedded DeepFilterNet3 (41.58 ms), and **Low-Lat**
  (`1.58 ms` total DSP latency for competitive FPS gaming).
- **5-second A/B Mic Check & Raw-Mic App Detector**: record 5 seconds once to
  compare sample-aligned Raw vs. Tine playback in-app, plus a live banner
  warning you if an app is still recording your raw microphone instead of the
  virtual cable.
- **Two-tier control**: a one-line "Golden Line" for daily use, a full
  **Pro** rack when you want every parameter pinned.
- **Footprint**: a single native executable (~21–28 MB, under the 30 MiB release
  gate even with embedded DeepFilterNet3 weights); the always-on tray parent is
  GPU-free and idles at ~3.9–5.5 MB private bytes (0.16 % CPU when parked). No
  background services, no accounts.

## How it works

```text
 your mic ──► Tine (tine.exe, user session) ──► "CABLE Input" (virtual cable)
                   │                                   │
                   └─ tray + Rune window (on demand)   ▼
                                             Apps pick "CABLE Output"
                                             (Discord, OBS, CS2, browser…)
```

1. Tine captures your **real microphone**, runs the 7-stage DSP pipeline, and
   plays the processed audio into a [virtual audio
   cable](https://vb-audio.com/Cable/).
2. Your apps use the cable's **CABLE Output** as their microphone.
3. First-run onboarding validates the cable setup (and tells you exactly what
   to install if it's missing).

## Requirements

- Windows 10 22H2 or Windows 11, 64-bit.
- A virtual audio cable. **VB-Audio Virtual Cable** is the primary tested
  configuration, and Tine automatically discovers **VB-Audio Hi-Fi Cable**,
  **Cables A–D**, **VoiceMeeter VAIO/AUX**, and **Virtual Audio Cable (VAC)**.
  Onboarding and `tine apo status` verify the cable and show the exact capture
  device to select in your apps.
- A microphone (of course).

## Install

1. Download `tine-<version>-x64.msi` (or the portable ZIP) from
   [Releases](https://github.com/abugeek/tine/releases), or use the package
   manifests in [`packaging/winget/`](packaging/winget/) and
   [`packaging/scoop/`](packaging/scoop/).
2. (Recommended) verify the download against `SHA256SUMS.txt` (or verify the
   cryptographic build provenance attestation with
   `gh attestation verify tine-<version>-x64.msi -R abugeek/tine`):

   ```pwsh
   Get-FileHash .\tine-<version>-x64.msi -Algorithm SHA256
   ```

3. Verify (or install) a virtual audio cable with the helper script:

   ```pwsh
   pwsh -File scripts/setup-virtual-cable.ps1 -CheckOnly
   # or pass -Install to download and verify the official VB-Audio Virtual Cable installer:
   pwsh -File scripts/setup-virtual-cable.ps1 -Install
   ```

4. Run the MSI (per-machine: it needs one UAC prompt). It installs
   `tine.exe` to `C:\Program Files\Tine\`, adds a Start-menu shortcut, sets
   autostart (`HKCU\...\Run`, toggleable from the tray), and offers **Run Tine
   now**.
5. Onboarding: pick your mic → let the **Auto-Tune wizard** record you for
   ~8 seconds → **Apply**. Done: your apps can use the processed mic.
6. In Discord / OBS / your game, set the microphone to **CABLE Output
   (VB-Audio Virtual Cable)** (or the capture device shown by
   `tine apo status` / `setup-virtual-cable.ps1`). In Tine, always pick your
   *normal* microphone.

**SmartScreen note:** the beta is not yet code-signed, so Windows may show
"Windows protected your PC". Choose *More info → Run anyway*. The SHA-256
sums above let you verify exactly what you downloaded; public code signing
(SignPath Foundation or Azure Trusted Signing) is on the release runbook.

## Everyday use

- **Tray icon** (always on). The Rune window opens on demand and closes
  without leaving GPU memory behind.
- **Golden Line mode** — one slider for the overall body/purity feel.
  **Pro mode** — every stage's parameters, pin any of them against Auto-Tune.
- **Denoiser selector (`RNNoise` / `Studio Clean` / `Low-Lat`)**: switch live
  between pure-Rust RNNoise (`21.6 ms`), embedded DeepFilterNet3 (`Studio Clean`,
  `41.6 ms`, [D-037](docs/project/06-decisions.md)), or `Low-Lat` bypass
  denoiser (`1.58 ms` total DSP latency, [D-039](docs/project/06-decisions.md)).
- **5-Second Mic Check (A/B)**: record 5 seconds once and toggle
  sample-aligned Raw vs. Tine playback in-app.
- **Hear Myself**: monitor your **post-DSP processed microphone signal** in
  real time (`VirtualCableProcessor` post-DSP monitor tap), with automatic
  click-free mute and drift compensation.
- **Global Hotkeys**: `Ctrl+Shift+M` (click-free 5 ms output mute, while input
  meters keep showing your voice) and `Ctrl+Shift+B` (global A/B bypass).
- **Keep Warm (PTT)** (`tine ptt on|off`): optional toggle to disable idle
  listener parking for instant push-to-talk transmission.
- **CLI** (from a terminal):

  ```text
  tine status                            live state of the running instance (incl. autostart)
  tine diag [--out FILE]                 privacy-safe diagnostics bundle (no audio inside)
  tine mute [on|off|toggle]              click-free 5 ms output mute
  tine bypass on|off                     global A/B bypass
  tine denoise rnnoise|deepfilter|off    switch live denoiser engine
  tine ptt on|off                        Keep Warm (disable idle parking for push-to-talk)
  tine archetype <name>                  dynamic|condenser|headset|laptop
  tine macro <name> <val>                move a macro (purity|body|presence|leveler, 0.0..=1.0)
  tine set <key> <val>                   set one Pro parameter (SPECS key, pins it)
  tine monitor on|off [--device ID]      "Hear Myself" post-DSP monitoring
  tine calibrate                         run the 8-second Auto-Tune
  tine meters                            live telemetry + spectrum
  tine apo status|install|repair         verify/diagnose virtual-cable pairing
  tine --version
  ```

- **Autostart** is a per-user registry value (`"…\tine.exe" --background`);
  toggle it from the tray — `tine status` reports it live.

## Uninstall

Windows **Settings → Apps → Installed apps → Tine → Uninstall** (or
`appwiz.cpl`). The uninstaller removes `C:\Program Files\Tine`, the
Start-menu shortcut and the autostart value. **Your settings are kept** —
that's deliberate; delete them yourself if you want a full wipe:

```pwsh
# What a normal uninstall keeps:
#   %APPDATA%\Tine      (settings.json, profiles)  — per user
#   %LOCALAPPDATA%\Tine (logs)                     — per user
#   C:\ProgramData\Tine (per-endpoint boot params) — shared
Remove-Item -Recurse -Force "$env:APPDATA\Tine", "$env:LOCALAPPDATA\Tine"
```

Power users can ask the MSI to remove all of it in one go:
`msiexec /x {ProductCode} REMOVESAVEDDATA=1`. Virtual audio cable apps
(VB-Cable etc.) are third-party and are never touched.

## FAQ

**An app doesn't hear Tine.**
Apps that open the microphone in *exclusive mode* (some games and DAWs) bypass
the shared-mode capture path — point them at **CABLE Output** like everything
else. If that's not possible, Tine can't feed that particular app.

**Bluetooth headset?**
The Bluetooth hands-free (HFP) mic is 8/16 kHz and heavily squashed by the
stack; Tine will process it, but it can't restore what HFP throws away.
(Not part of the tested hardware matrix — beta feedback welcome.)

**My mic has vendor effects (Nahimic, Realtek, NVIDIA Broadcast).**
Under the virtual-cable topology Tine records the *physical* mic, so vendor
APOs still apply to what Tine receives. Disable them in the vendor app if you
want a completely clean feed (and file an issue with what you measured).

**Is it spyware? No.** Tine has no network code at all — not even an update
check. To update, watch [Releases](https://github.com/abugeek/tine/releases)
manually; every release lists SHA-256 sums. `tine diag` is privacy-safe (no
audio, FNV-1a-hashed device ids).

**Why ~22 ms of latency (and how to get 1.58 ms)?**
Added DSP latency depends on the active denoiser mode ([D-019](docs/project/06-decisions.md),
[D-037](docs/project/06-decisions.md), [D-039](docs/project/06-decisions.md)):
- **`Low-Lat` (`Off`)**: adds only **1.58 ms** (76 samples @ 48 kHz — limiter lookahead + true-peak FIR).
- **`RNNoise` (default)**: adds **21.58 ms** (1036 samples @ 48 kHz — 480-sample FIFO + 480-sample STFT synthesis + 76-sample limiter).
- **`Studio Clean` (DeepFilterNet3)**: adds **41.58 ms** (1996 samples @ 48 kHz).

**SmartScreen / "unknown publisher"?**
See the SmartScreen note under [Install](#install).

## Building from source

```pwsh
git clone https://github.com/abugeek/tine
cd tine
cargo build --release -p tine-app
# run it directly, or build the installer locally (WiX v5, user-profile only):
dotnet tool install --global wix --version 5.*
pwsh packaging\build-msi.ps1
```

Requirements: Rust 1.99+ (MSVC), the pinned toolchain from
`rust-toolchain.toml`, and the `[patch.crates-io]` entries already in the
workspace `Cargo.toml` (Rune's forked `winit`/`wgpu-hal`/`parley`/
`accesskit`). See [CONTRIBUTING.md](CONTRIBUTING.md) for the full workflow.

## License

Licensed under either of [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE)
at your option. The complete license texts are also shipped inside the
installer (`C:\Program Files\Tine\license\`).

Unless you explicitly state otherwise, any contribution intentionally
submitted for inclusion in the work by you, as defined in the Apache-2.0
license, shall be dual-licensed as above, without any additional terms or
conditions.

Optional "Studio Clean" noise suppression embeds DeepFilterNet3 weights via
[`dfn3_rs`](https://crates.io/crates/dfn3_rs) (`MIT OR Apache-2.0`).

## Code Signing & Build Provenance

Release builds run reproducibly on GitHub Actions and publish both `SHA256SUMS.txt`
and cryptographic [GitHub Build Provenance Attestations](https://docs.github.com/en/actions/security-for-github-actions/using-artifact-attestations/using-artifact-attestations-to-establish-provenance-for-builds)
so anyone can verify an artifact came directly from this repository (`gh attestation verify tine-<version>-x64.msi -R abugeek/tine`).

Our release pipeline already supports Authenticode signing (`TINE_SIGN_COMMAND`,
including Azure Artifact Signing), and we will reapply to
[SignPath Foundation](https://signpath.org) for free open-source Authenticode
signing once Tine reaches their public adoption milestone.
