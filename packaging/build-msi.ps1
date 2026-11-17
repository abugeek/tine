# Tine MSI build (T23). Wraps: cargo release build -> wix build -> SHA256SUMS.
# See packaging/README.md. Parameters:
#   -Version    product version (default: the Cargo workspace version)
#   -ReleaseDir where tine.exe lives (default: <repo>\target\release)
#   -OutDir     artifact dir (default: <repo>\packaging\out)
#   -SkipBuild  don't run cargo (use an existing tine.exe)
# Example: pwsh packaging/build-msi.ps1 -Version 0.1.0
[CmdletBinding()]
param(
    [string] $Version,
    [string] $ReleaseDir,
    [string] $OutDir,
    [switch] $SkipBuild
)

$ErrorActionPreference = 'Stop'

$RepoDir = Split-Path -Parent $PSScriptRoot
if (-not $Version) {
    $Version = (Select-String -Path (Join-Path $RepoDir 'Cargo.toml') -Pattern '^version = "(.+)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
}
if (-not $ReleaseDir) { $ReleaseDir = Join-Path $RepoDir 'target\release' }
if (-not $OutDir)     { $OutDir     = Join-Path $RepoDir 'packaging\out' }
$Exe = Join-Path $ReleaseDir 'tine.exe'
$MsiName = "tine-$Version-x64.msi"
$Msi = Join-Path $OutDir $MsiName

Write-Host "== Tine MSI build: $Version =="
if (-not $SkipBuild) {
    Write-Host "== cargo build --release -p tine-app =="
    Push-Location $RepoDir
    try {
        cargo build --release -p tine-app
        if ($LASTEXITCODE -ne 0) { throw "cargo build failed ($LASTEXITCODE)" }
    }
    finally { Pop-Location }
}
if (-not (Test-Path $Exe)) { throw "missing $Exe (build with -SkipBuild off or set -ReleaseDir)" }

$WixExe = (Get-Command wix -ErrorAction SilentlyContinue).Source
if (-not $WixExe) {
    $userTools = Join-Path $env:USERPROFILE '.dotnet\tools\wix.exe'
    if (Test-Path $userTools) { $WixExe = $userTools }
    else { throw "wix CLI not found; run: dotnet tool install --global wix --version 5.*" }
}
# WiX 5.x targets an older .NET runtime than may be installed; roll forward.
if (-not $env:DOTNET_ROLL_FORWARD) { $env:DOTNET_ROLL_FORWARD = 'LatestMajor' }

# Extensions install globally into the user profile (~/.wix/extensions), once.
# Versions are pinned to WiX v5-compatible extension builds (5.0.2).
$WixExtDir = Join-Path $env:USERPROFILE '.wix\extensions'
if (-not (Test-Path (Join-Path $WixExtDir 'WixToolset.UI.wixext'))) {
    Write-Host "== wix extension add -g (UI + Util 5.0.2) =="
    & $WixExe extension add -g WixToolset.UI.wixext/5.0.2
    if ($LASTEXITCODE -ne 0) { throw "wix extension add UI failed ($LASTEXITCODE)" }
    & $WixExe extension add -g WixToolset.Util.wixext/5.0.2
    if ($LASTEXITCODE -ne 0) { throw "wix extension add Util failed ($LASTEXITCODE)" }
}

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
if (Test-Path $Msi) { Remove-Item $Msi -Force }
Write-Host "== wix build $MsiName =="
& $WixExe build -arch x64 `
    -d "Version=$Version" `
    -d "ReleaseDir=$ReleaseDir" `
    -d "RepoDir=$RepoDir" `
    -ext WixToolset.UI.wixext -ext WixToolset.Util.wixext `
    (Join-Path $PSScriptRoot 'tine.wxs') `
    -o $Msi
if ($LASTEXITCODE -ne 0) { throw "wix build failed ($LASTEXITCODE)" }

# Fast gate on the binary budget (release gate; the main 20 MB budget debate
# is tracked as R-21 and stays with the T01-owned ci.yml gate).
$ExeSize = (Get-Item $Exe).Length
"{0} {1} = {2:N0} bytes ({3:N2} MiB)" -f 'tine.exe', $Version, $ExeSize, ($ExeSize / 1MB) | Write-Host
if ($ExeSize -ge 24MB) { throw "tine.exe is $ExeSize bytes, over the 24 MiB release gate (R-21)" }

$Sums = Join-Path $OutDir 'SHA256SUMS.txt'
$hash = (Get-FileHash -Algorithm SHA256 $Msi).Hash
"$hash  $MsiName" | Set-Content -Encoding ascii $Sums
Write-Host "== done: $Msi =="
Write-Host "== SHA256: $hash =="
