<#
.SYNOPSIS
  Head-to-head resource benchmark: Tine vs. a competitor mic/audio suite
  (default: SteelSeries GG / Sonar), with no human in the loop.

.DESCRIPTION
  Runs a fixed list of scenarios (idle, active voice path, UI open/closed,
  game-like CPU load, saturated CPU, GPU load, and a Tine-off control) while
  sampler.py records per-app CPU, RAM, threads, context switches, GPU % and
  VRAM once per second. Consumers record each app's virtual mic so the mic
  path is live (like Discord listening) and so dropouts can be checked.
  report.py turns the run folder into summary.json + summary.md.

  The competitor is measured as found (it is not stopped). Tine is started
  and stopped by the script; its previous running state and the
  "Hear Myself" setting are restored at the end.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\bench\compare.ps1
  powershell -ExecutionPolicy Bypass -File scripts\bench\compare.ps1 -PhaseSeconds 30 -SkipCompetitorUi
#>
[CmdletBinding()]
param(
    [int]$PhaseSeconds = 60,
    [int]$WarmupSeconds = 6,
    [string]$OutDir = "",
    [string]$CompetitorName = "sonar",
    [string]$CompetitorProcesses = "steelseries*",
    [string]$CompetitorMic = "SteelSeries Sonar - Microphone",
    [string]$CompetitorUiExe = "C:\Program Files\SteelSeries\GG\SteelSeriesGG.exe",
    [string]$TineMic = "CABLE Output",
    [int]$GameWorkers = 0,
    [switch]$SkipCompetitorUi,
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Resolve-Path (Join-Path $here '..\..')
$tine = Join-Path $root 'target\release\tine.exe'
$toolsTarget = Join-Path $root 'target\bench-tools'
$micRecord = Join-Path $toolsTarget 'release\mic-record.exe'
$gpuStress = Join-Path $toolsTarget 'release\gpu-stress.exe'
$py = (Get-Command python -ErrorAction Stop).Source
$logical = [Environment]::ProcessorCount
if ($GameWorkers -le 0) { $GameWorkers = [Math]::Max(1, $logical - 2) }
if (-not $OutDir) { $OutDir = Join-Path $root ("target\bench\" + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
$OutDir = (Resolve-Path $OutDir).Path
$phaseFile = Join-Path $OutDir 'phase.txt'
$stopFile = Join-Path $OutDir 'stop.txt'

function Write-Step([string]$m) { Write-Host ("[{0}] {1}" -f (Get-Date -Format 'HH:mm:ss'), $m) }
function Invoke-Py([string[]]$a) { & $py -I @a }
function Get-TineStatus {
    $j = Invoke-Py @((Join-Path $here 'tine_pipe.py'), 'status') | Out-String
    try { return ($j | ConvertFrom-Json) } catch { return $null }
}
function Send-Tine([string[]]$words) { Invoke-Py (@((Join-Path $here 'tine_pipe.py')) + $words) | Out-Null }
function Test-TineRunning { return [bool](Get-Process -Name tine -ErrorAction SilentlyContinue) }
function Start-Tine {
    if (Test-TineRunning) { return }
    Start-Process -FilePath $tine -ArgumentList '--background' -WindowStyle Hidden | Out-Null
    for ($i = 0; $i -lt 50; $i++) {
        Start-Sleep -Milliseconds 200
        $s = Get-TineStatus
        if ($s -and $s.ok -and $s.data.router.on) { return }
    }
    Write-Warning 'Tine started but its router did not report on within 10 s'
}
function Stop-Tine {
    if (-not (Test-TineRunning)) { return }
    Send-Tine 'quit'
    for ($i = 0; $i -lt 50 -and (Test-TineRunning); $i++) { Start-Sleep -Milliseconds 200 }
    if (Test-TineRunning) { Get-Process -Name tine | Stop-Process -Force }
}
function Start-Recorder([string]$device, [string]$name, [int]$seconds) {
    $wav = Join-Path $OutDir "$name.wav"
    $out = Join-Path $OutDir "$name.rec.json"
    $err = Join-Path $OutDir "$name.rec.log"
    return Start-Process -FilePath $micRecord -ArgumentList @('--input', "`"$device`"", '--seconds', $seconds, "`"$wav`"") `
        -RedirectStandardOutput $out -RedirectStandardError $err -WindowStyle Hidden -PassThru
}

# Runs one phase: optional setup, warmup (unsampled), sampled window, teardown.
# $Recorders: hashtable label -> device. $Load: scriptblock returning processes to wait on.
function Invoke-Phase {
    param([string]$Name, [hashtable]$Recorders = @{}, [scriptblock]$Load = $null, [int]$Warmup = $WarmupSeconds)
    Write-Step "phase $Name ($PhaseSeconds s + $Warmup s warmup)"
    Set-Content -Path $phaseFile -Value 'warmup' -Encoding ascii
    $total = $PhaseSeconds + $Warmup
    $procs = @()
    foreach ($k in $Recorders.Keys) { $procs += Start-Recorder $Recorders[$k] "$Name.$k" $total }
    if ($Load) { $procs += & $Load $total }
    $before = if (Test-TineRunning) { Get-TineStatus } else { $null }
    Start-Sleep -Seconds $Warmup
    Set-Content -Path $phaseFile -Value $Name -Encoding ascii
    Start-Sleep -Seconds $PhaseSeconds
    Set-Content -Path $phaseFile -Value 'between' -Encoding ascii
    foreach ($p in $procs) { if ($p) { $p.WaitForExit(30000) | Out-Null } }
    $after = if (Test-TineRunning) { Get-TineStatus } else { $null }
    if ($before -and $after -and $before.ok -and $after.ok) {
        $b = $before.data.router; $a = $after.data.router
        $parked = [bool]$a.parked -or [bool]$b.parked
        $restarted = (-not $parked) -and ($a.callbacks -lt $b.callbacks)
        # Counters restart with the loop (parking / resume): report raw values then.
        $d = { param($k) if ($parked -or $restarted) { $a.$k } else { $a.$k - $b.$k } }
        [ordered]@{
            phase = $Name
            parked = [bool]$a.parked
            underruns = & $d 'underruns'
            overruns = & $d 'overruns'
            xruns = & $d 'xruns'
            callbacks = & $d 'callbacks'
            max_callback_us = $a.max_callback_us
            router_restarted = $restarted
            monitor = $after.data.monitor
        } | ConvertTo-Json | Set-Content -Path (Join-Path $OutDir "$Name.tine.json") -Encoding utf8
    }
    foreach ($k in $Recorders.Keys) {
        $wav = Join-Path $OutDir "$Name.$k.wav"
        Invoke-Py @((Join-Path $here 'audio_check.py'), $wav, '--json', (Join-Path $OutDir "$Name.$k.audio.json")) | Out-Null
    }
}

function Start-GameLoad([int]$workers, [string]$tag, [switch]$NoProbe) {
    return {
        param($secs)
        $a = @('-I', (Join-Path $here 'cpu_stress.py'), '--seconds', $secs, '--workers', $workers,
               '--report', (Join-Path $OutDir "$tag.cpu_stress.json"))
        if ($NoProbe) { $a += '--no-probe' }
        Start-Process -FilePath $py -ArgumentList $a -WindowStyle Hidden -PassThru
    }.GetNewClosure()
}

# ---------------------------------------------------------------- prepare
Write-Step "output: $OutDir"
if (-not $SkipBuild) {
    Write-Step 'building tine (release) and bench tools'
    Push-Location $root
    try {
        cargo build --release -p tine-app -p tine-devhost --quiet
        if ($LASTEXITCODE -ne 0) { throw 'tine build failed' }
        cargo build --release --quiet --manifest-path (Join-Path $here 'tools\Cargo.toml') --target-dir $toolsTarget
        if ($LASTEXITCODE -ne 0) { throw 'bench tools build failed' }
    } finally { Pop-Location }
}
foreach ($f in $tine, $micRecord, $gpuStress) { if (-not (Test-Path $f)) { throw "missing $f (run without -SkipBuild)" } }

$tineWasRunning = Test-TineRunning
$monitorWas = $null
if ($tineWasRunning) { $s = Get-TineStatus; if ($s.ok) { $monitorWas = [bool]$s.data.monitor } }

$cpu = (Get-CimInstance Win32_Processor | Select-Object -First 1)
[ordered]@{
    started = (Get-Date).ToString('o')
    machine = @{
        cpu = $cpu.Name.Trim(); cores = $cpu.NumberOfCores; logical = $logical
        ram_gb = [Math]::Round((Get-CimInstance Win32_ComputerSystem).TotalPhysicalMemory / 1GB, 1)
        gpu = ((Get-CimInstance Win32_VideoController | ForEach-Object Name) -join '; ')
        os = (Get-CimInstance Win32_OperatingSystem).Caption + ' ' + [Environment]::OSVersion.Version.ToString()
        power_plan = ((powercfg /getactivescheme) -join ' ').Trim()
    }
    competitor = @{ name = $CompetitorName; processes = $CompetitorProcesses; mic = $CompetitorMic }
    tine = @{ exe = $tine; size_bytes = (Get-Item $tine).Length; mic = $TineMic }
    phase_seconds = $PhaseSeconds; warmup_seconds = $WarmupSeconds; game_workers = $GameWorkers
    tine_was_running = $tineWasRunning; tine_monitor_was = $monitorWas
} | ConvertTo-Json -Depth 4 | Set-Content -Path (Join-Path $OutDir 'meta.json') -Encoding utf8

Remove-Item $stopFile -ErrorAction SilentlyContinue
Set-Content -Path $phaseFile -Value 'start' -Encoding ascii
$sampler = Start-Process -FilePath $py -WindowStyle Hidden -PassThru -ArgumentList @(
    '-I', (Join-Path $here 'sampler.py'), '--out', (Join-Path $OutDir 'samples.csv'),
    '--phase-file', $phaseFile, '--stop-file', $stopFile,
    '--group', 'tine=tine.exe', '--group', "$CompetitorName=$CompetitorProcesses",
    '--group', 'audiodg=audiodg.exe', '--group', 'harness=mic-record.exe,gpu-stress.exe')

$recBoth = @{ tine = $TineMic; $CompetitorName = $CompetitorMic }
try {
    # 1-2: competitor alone (Tine stopped)
    Stop-Tine
    Invoke-Phase 'competitor_idle'
    Invoke-Phase 'competitor_active' -Recorders @{ $CompetitorName = $CompetitorMic }

    # 3-5: Tine background (window closed), then live mic path, then without Hear Myself
    Start-Tine
    $s = Get-TineStatus
    $monitorOn = ($s -and $s.ok -and $s.data.monitor)
    Invoke-Phase 'tine_idle'
    Invoke-Phase 'both_active' -Recorders $recBoth
    if ($monitorOn) {
        Send-Tine 'monitor','off'
        Invoke-Phase 'both_active_tine_monitor_off' -Recorders $recBoth
    } else {
        Send-Tine 'monitor','off'
    }
    # T24 listener gate: no app records CABLE Output and Hear Myself is off,
    # so Tine parks its routing loop after 10 s (warmup covers that).
    Invoke-Phase 'tine_parked' -Warmup 15
    if ($monitorOn) { Send-Tine 'monitor','on' }

    # 6-7: Tine window open, then closed again (VRAM must return to 0)
    Send-Tine 'open'
    Invoke-Phase 'tine_ui_open' -Recorders $recBoth
    Get-Process -Name tine -ErrorAction SilentlyContinue |
        Where-Object { $_.MainWindowHandle -ne 0 } | ForEach-Object { $_.CloseMainWindow() | Out-Null }
    Start-Sleep -Seconds 3
    Invoke-Phase 'tine_ui_closed' -Recorders $recBoth

    # 8: competitor window open
    if (-not $SkipCompetitorUi -and (Test-Path $CompetitorUiExe)) {
        Start-Process -FilePath $CompetitorUiExe | Out-Null
        Invoke-Phase 'competitor_ui_open' -Recorders $recBoth
        Get-Process | Where-Object { $_.Name -like ($CompetitorProcesses -replace '\*$', '*') -and $_.MainWindowHandle -ne 0 } |
            ForEach-Object { $_.CloseMainWindow() | Out-Null }
        Start-Sleep -Seconds 3
    }

    # 9-11: under load
    Invoke-Phase 'cpu_game_load' -Recorders $recBoth -Load (Start-GameLoad $GameWorkers 'cpu_game_load')
    Invoke-Phase 'cpu_saturated' -Recorders $recBoth -Load (Start-GameLoad $logical 'cpu_saturated' -NoProbe)
    Invoke-Phase 'gpu_game_load' -Recorders $recBoth -Load ({
        param($secs)
        Start-Process -FilePath $gpuStress -ArgumentList @($secs, '--report', "`"$(Join-Path $OutDir 'gpu_game_load.gpu_stress.json')`"") -WindowStyle Hidden -PassThru
    }.GetNewClosure())

    # 12: control - same game load with Tine stopped (game-impact A/B)
    Stop-Tine
    Invoke-Phase 'cpu_game_load_tine_off' -Recorders @{ $CompetitorName = $CompetitorMic } -Load (Start-GameLoad $GameWorkers 'cpu_game_load_tine_off')
}
finally {
    Set-Content -Path $stopFile -Value 'stop' -Encoding ascii
    if ($sampler) { $sampler.WaitForExit(10000) | Out-Null }
    Get-Process -Name mic-record, gpu-stress -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    # restore Tine to how we found it
    if ($tineWasRunning) {
        Start-Tine
        if ($null -ne $monitorWas) {
            Send-Tine 'monitor', $(if ($monitorWas) { 'on' } else { 'off' })
        }
    } else { Stop-Tine }
    Write-Step 'restored Tine state'
}

Write-Step 'building report'
Invoke-Py @((Join-Path $here 'report.py'), $OutDir)
Write-Step "done: $OutDir\summary.md"
