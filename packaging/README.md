# Packaging — Tine MSI (T23)

This folder builds the official Windows installer: a **WiX v5, per-machine x64 MSI**
(decision [D-030](../docs/project/06-decisions.md)). Under the D-018 virtual-cable
topology **it contains no APO DLL and registers no COM classes** — it ships
`tine.exe`, the dual-license texts, a `%ProgramData%\Tine\endpoints\` ACL for the
unelevated app's boot params (D-022), a Start-menu shortcut and the HKCU Run
autostart value (D-025).

## Files

| File | Purpose |
| :--- | :--- |
| `tine.wxs` | The single source of MSI truth (components, ACL, Run key, upgrade, data-removal). |
| `license.rtf` | Short dual-license notice for the MSI license dialog (full texts ship as files). |
| `build-msi.ps1` | One-shot local/ci build: `tine.exe` → `tine-<version>-x64.msi` → `SHA256SUMS.txt`. |

## Build locally (dry run)

```pwsh
# One-time, user profile only (no system state): a .NET SDK >= 6 is required.
dotnet tool install --global wix --version 5.*   # WiX v5 CLI
wix extension add WixToolset.UI.wixext WixToolset.Util.wixext

# Build the release binary and the MSI.
pwsh packaging/build-msi.ps1 -Version 0.1.0
```

Outputs `packaging/out/tine-<version>-x64.msi` plus `SHA256SUMS.txt`.

Notes:

- If the machine has no matching .NET runtime for the tool, set
  `DOTNET_ROLL_FORWARD=LatestMajor` (the script sets it when unset).
- `build-msi.ps1` installs the two WiX extensions globally into the user
  profile (`wix extension add -g` → `%USERPROFILE%\.wix\`) on first use;
  GitHub Actions installs them per-runner the same way
  (see `.github/workflows/release.yml`).

## Install / upgrade / uninstall (quick map)

Consumer-facing documentation of all three lives in the root `README.md`; here is
the MSI-side summary:

| Operation | Behavior |
| :--- | :--- |
| Install | Copy `tine.exe` + license texts to `%ProgramFiles%\Tine\`, create `%ProgramData%\Tine\endpoints\` (Users-modify), Start-menu shortcut, HKCU Run autostart; "Run Tine now" starts onboarding. |
| Upgrade (`MajorUpgrade`) | New MSI replaces files in place; settings/profiles migrate in-app (T15 schema), no reboot. |
| Uninstall | Remove app files, shortcut, `Run` value. User data (`%APPDATA%\Tine`, `%LOCALAPPDATA%\Tine`, `%ProgramData%\Tine`) is **kept** unless `msiexec /x {ProductCode} REMOVESAVEDDATA=1`. |

## MSI internals (review map)

| WiX element | Effect |
| :--- | :--- |
| `Package Scope="perMachine"` + `MajorUpgrade` | All-users install; upgrades replace files in place (settings migrate in-app, T15 schema), no reboot. |
| `util:CloseApplication tine.exe` | A running `tine.exe` is closed first, so upgrades don't need `RemoveFile` tricks. |
| `util:PermissionEx Users GenericAll` on `ProgramDataFolder\Tine\endpoints` | The unelevated user-session app can write per-endpoint boot params (D-022). |
| `RegistryValue HKCU\...\Run\Tine = "<exe>" --background` | Autostart, source of truth per D-025; removed on uninstall. |
| `WixUI_InstallDir` + `LaunchTine` CA + optional checkbox | Minimal UI, license dialog, "Run Tine now" exit checkbox → first-run onboarding. |
| `REMOVESAVEDDATA` `SetProperty`/`CustomAction` trio | `msiexec /x {ProductCode} REMOVESAVEDDATA=1` deletes `%APPDATA%\Tine`, `%LOCALAPPDATA%\Tine`, `%ProgramData%\Tine`. Default uninstall keeps them. |

Signing (R-02): a maintainer runbook item (SignPath Foundation or Azure Trusted
Signing), never part of this build script — see the task file's Result runbook.
