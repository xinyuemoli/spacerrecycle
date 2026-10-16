@echo off
REM Switches the running app to its settings tab and dumps what it renders.
REM
REM This has to run from a scheduled task with /it rather than through
REM runProgramInGuest: guest programs started by vmrun land in session 0, and an
REM accessibility client only sees the desktop of its own session. The first
REM attempt reported "automation client session: 0 / app session: 1" and found
REM nothing, which is exactly what that mismatch looks like.
set OUT=C:\sr-test\vm\ui.txt
powershell -NoProfile -ExecutionPolicy Bypass -File C:\sr-test\vm\read-ui.ps1 -Tab Settings > "%OUT%" 2>&1
echo exit=%ERRORLEVEL% >> "%OUT%"
