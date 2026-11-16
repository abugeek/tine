@echo off
setlocal enabledelayedexpansion

net session >nul 2>&1
if errorlevel 1 (
    echo ERROR: This script must be run as Administrator ^(elevated cmd/PowerShell^).
    exit /b 1
)

set "EXE=D:\projects\rust\tine-wt\T02\target\release\apo-spike.exe"
set "FIFINE={0.0.1.00000000}.{a60e1e49-434c-4b8f-8c58-54ac3daec5b9}"
set "FIFINE_GUID={a60e1e49-434c-4b8f-8c58-54ac3daec5b9}"
set "AB13X={0.0.1.00000000}.{1524e3bf-2167-474a-ba48-81f5a7464fa2}"
set "AB13X_GUID={1524e3bf-2167-474a-ba48-81f5a7464fa2}"
set "LOGDIR=C:\ProgramData\TineSpike"
set "LOG=%LOGDIR%\matrix-results.log"

mkdir "%LOGDIR%" 2>nul

REM Clean up any leftover state from a previous interrupted run
if exist "%LOGDIR%\backup_%FIFINE_GUID%.json" (
    echo Cleaning up leftover fifine install...
    "%EXE%" uninstall "%FIFINE%" >nul 2>&1
    "%EXE%" reload "%FIFINE%" >nul 2>&1
)
if exist "%LOGDIR%\backup_%AB13X_GUID%.json" (
    echo Cleaning up leftover AB13X install...
    "%EXE%" uninstall "%AB13X%" >nul 2>&1
    "%EXE%" reload "%AB13X%" >nul 2>&1
)

del /q "%LOGDIR%\spike.log" "%LOGDIR%\format.log" "%LOG%" "%LOGDIR%\cap.tmp" 2>nul

echo === T02 Hardware Matrix Run === > "%LOG%"
echo. >> "%LOG%"

echo [1/2] Testing Dynamic Mic: fifine Microphone
echo --- 1. FIFINE MICROPHONE --- >> "%LOG%"
reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\%FIFINE_GUID%\FxProperties" "%LOGDIR%\fifine-before.reg" /y >nul
"%EXE%" capture-test "%FIFINE%" --ms 1000 >> "%LOG%" 2>&1
"%EXE%" install "%FIFINE%" --slot sfx >> "%LOG%" 2>&1
"%EXE%" reload "%FIFINE%" >> "%LOG%" 2>&1
timeout /t 1 /nobreak >nul

del /q "%LOGDIR%\cap.tmp" 2>nul
start /b "" cmd /c ""%EXE%" capture-test "%FIFINE%" --ms 3000 > "%LOGDIR%\cap.tmp" 2>&1"
timeout /t 1 /nobreak >nul
"%EXE%" heartbeat --count 3 >> "%LOG%" 2>&1
if errorlevel 1 (
    echo [IPolicyConfig reload did not activate SHM; trying audiosrv restart...] >> "%LOG%"
    timeout /t 3 /nobreak >nul
    type "%LOGDIR%\cap.tmp" >> "%LOG%" 2>&1
    "%EXE%" reload "%FIFINE%" --service >> "%LOG%" 2>&1
    timeout /t 2 /nobreak >nul
    del /q "%LOGDIR%\cap.tmp" 2>nul
    start /b "" cmd /c ""%EXE%" capture-test "%FIFINE%" --ms 3000 > "%LOGDIR%\cap.tmp" 2>&1"
    timeout /t 1 /nobreak >nul
    "%EXE%" heartbeat --count 3 >> "%LOG%" 2>&1
)
timeout /t 3 /nobreak >nul
if exist "%LOGDIR%\cap.tmp" type "%LOGDIR%\cap.tmp" >> "%LOG%" 2>&1

echo [RAW MODE TEST] >> "%LOG%"
"%EXE%" capture-test "%FIFINE%" --raw --ms 1000 >> "%LOG%" 2>&1

