<#
.SYNOPSIS
  Repeated, alternating A/B of game-like load with Tine on vs off, to tell a
  real frame-pacing effect apart from run-to-run noise. Restores Tine's
  original running state at the end.

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File scripts\bench\ab_game.ps1 -Rounds 5 -Seconds 40
#>
param([int]$Rounds = 5, [int]$Seconds = 40, [int]$Workers = 0, [string]$OutDir = "",
    # routing: an app records CABLE Output in BOTH arms (Tine processes audio when on).
    # parked:  nobody records and Hear Myself is off, so Tine parks (T24 listener gate).
    [ValidateSet('routing', 'parked')][string]$Mode = 'routing')
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$root = Resolve-Path (Join-Path $here '..\..')
$tine = Join-Path $root 'target\release\tine.exe'
if ($Workers -le 0) { $Workers = [Math]::Max(1, [Environment]::ProcessorCount - 2) }
if (-not $OutDir) { $OutDir = Join-Path $root ("target\bench\ab-" + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
New-Item -ItemType Directory -Force $OutDir | Out-Null
function Test-Tine { [bool](Get-Process -Name tine -ErrorAction SilentlyContinue) }
function Start-Tine { if (-not (Test-Tine)) { Start-Process $tine -ArgumentList '--background' -WindowStyle Hidden; Start-Sleep 5 } }
function Stop-Tine {
    if (Test-Tine) { python -I (Join-Path $here 'tine_pipe.py') quit | Out-Null }
    for ($i = 0; $i -lt 50 -and (Test-Tine); $i++) { Start-Sleep -Milliseconds 200 }
    if (Test-Tine) { Get-Process tine | Stop-Process -Force }
    Start-Sleep 2
}
$was = Test-Tine
$recorder = Join-Path (Resolve-Path (Join-Path $here '..\..')) 'target\bench-tools\release\mic-record.exe'
$monitorWas = $null
Start-Tine
$st = python -I (Join-Path $here 'tine_pipe.py') status | ConvertFrom-Json
if ($st.ok) { $monitorWas = [bool]$st.data.monitor }
if ($Mode -eq 'parked') { python -I (Join-Path $here 'tine_pipe.py') monitor off | Out-Null }
try {
    for ($r = 1; $r -le $Rounds; $r++) {
        # alternate which arm goes first to cancel thermal / drift order effects
        $arms = if ($r % 2) { @('on', 'off') } else { @('off', 'on') }
        foreach ($arm in $arms) {
            if ($arm -eq 'on') { Start-Tine } else { Stop-Tine }
            $rec = $null
            if ($Mode -eq 'routing') {
                $rec = Start-Process $recorder -ArgumentList '--input', '"CABLE Output"', '--seconds', ($Seconds + 5), 'NUL' -WindowStyle Hidden -PassThru
            } elseif ($arm -eq 'on') {
                Start-Sleep -Seconds 12   # let the listener gate park the router
            }
            $parked = $null
            if ($arm -eq 'on') { $parked = (python -I (Join-Path $here 'tine_pipe.py') status | ConvertFrom-Json).data.router.parked }
            Write-Host "round $r tine=$arm mode=$Mode parked=$parked"
            python -I (Join-Path $here 'cpu_stress.py') --seconds $Seconds --workers $Workers `
                --report (Join-Path $OutDir "r$r-$arm.json") | Out-Null
            if ($rec) { $rec | Stop-Process -Force -ErrorAction SilentlyContinue }
        }
    }
} finally {
    Start-Tine
    if ($null -ne $monitorWas) {
        python -I (Join-Path $here 'tine_pipe.py') monitor $(if ($monitorWas) { 'on' } else { 'off' }) | Out-Null
        Start-Sleep -Seconds 1
    }
    if (-not $was) { Stop-Tine }
}
python -I -c @"
import json,glob,os,statistics as st
d=r'$OutDir'
for arm in ('on','off'):
    rs=[json.load(open(f)) for f in sorted(glob.glob(os.path.join(d,f'r*-{arm}.json')))]
    k=lambda key:[r[key] for r in rs]
    print(arm, 'n=',len(rs), 'late>2ms', k('frames_late_over_2ms'), 'p99', k('late_ms_p99'), 'work', [round(x) for x in k('work_units_per_s_mean')],
          '| median late>2ms', st.median(k('frames_late_over_2ms')), 'median p99', st.median(k('late_ms_p99')), 'mean work', round(st.fmean(k('work_units_per_s_mean'))))
"@
