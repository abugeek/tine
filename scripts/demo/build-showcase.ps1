<#
.SYNOPSIS
    Generates Tine's reproducible audio A/B showcase WAVs and SHOWCASE-REPORT.md.

.DESCRIPTION
    Runs `tine-dsp`'s `process_wav` example in release mode (with `deepfilter`
    enabled) across five representative fixtures/presets/denoisers, verifies that
    no pass-through latch occurred, computes SHA-256 hashes for every output WAV,
    and writes `target/showcase/SHOWCASE-REPORT.md`.
#>

[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$FixturesDir = Join-Path $RepoRoot 'crates\tine-dsp\tests\fixtures'
$OutDir = Join-Path $RepoRoot 'target\showcase'
$ReportPath = Join-Path $OutDir 'SHOWCASE-REPORT.md'

New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

# Pre-build once so individual runs have minimal Cargo overhead, while still
# executing each case via `cargo run --release -p tine-dsp --features deepfilter --example process_wav`.
Push-Location $RepoRoot
try {
    Write-Host 'Building tine-dsp process_wav example (release + deepfilter)...'
    $prevBuildEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & cargo build --release -p tine-dsp --features deepfilter --example process_wav
    $buildExitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevBuildEap
    if ($buildExitCode -ne 0) {
        throw "cargo build failed with exit code $buildExitCode"
    }

    $cases = @(
        @{
            Name     = 'Noisy Quiet (RNNoise)'
            Input    = 'noisy_quiet.flac'
            Output   = 'noisy_quiet_rnnoise.wav'
            Preset   = 'laptop'
            Denoiser = 'rnnoise'
        },
        @{
            Name     = 'Noisy Quiet (Studio Clean / DeepFilterNet3)'
            Input    = 'noisy_quiet.flac'
            Output   = 'noisy_quiet_studio_clean.wav'
            Preset   = 'laptop'
            Denoiser = 'deepfilter'
        },
        @{
            Name     = 'Dynamic Speech (RNNoise)'
            Input    = 'dynamic_speech.flac'
            Output   = 'dynamic_speech_rnnoise.wav'
            Preset   = 'dynamic'
            Denoiser = 'rnnoise'
        },
        @{
            Name     = 'Dynamic Speech (Low-Latency / Off)'
            Input    = 'dynamic_speech.flac'
            Output   = 'dynamic_speech_low_latency.wav'
            Preset   = 'dynamic'
            Denoiser = 'off'
        },
        @{
            Name     = 'Sibilant Condenser (RNNoise)'
            Input    = 'sibilant.flac'
            Output   = 'sibilant_condenser.wav'
            Preset   = 'condenser'
            Denoiser = 'rnnoise'
        }
    )

    $results = [System.Collections.Generic.List[object]]::new()

    foreach ($case in $cases) {
        $inRel = "crates/tine-dsp/tests/fixtures/$($case.Input)"
        $outRel = "target/showcase/$($case.Output)"
        $inAbs = Join-Path $FixturesDir $case.Input
        $outAbs = Join-Path $OutDir $case.Output

        if (-not (Test-Path $inAbs)) {
            throw "Missing input fixture: $inAbs"
        }

        Write-Host "Running: $($case.Name) [$($case.Input) -> $($case.Output)]"
        $prevEap = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $stdoutLines = & cargo run -q --release -p tine-dsp --features deepfilter --example process_wav -- `
            $inRel $outRel --preset $case.Preset --denoiser $case.Denoiser 2>&1
        $exitCode = $LASTEXITCODE
        $ErrorActionPreference = $prevEap
        if ($exitCode -ne 0) {
            $stdoutLines | ForEach-Object { Write-Host $_ }
            throw "process_wav failed for $($case.Output) with exit code $exitCode"
        }

        $text = ($stdoutLines | ForEach-Object { "$_" }) -join "`n"

        $abMatch = [regex]::Match($text, '(?m)^A/B:\s+(.+)$')
        $latencyMatch = [regex]::Match($text, '(?m)^Latency:\s+(.+)$')
        $gateMatch = [regex]::Match($text, '(?m)^\s*max_gate_gr_db:\s+([^\r\n]+)$')
        $deesserMatch = [regex]::Match($text, '(?m)^\s*max_deesser_gr_db:\s+([^\r\n]+)$')
        $compMatch = [regex]::Match($text, '(?m)^\s*max_comp_gr_db:\s+([^\r\n]+)$')
        $latchMatch = [regex]::Match($text, '(?m)^\s*passthrough_latched:\s+(\S+)')

        if (-not $abMatch.Success -or -not $latencyMatch.Success -or -not $gateMatch.Success -or
            -not $deesserMatch.Success -or -not $compMatch.Success -or -not $latchMatch.Success) {
            throw "Failed to parse expected metrics from process_wav output for $($case.Output):`n$text"
        }

        $abLine = $abMatch.Groups[1].Value.Trim()
        $latencyLine = $latencyMatch.Groups[1].Value.Trim()
        $gateGr = $gateMatch.Groups[1].Value.Trim()
        $deesserGr = $deesserMatch.Groups[1].Value.Trim()
        $compGr = $compMatch.Groups[1].Value.Trim()
        $latched = $latchMatch.Groups[1].Value.Trim()

        if ($latched -ne 'false') {
            throw "Safety invariant violated: passthrough_latched was '$latched' for $($case.Output)"
        }

        if (-not (Test-Path $outAbs)) {
            throw "Expected output WAV not created: $outAbs"
        }

        $fileInfo = Get-Item $outAbs
        $sha256 = (Get-FileHash -Path $outAbs -Algorithm SHA256).Hash.ToLowerInvariant()

        # Extract concise Raw -> Tine RMS/Peak summary from A/B line for compact display
        $abDetailMatch = [regex]::Match(
            $abLine,
            'raw \(RMS ([^,]+), peak ([^,]+), crest ([^)]+)\) -> tine \(RMS ([^,]+), peak ([^,]+), crest ([^|]+)\|\s*delta RMS ([^,]+), peak ([^)]+)\)'
        )
        $rawSummary = if ($abDetailMatch.Success) {
            "RMS $($abDetailMatch.Groups[1].Value), peak $($abDetailMatch.Groups[2].Value)"
        } else {
            $abLine
        }
        $tineSummary = if ($abDetailMatch.Success) {
            "RMS $($abDetailMatch.Groups[4].Value), peak $($abDetailMatch.Groups[5].Value) (delta RMS $($abDetailMatch.Groups[7].Value), peak $($abDetailMatch.Groups[8].Value))"
        } else {
            $abLine
        }

        $results.Add([pscustomobject]@{
            Name        = $case.Name
            Input       = $case.Input
            Output      = $case.Output
            Preset      = $case.Preset
            Denoiser    = $case.Denoiser
            RawSummary  = $rawSummary
            TineSummary = $tineSummary
            AbFull      = $abLine
            Latency     = $latencyLine
            GateGr      = $gateGr
            DeesserGr   = $deesserGr
            CompGr      = $compGr
            Latched     = $latched
            SizeBytes   = $fileInfo.Length
            Sha256      = $sha256
        })
    }

    $generatedAt = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    $md = [System.Text.StringBuilder]::new()
    [void]$md.AppendLine('# Tine Audio A/B Showcase Report')
    [void]$md.AppendLine('')
    [void]$md.AppendLine("Generated: ``$generatedAt`` via ``pwsh -File scripts/demo/build-showcase.ps1``.")
    [void]$md.AppendLine('Engine command: ``cargo run --release -p tine-dsp --features deepfilter --example process_wav``.')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Summary Metrics')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('| Showcase | Input Fixture | Preset / Denoiser | Latency | Raw | Tine Output | Max Gate GR | Max De-Esser GR | Max Comp GR | Pass-Through Latched |')
    [void]$md.AppendLine('| :--- | :--- | :--- | :--- | :--- | :--- | ---: | ---: | ---: | :--- |')
    foreach ($r in $results) {
        [void]$md.AppendLine("| $($r.Name) | ``$($r.Input)`` | ``$($r.Preset)`` / ``$($r.Denoiser)`` | $($r.Latency) | $($r.RawSummary) | $($r.TineSummary) | $($r.GateGr) | $($r.DeesserGr) | $($r.CompGr) | ``$($r.Latched)`` |")
    }
    [void]$md.AppendLine('')
    [void]$md.AppendLine('## Full A/B Lines & Output Artifacts')
    [void]$md.AppendLine('')
    [void]$md.AppendLine('| Output WAV | Size (bytes) | SHA-256 |')
    [void]$md.AppendLine('| :--- | ---: | :--- |')
    foreach ($r in $results) {
        [void]$md.AppendLine("| ``target/showcase/$($r.Output)`` | $($r.SizeBytes) | ``$($r.Sha256)`` |")
    }
    [void]$md.AppendLine('')
    foreach ($r in $results) {
        [void]$md.AppendLine("### $($r.Name)")
        [void]$md.AppendLine('')
        [void]$md.AppendLine("- **Command:** ``cargo run --release -p tine-dsp --features deepfilter --example process_wav -- crates/tine-dsp/tests/fixtures/$($r.Input) target/showcase/$($r.Output) --preset $($r.Preset) --denoiser $($r.Denoiser)``")
        [void]$md.AppendLine("- **A/B:** $($r.AbFull)")
        [void]$md.AppendLine("- **Latency:** $($r.Latency)")
        [void]$md.AppendLine("- **Meters:** ``max_gate_gr_db = $($r.GateGr)``, ``max_deesser_gr_db = $($r.DeesserGr)``, ``max_comp_gr_db = $($r.CompGr)``, ``passthrough_latched = $($r.Latched)``")
        [void]$md.AppendLine("- **SHA-256:** ``$($r.Sha256)``")
        [void]$md.AppendLine('')
    }

    [System.IO.File]::WriteAllText($ReportPath, $md.ToString(), [System.Text.UTF8Encoding]::new($false))
    Write-Host "Showcase report written to $ReportPath"
}
finally {
    Pop-Location
}
