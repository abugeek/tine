<#
.SYNOPSIS
  Starts and stops tine.exe N times and reports the router's underrun / xrun
  counters a few seconds after each start (startup-glitch regression check).
  Restores Tine's original running state at the end.
#>
param([int]$Starts = 5, [int]$SettleSeconds = 4)
$ErrorActionPreference = 'Stop'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$tine = Join-Path (Resolve-Path (Join-Path $here '..\..')) 'target\release\tine.exe'
function Test-Tine { [bool](Get-Process -Name tine -ErrorAction SilentlyContinue) }
function Stop-Tine {
    if (Test-Tine) { python -I (Join-Path $here 'tine_pipe.py') quit | Out-Null }
    for ($i = 0; $i -lt 50 -and (Test-Tine); $i++) { Start-Sleep -Milliseconds 200 }
    if (Test-Tine) { Get-Process tine | Stop-Process -Force }
}
$was = Test-Tine
Stop-Tine
$rows = @()
for ($n = 1; $n -le $Starts; $n++) {
    Start-Process $tine -ArgumentList '--background' -WindowStyle Hidden
    Start-Sleep -Seconds $SettleSeconds
    $r = (python -I (Join-Path $here 'tine_pipe.py') status | ConvertFrom-Json).data.router
    $rows += [pscustomobject]@{ start = $n; callbacks = $r.callbacks; underruns = $r.underruns; xruns = $r.xruns; overruns = $r.overruns }
    Stop-Tine
    Start-Sleep -Milliseconds 500
}
$rows | Format-Table -AutoSize | Out-String | Write-Host
"total underruns: " + (($rows | Measure-Object underruns -Sum).Sum) + " over $Starts starts"
if ($was) { Start-Process $tine -ArgumentList '--background' -WindowStyle Hidden }
