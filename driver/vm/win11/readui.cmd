@echo off
REM vmrun's runProgramInGuest does not relay the program's stdout, so the
REM automation output goes to a file that is copied back afterwards.
powershell -NoProfile -ExecutionPolicy Bypass -File C:\sr-test\vm\read-ui.ps1 %* > C:\sr-test\vm\ui.txt 2>&1
echo exit=%ERRORLEVEL% >> C:\sr-test\vm\ui.txt
