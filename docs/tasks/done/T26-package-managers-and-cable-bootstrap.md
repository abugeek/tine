# T26 — WinGet & Scoop Package Manifests + Automated Virtual-Cable Setup Script

| Field | Value |
| :--- | :--- |
| Status | done (pkg-distribution) |
| Lane | Z — Ship |
| Depends on | T23 |
| Owns | `packaging/winget/abugeek.tine.yaml`, `packaging/winget/abugeek.tine.installer.yaml`, `packaging/winget/abugeek.tine.locale.en-US.yaml`, `packaging/scoop/tine.json`, `packaging/update-manifests.ps1`, `scripts/setup-virtual-cable.ps1`, `docs/tasks/done/T26-package-managers-and-cable-bootstrap.md` |
| Size | S |

## Goal

Provide ready-to-submit WinGet v1.6.0 and Scoop manifests synchronized with `packaging/tine.wxs` and `packaging/out/SHA256SUMS.txt`, an automated manifest updater script for releases, and a PowerShell virtual-audio-cable verification and bootstrap assistant for Windows users.

## Deliverables

- **WinGet v1.6.0 multi-file manifests (`packaging/winget/`)**:
  - `packaging/winget/abugeek.tine.yaml`: version manifest (`PackageIdentifier: abugeek.tine`, `PackageVersion: 0.1.0`, `DefaultLocale: en-US`, `ManifestVersion: 1.6.0`).
  - `packaging/winget/abugeek.tine.installer.yaml`: installer manifest (`InstallerType: wix`, `Scope: machine`, `MinimumOSVersion: 10.0.19045.0`, `UpgradeBehavior: install`, `AppsAndFeaturesEntries` with `UpgradeCode: '{159D5692-2CCF-4639-8F1A-B4B4331F41BC}'`, `x64` MSI URL, and SHA-256 `08CD2E0151A6B398AE2F9ABE9C45D7AAB131ADB49B32ED08FE64D2C9D2E04D71` from `packaging/out/SHA256SUMS.txt`).
  - `packaging/winget/abugeek.tine.locale.en-US.yaml`: defaultLocale manifest (`Publisher: Abdulloh`, `PackageName: Tine`, `License: MIT OR Apache-2.0`, description, and 9 audio/DSP tags).
- **Scoop manifest (`packaging/scoop/tine.json`)**:
  - `version: "0.1.0"`, `license: "MIT|Apache-2.0"`, 64-bit portable zip URL (`tine-0.1.0-portable-win-x64.zip`), 64-char lowercase hex hash, `bin: "tine.exe"`, Start Menu shortcut `["tine.exe", "Tine"]`, `checkver: "github"`, and `autoupdate` wired to `SHA256SUMS.txt`.
- **Manifest updater (`packaging/update-manifests.ps1`)**:
  - Accepts `-Version` (default `0.1.0`) and `-Sha256SumsPath` (default `packaging\out\SHA256SUMS.txt`), extracts MSI and optional portable zip SHA-256 hashes, updates `packaging/winget/*.yaml` and `packaging/scoop/tine.json` (UTF-8 without BOM), and validates `tine.json` via `ConvertFrom-Json`.
- **Virtual-cable bootstrap script (`scripts/setup-virtual-cable.ps1`)**:
  - Accepts `-CheckOnly` and `-Install`.
  - Queries `Win32_SoundDevice` and `Get-PnpDevice -Class AudioEndpoint` for `CABLE Input`, `Hi-Fi Cable`, `VoiceMeeter Input`, and `Virtual Audio Cable`, printing render/capture pairs and the exact `App microphone target` for Discord/OBS/games.
  - Exits `0` cleanly on `-CheckOnly` or when a cable is already present; with `-Install` and no cable installed, downloads `VBCABLE_Driver_Pack45.zip` to `$env:TEMP\tine-vbcable`, verifies Authenticode signatures via `Get-AuthenticodeSignature` on `.exe`/`.cat` files, and launches `VBCABLE_Setup_x64.exe` (printing manual instructions if UAC is declined).

## Verification

Note: `pwsh` (PowerShell 7+) is not installed on this workstation, so verification ran using Windows PowerShell 5.1 (`powershell -NoProfile -ExecutionPolicy Bypass -File ...`) plus `winget validate`.

1. `powershell -NoProfile -ExecutionPolicy Bypass -File packaging\update-manifests.ps1 -Version 0.1.0` (exit code `0`):
```text
Reading checksums from D:\projects\rust\tine\packaging\out\SHA256SUMS.txt
Updated WinGet manifest: D:\projects\rust\tine\packaging\winget\abugeek.tine.installer.yaml
Updated WinGet manifest: D:\projects\rust\tine\packaging\winget\abugeek.tine.locale.en-US.yaml
Updated WinGet manifest: D:\projects\rust\tine\packaging\winget\abugeek.tine.yaml
Validated Scoop manifest: D:\projects\rust\tine\packaging\scoop\tine.json (version=0.1.0)
MSI SHA-256: 08CD2E0151A6B398AE2F9ABE9C45D7AAB131ADB49B32ED08FE64D2C9D2E04D71
```

2. `powershell -NoProfile -ExecutionPolicy Bypass -File scripts\setup-virtual-cable.ps1 -CheckOnly` (exit code `0`):
```text
== Tine Virtual Audio Cable Check ==
Virtual audio drivers (Win32_SoundDevice):
  - VB-Audio Virtual Cable [VB-Audio Software] (Status: OK)
  - VB-Audio Virtual Cable [VB-Audio Software] (Status: Error)

Detected virtual cable render/capture pairs:
  Render sink          : CABLE Input (VB-Audio Virtual Cable) (Status: OK)
  Capture endpoint     : CABLE Output (VB-Audio Virtual Cable) (Status: OK)
  App microphone target: CABLE Output (VB-Audio Virtual Cable)

Status: Ready. In Discord, OBS, or games, select 'CABLE Output (VB-Audio Virtual Cable)' as your microphone.
```

3. `winget validate --manifest packaging\winget` (exit code `0`):
```text
Manifest validation succeeded.
```
