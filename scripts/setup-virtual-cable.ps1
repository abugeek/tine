<#
.SYNOPSIS
    Checks for or installs a supported virtual audio cable for Tine on Windows.
.PARAMETER CheckOnly
    Only inspects and prints virtual audio cable status; always exits 0.
.PARAMETER Install
    Downloads, verifies Authenticode signatures, and launches the VB-Audio Virtual Cable installer if no supported cable is found.
#>
[CmdletBinding()]
param(
    [switch]$CheckOnly,
    [switch]$Install
)

$ErrorActionPreference = "Stop"

$VbCableUrl = "https://download.vb-audio.com/Download_CABLE/VBCABLE_Driver_Pack45.zip"
$TempDir = Join-Path $env:TEMP "tine-vbcable"

function Get-CaptureNameForSink {
    param([Parameter(Mandatory = $true)][string]$SinkName)

    if ($SinkName.Contains("Input")) {
        $idx = $SinkName.IndexOf("Input")
        return $SinkName.Substring(0, $idx) + "Output" + $SinkName.Substring($idx + 5)
    }

    $lower = $SinkName.ToLowerInvariant()
    $idxLower = $lower.IndexOf("input")
    if ($idxLower -ge 0) {
        return $SinkName.Substring(0, $idxLower) + "Output" + $SinkName.Substring($idxLower + 5)
    }

    return $SinkName
}

function Test-IsSupportedVirtualSinkName {
    param([string]$Name)
    if ([string]::IsNullOrWhiteSpace($Name)) { return $false }

    $lower = $Name.ToLowerInvariant()
    if ($lower.Contains("steelseries") -or $lower.Contains("sonar")) {
        return $false
    }

    return (
        $lower.Contains("cable input") -or
        $lower.Contains("hi-fi cable") -or
        $lower.Contains("voicemeeter input") -or
        $lower.Contains("voicemeeter aux input") -or
        $lower.Contains("virtual audio cable")
    )
}

function Test-IsRenderEndpoint {
    param($Device)
    # Windows MMDEVAPI instance IDs use {0.0.0.00000000} for render and {0.0.1.00000000} for capture.
    if ($Device.InstanceId -and $Device.InstanceId -match '\{0\.0\.1\.00000000\}') {
        return $false
    }
    return $true
}

Write-Host "== Tine Virtual Audio Cable Check =="

$soundDevices = @(Get-CimInstance Win32_SoundDevice -ErrorAction SilentlyContinue | Where-Object {
    $_.Name -match 'VB-Audio|VoiceMeeter|Virtual Audio Cable'
})

if ($soundDevices.Count -gt 0) {
    Write-Host "Virtual audio drivers (Win32_SoundDevice):"
    foreach ($dev in $soundDevices) {
        Write-Host ("  - {0} [{1}] (Status: {2})" -f $dev.Name, $dev.Manufacturer, $dev.Status)
    }
}

$audioEndpoints = @(Get-PnpDevice -Class AudioEndpoint -ErrorAction SilentlyContinue)

$renderEndpoints = @($audioEndpoints | Where-Object {
    (Test-IsRenderEndpoint $_) -and (Test-IsSupportedVirtualSinkName $_.FriendlyName)
})

$captureEndpoints = @($audioEndpoints | Where-Object {
    $_.InstanceId -and ($_.InstanceId -match '\{0\.0\.1\.00000000\}')
})

$detectedPairs = @()
foreach ($sink in $renderEndpoints) {
    $sinkName = $sink.FriendlyName
    # For Virtual Audio Cable, both render and capture share the same friendly name; exclude capture-only matches above.
    $expectedCapture = Get-CaptureNameForSink -SinkName $sinkName
    $matchedCapture = $captureEndpoints | Where-Object {
        $_.FriendlyName -and $_.FriendlyName.Equals($expectedCapture, [System.StringComparison]::OrdinalIgnoreCase)
    } | Select-Object -First 1

    $detectedPairs += [PSCustomObject]@{
        RenderSink          = $sinkName
        RenderStatus        = $sink.Status
        CaptureEndpoint     = if ($matchedCapture) { $matchedCapture.FriendlyName } else { $expectedCapture }
        CaptureStatus       = if ($matchedCapture) { $matchedCapture.Status } else { "Not found" }
        AppMicrophoneTarget = $expectedCapture
    }
}

# Fallback: if AudioEndpoint PnP query returned nothing (e.g. restricted environment), inspect Win32_SoundDevice OK entries
$hasUsableDriver = ($soundDevices | Where-Object { $_.Status -eq 'OK' }).Count -gt 0
$cableInstalled = ($detectedPairs.Count -gt 0) -or $hasUsableDriver

