# T23 — Installer, Signing, Release, OSS Docs

| Field | Value |
| :--- | :--- |
| Status | todo |
| Lane | Z — Ship |
| Depends on | T22 |
| Unblocks | v0.1 release |
| Owns | `packaging/**`, `.github/workflows/release.yml`, `README.md`, `CONTRIBUTING.md`, `SECURITY.md`, `CODE_OF_CONDUCT.md`, `CHANGELOG.md`, `.github/ISSUE_TEMPLATE/**` |
| Size | M |

## Goal
A user downloads one installer from GitHub Releases, installs, and reaches a better-sounding mic in under 2 minutes. Uninstalling leaves the system exactly as it was. Releases are reproducible from a tag.

## Read first
D-003 · D-015 (license) · Q-04, Q-06 · R-02 · T02/T14 results (registration, uninstall-all)

## Steps
- [ ] Resolve Q-04 (installer tech; recommendation: WiX v5 MSI, per-machine) and Q-06 (update check); record decisions.
- [ ] Installer contents:
  - `tine.exe` and `tine_apo.dll` → `%ProgramFiles%\Tine\` (DLL file name versioned, e.g. `tine_apo_0_1_0.dll`, so upgrades never fight a DLL loaded by audiodg)
  - `%ProgramData%\Tine\` with ACLs (LocalService read, Users modify on `endpoints\`)
  - Start-menu shortcut
  - launch `tine.exe` at the end (first-run onboarding)
- [ ] Upgrade:
  - register the new versioned DLL path under the same CLSID
  - reload endpoints (T02 procedure)
  - delete old DLLs on the next start once no longer loaded
  - settings migrate (T15 schema)
- [ ] Uninstall: `tine.exe --elevated-helper uninstall-all` (custom action) → restore every backup → remove files and HKCU Run → ask before deleting user settings.
- [ ] Signing:
  - apply to SignPath Foundation (free for OSS) or use Azure Trusted Signing
  - sign `tine.exe`, `tine_apo.dll` and the MSI
  - until signing is available, document the SmartScreen steps and publish SHA-256 sums
- [ ] `release.yml`: on tag `v*` →
  - build `--release` (no `tooling`)
  - run the full test suite and headless UI
  - build the MSI and sign it
  - generate SHA-256
  - upload the MSI, a portable zip (dev use only) and the PDB zip
  - changelog from `CHANGELOG.md`
- [ ] Docs:
  - **README:** what/why, GIF, install/uninstall, how to pick Tine in Discord/OBS ("pick your normal mic"), FAQ (RAW-mode apps, Bluetooth, vendor effects), building from source
  - **CONTRIBUTING:** lanes, task files, RT rules, how to bump Rune, and the inbound = outbound licensing note (D-015)
  - **License:** README section with the standard `MIT OR Apache-2.0` dual-license notice; license texts bundled in the installer
  - **SECURITY:** runs inside audiodg; how to report
  - **Issue templates:** bug report with `tine status` output; new mic preset request
- [ ] Opt-in update check (if Q-06 says yes): the GitHub Releases API, at most once a day, no identifiers sent, user-visible toggle.

## Acceptance criteria
- Fresh Win11 VM: install → onboarding → Discord hears processed audio in < 2 min. Uninstall → `reg export` diff vs pre-install is empty, and audio works.
- An upgrade from the previous build keeps settings and works without a reboot.
- Pushing a tag produces signed (or documented-unsigned) artifacts with no manual steps.

## Verification
```bash
git tag v0.1.0-rc.1
git push origin v0.1.0-rc.1
```