echo [FORCE PANIC TEST] >> "%LOG%"
reg add "HKLM\SOFTWARE\TineSpike" /v ForcePanic /t REG_DWORD /d 1 /f >nul
"%EXE%" reload "%FIFINE%" >> "%LOG%" 2>&1
timeout /t 1 /nobreak >nul
del /q "%LOGDIR%\cap.tmp" 2>nul
start /b "" cmd /c ""%EXE%" capture-test "%FIFINE%" --ms 3000 > "%LOGDIR%\cap.tmp" 2>&1"
timeout /t 1 /nobreak >nul
"%EXE%" heartbeat --count 2 >> "%LOG%" 2>&1
timeout /t 3 /nobreak >nul
if exist "%LOGDIR%\cap.tmp" type "%LOGDIR%\cap.tmp" >> "%LOG%" 2>&1
reg delete "HKLM\SOFTWARE\TineSpike" /v ForcePanic /f >nul 2>&1

"%EXE%" uninstall "%FIFINE%" >> "%LOG%" 2>&1
"%EXE%" reload "%FIFINE%" >> "%LOG%" 2>&1
timeout /t 1 /nobreak >nul
reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\%FIFINE_GUID%\FxProperties" "%LOGDIR%\fifine-after.reg" /y >nul
fc "%LOGDIR%\fifine-before.reg" "%LOGDIR%\fifine-after.reg" >> "%LOG%" 2>&1
"%EXE%" capture-test "%FIFINE%" --ms 1000 >> "%LOG%" 2>&1

echo. >> "%LOG%"
echo [2/2] Testing Gaming Headset: AB13X USB Audio
echo --- 2. AB13X USB AUDIO --- >> "%LOG%"
reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\%AB13X_GUID%\FxProperties" "%LOGDIR%\ab13x-before.reg" /y >nul
"%EXE%" capture-test "%AB13X%" --ms 1000 >> "%LOG%" 2>&1
"%EXE%" install "%AB13X%" --slot sfx >> "%LOG%" 2>&1
"%EXE%" reload "%AB13X%" >> "%LOG%" 2>&1
timeout /t 1 /nobreak >nul

del /q "%LOGDIR%\cap.tmp" 2>nul
start /b "" cmd /c ""%EXE%" capture-test "%AB13X%" --ms 3000 > "%LOGDIR%\cap.tmp" 2>&1"
timeout /t 1 /nobreak >nul
"%EXE%" heartbeat --count 3 >> "%LOG%" 2>&1
if errorlevel 1 (
    echo [IPolicyConfig reload did not activate SHM; trying audiosrv restart...] >> "%LOG%"
    timeout /t 3 /nobreak >nul
    type "%LOGDIR%\cap.tmp" >> "%LOG%" 2>&1
    "%EXE%" reload "%AB13X%" --service >> "%LOG%" 2>&1
    timeout /t 2 /nobreak >nul
    del /q "%LOGDIR%\cap.tmp" 2>nul
    start /b "" cmd /c ""%EXE%" capture-test "%AB13X%" --ms 3000 > "%LOGDIR%\cap.tmp" 2>&1"
    timeout /t 1 /nobreak >nul
    "%EXE%" heartbeat --count 3 >> "%LOG%" 2>&1
)
timeout /t 3 /nobreak >nul
if exist "%LOGDIR%\cap.tmp" type "%LOGDIR%\cap.tmp" >> "%LOG%" 2>&1

"%EXE%" uninstall "%AB13X%" >> "%LOG%" 2>&1
"%EXE%" reload "%AB13X%" >> "%LOG%" 2>&1
timeout /t 1 /nobreak >nul
reg export "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\MMDevices\Audio\Capture\%AB13X_GUID%\FxProperties" "%LOGDIR%\ab13x-after.reg" /y >nul
fc "%LOGDIR%\ab13x-before.reg" "%LOGDIR%\ab13x-after.reg" >> "%LOG%" 2>&1
"%EXE%" capture-test "%AB13X%" --ms 1000 >> "%LOG%" 2>&1

del /q "%LOGDIR%\cap.tmp" 2>nul
echo === DONE === >> "%LOG%"
echo Done! Results saved to %LOG%
endlocal
