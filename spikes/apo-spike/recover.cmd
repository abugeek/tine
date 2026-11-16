@echo off
setlocal enabledelayedexpansion

REM Tine APO Spike Emergency Recovery Script
REM Works from normal elevated cmd or Safe Mode with Command Prompt.
REM Usage:
REM   recover.cmd             - Full recovery (requires Administrator)
REM   recover.cmd --dry-run   - Dry run against HKCU\Software\TineSpikeDryRun

set "CLSID={70355144-DE5B-4045-BC60-B8787CDC4077}"
set "BACKUP=%USERPROFILE%\tine-capture-backup.reg"
if not exist "%BACKUP%" (
    if exist "C:\ProgramData\TineSpike\tine-capture-backup.reg" (
        set "BACKUP=C:\ProgramData\TineSpike\tine-capture-backup.reg"
    )
)

if /I "%~1"=="--dry-run" (
    echo [DRY RUN] Testing recovery commands against HKCU\Software\TineSpikeDryRun...
    set "DRY_BACKUP=%TEMP%\tine-dryrun-backup.reg"
    reg add "HKCU\Software\TineSpikeDryRun\Classes\CLSID\%CLSID%\InprocServer32" /ve /d "C:\Program Files\TineSpike\apo_spike.dll" /f >nul
    reg add "HKCU\Software\TineSpikeDryRun\Classes\AudioEngine\AudioProcessingObjects\%CLSID%" /v "FriendlyName" /d "Tine APO Spike" /f >nul
    reg add "HKCU\Software\TineSpikeDryRun\MMDevices\Audio\Capture\TestEndpoint\FxProperties" /v "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1" /d "{00000000-0000-0000-0000-000000000000}" /f >nul
    reg export "HKCU\Software\TineSpikeDryRun\MMDevices\Audio\Capture" "!DRY_BACKUP!" /y >nul
    if errorlevel 1 (
        echo [DRY RUN] FAILED: reg export failed.
        exit /b 1
    )
    reg add "HKCU\Software\TineSpikeDryRun\MMDevices\Audio\Capture\TestEndpoint\FxProperties" /v "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1" /d "%CLSID%" /f >nul

    reg delete "HKCU\Software\TineSpikeDryRun\Classes\CLSID\%CLSID%" /f >nul
    reg delete "HKCU\Software\TineSpikeDryRun\Classes\AudioEngine\AudioProcessingObjects\%CLSID%" /f >nul
    reg import "!DRY_BACKUP!" >nul
    if errorlevel 1 (
        echo [DRY RUN] FAILED: reg import failed.
        exit /b 1
    )
    reg query "HKCU\Software\TineSpikeDryRun\MMDevices\Audio\Capture\TestEndpoint\FxProperties" /v "{d04e05a6-594b-4fb6-a80d-01af5eed7d1d},1" | findstr /C:"{00000000-0000-0000-0000-000000000000}" >nul
    if errorlevel 1 (
        echo [DRY RUN] FAILED: original value not restored.
        exit /b 1
    )
    reg delete "HKCU\Software\TineSpikeDryRun" /f >nul
    del /q "!DRY_BACKUP!" >nul 2>&1
    echo [DRY RUN] PASSED: CLSID/APO keys deleted and backup .reg restored cleanly.
    exit /b 0
)

echo === Tine APO Spike Recovery ===
echo Stopping Windows Audio service...
sc stop audiosrv >nul 2>&1
timeout /t 2 /nobreak >nul

echo Removing spike CLSID registration...
reg delete "HKLM\SOFTWARE\Classes\CLSID\%CLSID%" /f 2>nul

echo Removing spike APO registration...
reg delete "HKLM\SOFTWARE\Classes\AudioEngine\AudioProcessingObjects\%CLSID%" /f 2>nul

echo Removing spike flags...
reg delete "HKLM\SOFTWARE\TineSpike" /f 2>nul

if exist "%BACKUP%" (
    echo Re-importing capture registry backup from "%BACKUP%"...
    reg import "%BACKUP%"
    if errorlevel 1 (
        echo WARNING: reg import returned an error. Some keys may require SYSTEM/TrustedInstaller ownership.
    )
) else (
    echo WARNING: Backup file not found at "%BACKUP%". Skipping reg import.
)

echo Starting Windows Audio service...
sc start audiosrv >nul 2>&1
echo === Recovery complete ===
endlocal
