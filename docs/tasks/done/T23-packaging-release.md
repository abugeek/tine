# T23 — Installer, Signing, Release, OSS Docs

| Field | Value |
| :--- | :--- |
| Status | done (t23-worker, Mistral Vibe; early start authorized — T22 QA gates only actual publishing) |
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
> **D-018 adaptation note:** this file predates the T02 NO-GO and D-018. As written it asks for `tine_apo.dll`, COM/FxProperties registration and DLL reloads; **none of that exists in v0.1** (virtual-cable routing, no APO, no COM). Each step below is checked off against its *adapted* intent:

- [x] Q-04 → **D-030** (WiX v5 MSI, per-machine, adapted to the virtual-cable topology) and Q-06 → **D-031** (no update check in v0.1; manual until beta feedback) recorded in `docs/project/06-decisions.md`; Q-04/Q-06 rows in `07-risks.md` marked resolved, R-02 updated.
- [x] Installer contents (D-030; **no APO DLL ships, nothing is COM-registered**):
  - `tine.exe` → `%ProgramFiles%\Tine\` — only one binary, so the "versioned DLL vs audiodg" concern disappears with the APO
  - dual-license texts → `%ProgramFiles%\Tine\license\` (the spec's "license texts bundled for the installer")
  - `%ProgramData%\Tine\endpoints\` with a **Users: modify** ACL (`util:PermissionEx`; the old "LocalService read" existed only for an `audiodg` reader, which D-018 removed); the unelevated app persists per-endpoint boot params there (D-022)
  - all-users Start-menu shortcut
  - "Run Tine now" exit-dialog checkbox → first-run onboarding (which validates virtual-cable setup)
- [x] Upgrade: `MajorUpgrade` replaces `tine.exe` in place; a running `tine.exe` is closed by `util:CloseApplication` (no audiodg can hold the file, so "delete old DLLs later" is moot); settings/profiles migrate in-app per the T15 schema (`migrate_value`, schema-stamped, out-of-range clamped).
- [x] Uninstall: removes app files, Start-menu entry and the HKCU `Run` value. Nothing was ever registered in HKLM audio keys, so "restore every backup" is vacuously true and `--elevated-helper uninstall-all` is an information-only no-op under D-018 (T17). "Ask before deleting settings" = delete-never-by-default: user data (`%APPDATA%\Tine`, `%LOCALAPPDATA%\Tine`, `%ProgramData%\Tine`) is kept unless the user explicitly runs `msiexec /x {ProductCode} REMOVESAVEDDATA=1` (three gated `cmd /c rmdir` custom actions; never on upgrades).
- [x] Signing applies to **`tine.exe` and the MSI only** (R-02; no DLL exists): exact maintainer runbook below (SignPath Foundation first choice, Azure Trusted Signing alternative, SHA-256 always); unsigned-beta SmartScreen steps documented in README §Install; the workflow signs before hashing when repo secrets are configured.
- [x] `release.yml`: on tag `v*` → clippy, full nextest, headless UI, release build (24 MiB gate per R-21), MSI, optional sign, SHA-256 (`SHA256SUMS.txt`), portable zip (dev) + PDB zip, changelog→notes from `CHANGELOG.md`, artifacts uploaded, **draft** GitHub Release (publishing stays with the maintainer). Cannot run: R-14 billing; reviewed-by-inspection artifact.
- [x] Docs: README (what/why, virtual-cable topology, install/uninstall, "pick your normal mic in Tine, pick CABLE Output in apps", FAQ with "apps that open the mic in exclusive mode" replacing the RAW-mode wording, Bluetooth, vendor effects, building from source, dual-license notice, GIF **placeholder** — capture is a maintainer action, noted inline); CONTRIBUTING (lanes/task files, RT rules, Rune bump, inbound = outbound D-015); SECURITY (D-018 posture — no audiodg involvement, security.txt-style contact placeholder, ProgramData ACL scope); CHANGELOG (0.1.0 seeded from `docs/tasks/done/*`).
- [x] `bug_report.yml` (asks for `tine status` + `tine diag`) and `mic-preset-request.yml`. **`beta-report.yml` deliberately absent** — owned by the in-flight T22 branch (D-016 beta flow). No update-check code per D-031 (the step's "if Q-06 says yes" resolved to no).

## Acceptance criteria
- Fresh Win11 VM: install → onboarding → Discord hears processed audio in < 2 min. Uninstall → `reg export` diff vs pre-install is empty, and audio works.
- An upgrade from the previous build keeps settings and works without a reboot.
- Pushing a tag produces signed (or documented-unsigned) artifacts with no manual steps.

## Verification
```bash
git tag v0.1.0-rc.1
git push origin v0.1.0-rc.1
```


## Result

**Status: done for everything automatable.** (Definition of done re-run after
rebasing on `main@6191ed7`: with the R-19 rustfmt deadlock fixed upstream (#28),
`cargo fmt --all --check` passes for the whole workspace; clippy
`--workspace --all-targets -- -D warnings` clean; `cargo nextest run --workspace`
**281 passed, 6 skipped**.) All artifacts were built and measured
locally — the WiX v5 CLI installed as a user-profile `dotnet tool` (no system
state changes beyond its own user-profile caches: `~/.dotnet/tools`,
`~/.wix/extensions`). Publishing, signing applications and tag pushing are the
maintainer's actions — the ordered runbook is below. The `release.yml` pipeline
cannot be exercised under R-14 (Actions billing) and is a reviewed-by-inspection
artifact mirroring `ci.yml`'s proven step shapes.

### Deliverables (this PR)

| Artifact | Evidence |
| :--- | :--- |
| `packaging/tine.wxs` | WiX v5 per-machine x64 MSI source (D-030); **no APO DLL, no COM registration** (D-018) |
| `packaging/build-msi.ps1` | build wrapper: `cargo build --release` -> `wix build` -> `SHA256SUMS.txt`; pins WiX 5.0.2 + UI/Util extensions 5.0.2; 24 MiB R-21 release gate |
| `packaging/license.rtf`, `packaging/README.md`, `packaging/inspect-msi.ps1` | MSI license-dialog notice; packaging doc + install/upgrade/uninstall map; MSI table inspection helper |
| `.github/workflows/release.yml` | tag `v*` -> clippy + nextest + headless UI -> release build -> MSI -> optional sign (repo secrets) -> SHA-256 -> portable zip + PDB zip -> changelog notes -> artifacts -> **draft** Release |
| `README.md` `CONTRIBUTING.md` `SECURITY.md` `CODE_OF_CONDUCT.md` `CHANGELOG.md` | user/contributor/security/conduct/changelog docs (details in the commit bodies) |
| `.github/ISSUE_TEMPLATE/{bug_report,mic-preset-request}.yml` | `beta-report.yml` intentionally NOT created (T22 owns it, in flight) |

### Measured evidence (dry run, this machine)

- `tine.exe` release build: **21,627,392 bytes (20.63 MiB)** — matches R-21,
  under the 24 MiB release gate (`release.yml`); the 20 MB CI-budget dispute
  stays with the T01 owner (R-21, untouched here).
- `tine-0.1.0-x64.msi` = **8,544,256 bytes**, SHA-256
  `C635DFCC6C68DF3BE3A767530D12AF11CF0ED15F43D4ABB336204419955673D3`
  (local build; expect a different hash from CI builds — the shipped
  `SHA256SUMS.txt` is generated per artifact build, after optional signing).
- MSI tables inspected via COM (`packaging/inspect-msi.ps1`, real output):
  - `Property`: `ALLUSERS=1` (per-machine), `ProductVersion 0.1.0`, `UpgradeCode
    {159D5692-2CCF-4639-8F1A-B4B4331F41BC}`, `WIXUI_INSTALLDIR=INSTALLDIR`,
    exit-dialog checkbox property = 1, `ARPNOREPAIR=1`, `ARPNOMODIFY=1`.
  - `Registry`: `HKCU\...\CurrentVersion\Run\Tine = "[INSTALLDIR]tine.exe" --background`
    (root=1) + three `HKLM\Software\Tine` marker keypaths.
  - `CustomAction`: `SetRemove*DataCmd` (type 51) + `RemoveRoamingData` /
    `RemoveLocalData` / `RemoveProgramData` (deferred, property-driven),
    `LaunchTine` (async, exit-dialog "Run Tine now"), `Wix4CloseApplications`
    scheduled (closes a running `tine.exe` so upgrades never need a reboot).
  - `InstallExecuteSequence`: data removal at 3501-3506, all conditioned on
    `REMOVE="ALL" AND REMOVESAVEDDATA AND NOT UPGRADINGPRODUCTCODE`.
  - `Component`/`Directory`: all `Bitness=always64`; `INSTALLDIR` under
    `ProgramFiles64Folder`, `TineProgramDataDir` under `CommonAppDataFolder`
    (WiX v5's ProgramData id) with `EndpointsDataDir`; the `util:PermissionEx
    Users GenericAll` on `endpoints\` is realized via the scheduled
    SchedSecureObjects/ExecSecureObjects pair.
  - `File`: `tine.exe` (21,627,392), `license_mit.txt`, `license_apache.txt` —
    nothing else ships; there is no `tine_apo.dll` in the MSI (verified).
- LOCAL INSTALL/UNINSTALL NOT RUN: the agent is not authorized to change
  system state, and a sibling's hardware soak is in flight on this machine.
  Verified by inspection of the sequenced tables instead; the one live pass
  (Apps & Features install, upgrade-over-installed, REMOVESAVEDDATA=1 wipe,
  `reg export` diffs) is runbook step B4, ~10 minutes.

### WiX v5 gotchas hit (for future packagers)

- Unpinned `wix extension add` resolves the *latest* (v7) extensions, which are
  WiX v6+ only -> `WIX6101`; pin `WixToolset.UI.wixext/5.0.2` etc.
- The plain `WixUI_InstallDir` dialog-set symbol is private (WIX0094); the
  exported, arch-suffixed symbol is `WixUI_InstallDir_X64` (found by
  extracting the extension's embedded wixlib IR).
- Element shapes differ from v3-era docs: launch condition is
  `<Launch Condition=...>`, `CommonAppDataFolder` is the ProgramData
  `StandardDirectory` Id, `SetProperty` takes `@Condition` (not inner text),
  XML comments may not contain `--`, and the property-driven deferred exe CA
  needs an explicit empty `ExeCommand=""` target.
- `wix extension list` is unreliable (rc=2 with a populated cache); the build
  script presence-checks the `%USERPROFILE%\.wix\extensions` directory instead.

### Acceptance criteria status

| Criterion | Status |
| :--- | :--- |
| Fresh Win11: install -> onboarding -> app hears processed mic < 2 min | Runbook B4 (maintainer; D-016: no spare PC/VM for this agent — community beta covers the hardware matrix) |
| Uninstall leaves the system exactly as it was | By construction (nothing COM/HKLM-audio registered, D-018); B4 includes the `reg export` / `fc` diff recipe |
| Upgrade from previous build keeps settings, no reboot | By construction (`MajorUpgrade` + `CloseApplication` + T15 in-app schema migration); B4 live pass |
| Tag -> reproducible artifacts, signed or documented-unsigned, no manual steps | Workflow complete by inspection; R-14 blocks a live run; signing = runbook section A; unsigned SmartScreen + SHA-256 documented in README |

### PUBLISHING RUNBOOK (maintainer actions, in order; each step is one command or one web action)

> Unsigned ordering: **B** produces a releasable, documented-unsigned beta
> (SmartScreen steps in README + SHA-256 sums always); **A** can land later and
> the same tag re-run for signed artifacts.
> Prerequisite for any Actions run at all: fix the R-14 billing/spending limit
> (GitHub Settings -> Billing; web action).

**A. One-time signing setup — SignPath Foundation (first choice, free for OSS)**

1. Apply: <https://signpath.org> -> "Open Source" (fill the web form with the
   repo URL; expect a manual review — no CLI for this step).
2. After approval, point the workflow's secrets at your SignPath project
   (signtool RemoteSign mode or the REST API; `<FILE>` is appended by the
   workflow):
   ```pwsh
   gh secret set TINE_SIGNING_ENABLED --body "true"
   gh secret set TINE_SIGN_COMMAND --body 'signtool sign /tr <TIMESTAMP_URL> /td sha256 /fd sha256 /d "Tine" /du "https://github.com/abugeek/tine" "<FILE>"'
   ```
3. Alternative (if SignPath is denied): Azure Trusted Signing — enroll at
   <https://portal.azure.com> -> Trusted Signing (identity validation ~1 day),
   then set the same secrets with:
   `signtool sign /atr <ENDPOINT>/<ACCOUNT_NAME>/<CERT_PROFILE> /td sha256 /fd sha256 "<FILE>"`.

**B. First release (v0.1.0) — after T22 QA passes and this PR is merged**

1. Stamp the changelog date:
   ```pwsh
   (Get-Content CHANGELOG.md) -replace '\[0\.1\.0\] - Unreleased \(beta\)','[0.1.0] - 2026-MM-DD' | Set-Content CHANGELOG.md
   git commit -am "chore(release): changelog date for 0.1.0"; git push
   ```
2. Tag and push (this triggers `release.yml`; it ends with a **draft** Release):
   ```pwsh
   git tag -a v0.1.0 -m "Tine 0.1.0 (beta)"
   git push origin v0.1.0
   ```
3. Watch it build (or debug it):
   ```pwsh
   gh run watch --exit-status $(gh run list --workflow=release.yml --limit 1 --json databaseId --jq '.[0].databaseId')
   ```
4. Live-verify the acceptance criteria (~10 min): install the draft MSI ->
   onboarding -> Discord on **CABLE Output**; uninstall -> registry diffs; one
   in-place upgrade with settings kept. Recipe:
   ```pwsh
   reg export "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" "$env:TEMP\run-before.reg" /y
   # ... install / onboarding / verify, then uninstall ...
   reg export "HKCU\Software\Microsoft\Windows\CurrentVersion\Run" "$env:TEMP\run-after.reg" /y
   fc.exe "$env:TEMP\run-before.reg" "$env:TEMP\run-after.reg"
   # expect: only the Tine Run line missing; settings.json kept at $env:APPDATA\Tine
   ```
5. Publish (the one true "publish" action):
   ```pwsh
   gh release edit v0.1.0 --draft=false --latest
   ```
6. Housekeeping after publishing (each one web action / small PR): replace the
   `security PLACEHOLDER @ example.invalid` contact in `SECURITY.md`; record
   `assets/demo.gif` to use the README's GIF placeholder (capture is a
   maintainer action); flip `docs/tasks/STATUS.md` (orchestrator-owned).

**C. Re-publishing signed binaries after A lands** (draft already deleted):

   ```pwsh
   gh release delete v0.1.0 --yes
   gh workflow run release.yml --ref v0.1.0 -f version=0.1.0
   ```