if ($detectedPairs.Count -gt 0) {
    Write-Host ""
    Write-Host "Detected virtual cable render/capture pairs:"
    foreach ($pair in $detectedPairs) {
        Write-Host ("  Render sink          : {0} (Status: {1})" -f $pair.RenderSink, $pair.RenderStatus)
        Write-Host ("  Capture endpoint     : {0} (Status: {1})" -f $pair.CaptureEndpoint, $pair.CaptureStatus)
        Write-Host ("  App microphone target: {0}" -f $pair.AppMicrophoneTarget)
        Write-Host ""
    }
    $primaryTarget = ($detectedPairs | Select-Object -First 1).AppMicrophoneTarget
    Write-Host "Status: Ready. In Discord, OBS, or games, select '$primaryTarget' as your microphone."
}
elseif ($hasUsableDriver) {
    Write-Host ""
    Write-Host "Status: Virtual audio driver detected (OK), though PnP AudioEndpoint pairs could not be enumerated."
    Write-Host "App microphone target: CABLE Output (VB-Audio Virtual Cable) (or your driver's Output endpoint)."
}
else {
    Write-Host ""
    Write-Host "Status: No supported virtual audio cable detected (checked CABLE Input, Hi-Fi Cable, VoiceMeeter Input, Virtual Audio Cable)."
}

if ($CheckOnly -or ($cableInstalled -and -not $Install)) {
    exit 0
}

if ($cableInstalled -and $Install) {
    Write-Host "A supported virtual audio cable is already installed. Skipping download."
    exit 0
}

if (-not $Install) {
    Write-Host ""
    Write-Host "To automatically download and install VB-Audio Virtual Cable (signed driver), run:"
    Write-Host "  pwsh -File scripts/setup-virtual-cable.ps1 -Install"
    exit 0
}

Write-Host ""
Write-Host "== Installing VB-Audio Virtual Cable =="
Write-Host "Downloading: $VbCableUrl"

if (Test-Path -LiteralPath $TempDir) {
    Remove-Item -LiteralPath $TempDir -Recurse -Force
}
New-Item -ItemType Directory -Path $TempDir -Force | Out-Null

$ZipPath = Join-Path $TempDir "VBCABLE_Driver_Pack45.zip"
$ExtractDir = Join-Path $TempDir "extracted"

[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12
Invoke-WebRequest -Uri $VbCableUrl -OutFile $ZipPath -UseBasicParsing

Write-Host "Extracting package to $ExtractDir..."
Expand-Archive -LiteralPath $ZipPath -DestinationPath $ExtractDir -Force

$SetupExe = Get-ChildItem -Path $ExtractDir -Filter "VBCABLE_Setup_x64.exe" -Recurse | Select-Object -First 1
if (-not $SetupExe) {
    throw "VBCABLE_Setup_x64.exe was not found in the extracted package at $ExtractDir"
}

Write-Host "Verifying Authenticode signatures..."
$FilesToVerify = @($SetupExe)
$CatFiles = @(Get-ChildItem -Path $ExtractDir -Filter "*.cat" -Recurse)
if ($CatFiles.Count -gt 0) {
    $FilesToVerify += $CatFiles
}

foreach ($file in $FilesToVerify) {
    $sig = Get-AuthenticodeSignature -FilePath $file.FullName
    if ($sig.Status -ne "Valid") {
        throw "Signature verification failed for '$($file.FullName)': Status=$($sig.Status) ($($sig.StatusMessage))"
    }
    Write-Host ("  Verified: {0} (Signer: {1})" -f $file.Name, $sig.SignerCertificate.Subject)
}

Write-Host "Launching $($SetupExe.Name) (Administrator elevation required)..."
try {
    $proc = Start-Process -FilePath $SetupExe.FullName -Verb RunAs -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Warning "Installer exited with code $($proc.ExitCode)."
    }
    else {
        Write-Host "Installer completed. A Windows reboot may be required before the virtual cable endpoint appears."
    }
}
catch {
    Write-Warning "Elevation was declined or installer could not be launched: $($_.Exception.Message)"
    Write-Host ""
    Write-Host "Manual installation instructions:"
    Write-Host "  1. Open an Administrator terminal or right-click:"
    Write-Host "     $($SetupExe.FullName)"
    Write-Host "  2. Click 'Install Driver' and reboot Windows if prompted."
    Write-Host "  3. Re-run: pwsh -File scripts/setup-virtual-cable.ps1 -CheckOnly"
}
exit 0
