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
- **Two-tier control**: a one-line "Golden Line" for daily use, a full
  **Pro** rack when you want every parameter pinned.
- **Footprint**: a single ~21 MB executable; the always-on tray parent is
  GPU-free and targets under 12 MB of working memory at idle. No background
  services, no accounts.

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
- A virtual audio cable. VB-Audio Virtual Cable is the tested configuration;
  any WASAPI-compatible cable works. Onboarding checks for it.
- A microphone (of course).

## Install

1. Download `tine-<version>-x64.msi` from
   [Releases](https://github.com/abugeek/tine/releases).
2. (Recommended) verify the download against `SHA256SUMS.txt`:

   ```pwsh
   Get-FileHash .\tine-<version>-x64.msi -Algorithm SHA256
   ```

3. Run the MSI (per-machine: it needs one UAC prompt). It installs
   `tine.exe` to `C:\Program Files\Tine\`, adds a Start-menu shortcut, sets
   autostart (`HKCU\...\Run`, toggleable from the tray), and offers **Run Tine
   now**.
4. Onboarding: pick your mic → let the **Auto-Tune wizard** record you for
   ~8 seconds → **Apply**. Done: your apps can use the processed mic.
5. In Discord / OBS / your game, set the microphone to **CABLE Output
   (VB-Audio Virtual Cable)**. In Tine, always pick your *normal* microphone.

**SmartScreen note:** the beta is not yet code-signed, so Windows may show
"Windows protected your PC". Choose *More info → Run anyway*. The SHA-256
sums above let you verify exactly what you downloaded; public code signing
(SignPath Foundation or Azure Trusted Signing) is on the release runbook.

## Everyday use

- **Tray icon** (always on). The Rune window opens on demand and closes
  without leaving GPU memory behind.
- **Golden Line mode** — one slider for the overall body/purity feel.
  **Pro mode** — every stage's parameters, pin any of them against Auto-Tune.
- **Hear Myself**: monitor your microphone input on your headphones (in v0.1,
  Hear Myself monitors the input signal rather than the post-cable processed
  stream; see [D-029](docs/project/06-decisions.md)).
- **CLI** (from a terminal):

  ```text
  tine status        live state of the running instance (incl. autostart)
  tine diag [--out FILE]   privacy-safe diagnostics bundle (no audio inside)
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

**Why ~22 ms of latency?**
The denoiser's 480-sample STFT framing + synthesis adds two frames at 48 kHz
(decision [D-019](docs/project/06-decisions.md)); the rest of the chain adds
~1.6 ms. A "low-latency mode" (denoiser off, < 2 ms added) is a planned
option.

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

## Code Signing

Free code signing provided by [SignPath.io](https://signpath.io), certificate by [SignPath Foundation](https://signpath.org).
