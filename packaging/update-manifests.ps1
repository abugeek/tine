<#
.SYNOPSIS
    Updates Tine WinGet and Scoop package manifests for a release version and SHA-256 sums.
.PARAMETER Version
    Target semantic version (without leading 'v'), e.g. "0.1.0".
.PARAMETER Sha256SumsPath
    Path to SHA256SUMS.txt containing artifact hashes (e.g. packaging\out\SHA256SUMS.txt).
#>
[CmdletBinding()]
param(
    [string]$Version = "0.1.0",
    [string]$Sha256SumsPath = "packaging\out\SHA256SUMS.txt"
)

$ErrorActionPreference = "Stop"

if ($Version -notmatch '^\d+\.\d+\.\d+(-[\w.]+)?$') {
    throw "Invalid version '$Version'. Expected semantic version like '0.1.0'."
}

$RepoRoot = Split-Path -Parent $PSScriptRoot
$WingetDir = Join-Path $PSScriptRoot "winget"
$ScoopPath = Join-Path $PSScriptRoot "scoop\tine.json"

function Write-Utf8NoBom {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($Path, $Content, $utf8NoBom)
}

# Resolve SHA256SUMS.txt relative to current working directory or repo root
$ResolvedSumsPath = $null
if ($Sha256SumsPath) {
    if (Test-Path -LiteralPath $Sha256SumsPath -PathType Leaf) {
        $ResolvedSumsPath = (Resolve-Path -LiteralPath $Sha256SumsPath).Path
    }
    else {
        $Candidate = Join-Path $RepoRoot $Sha256SumsPath
        if (Test-Path -LiteralPath $Candidate -PathType Leaf) {
            $ResolvedSumsPath = (Resolve-Path -LiteralPath $Candidate).Path
        }
    }
}

$MsiHash = $null
$PortableZipHash = $null

if ($ResolvedSumsPath) {
    Write-Host "Reading checksums from $ResolvedSumsPath"
    foreach ($line in (Get-Content -LiteralPath $ResolvedSumsPath)) {
        $trimmed = $line.Trim()
        if (-not $trimmed) { continue }
        if ($trimmed -match '^(?i)([0-9a-f]{64})\s+\*?(tine-.*-x64\.msi)$') {
            $MsiHash = $Matches[1].ToUpperInvariant()
        }
        elseif ($trimmed -match '^(?i)([0-9a-f]{64})\s+\*?(tine-.*-portable-win-x64\.zip)$') {
            $PortableZipHash = $Matches[1].ToLowerInvariant()
        }
    }
}
else {
    Write-Host "Checksum file not found at '$Sha256SumsPath'; keeping existing hashes."
}

# Update WinGet YAML manifests
$WingetFiles = Get-ChildItem -Path $WingetDir -Filter "*.yaml" | Sort-Object Name
if (-not $WingetFiles) {
    throw "No WinGet manifest files found in $WingetDir"
}

$MsiUrl = "https://github.com/abugeek/tine/releases/download/v$Version/tine-$Version-x64.msi"

foreach ($file in $WingetFiles) {
    $content = [System.IO.File]::ReadAllText($file.FullName)
    $updated = $content -replace '(?m)^(PackageVersion:\s*)[^\r\n]+', "`${1}$Version"

    if ($file.Name -like "*.installer.yaml") {
        $updated = $updated -replace '(?m)^(\s*DisplayVersion:\s*)[^\r\n]+', "`${1}$Version"
        $updated = $updated -replace '(?m)^(\s*InstallerUrl:\s*)[^\r\n]+', "`${1}$MsiUrl"
        if ($MsiHash) {
            $updated = $updated -replace '(?m)^(\s*InstallerSha256:\s*)[^\r\n]+', "`${1}$MsiHash"
        }
    }

    Write-Utf8NoBom -Path $file.FullName -Content $updated
    Write-Host "Updated WinGet manifest: $($file.FullName)"
}

# Update Scoop manifest while preserving clean 2-space formatting across PS 5.1 / 7+
if (-not (Test-Path -LiteralPath $ScoopPath -PathType Leaf)) {
    throw "Scoop manifest not found at $ScoopPath"
}

$PortableUrl = "https://github.com/abugeek/tine/releases/download/v$Version/tine-$Version-portable-win-x64.zip"
$scoopText = [System.IO.File]::ReadAllText($ScoopPath)
$scoopText = $scoopText -replace '(?m)^(\s*"version":\s*)"[^"]*"', "`${1}`"$Version`""
$scoopText = $scoopText -replace '(?m)^(\s*"url":\s*)"https://github\.com/abugeek/tine/releases/download/v[^$"]+/tine-[^$"]+-portable-win-x64\.zip"', "`${1}`"$PortableUrl`""
if ($PortableZipHash) {
    $scoopText = $scoopText -replace '(?m)^(\s*"hash":\s*)"[0-9a-fA-F]{64}"', "`${1}`"$PortableZipHash`""
}

Write-Utf8NoBom -Path $ScoopPath -Content $scoopText

# Validate Scoop JSON parses cleanly and matches the target version/URL
$parsedScoop = Get-Content -LiteralPath $ScoopPath -Raw | ConvertFrom-Json
if ($parsedScoop.version -ne $Version) {
    throw "Scoop manifest validation failed: expected version '$Version', got '$($parsedScoop.version)'"
}
if ($parsedScoop.architecture.'64bit'.url -ne $PortableUrl) {
    throw "Scoop manifest validation failed: expected 64bit url '$PortableUrl', got '$($parsedScoop.architecture.'64bit'.url)'"
}
Write-Host "Validated Scoop manifest: $ScoopPath (version=$Version)"

if ($MsiHash) {
    Write-Host "MSI SHA-256: $MsiHash"
}
if ($PortableZipHash) {
    Write-Host "Portable ZIP SHA-256: $PortableZipHash"
}
exit 0
