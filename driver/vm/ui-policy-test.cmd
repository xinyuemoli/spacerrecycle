@echo off
REM End-to-end check for the settings screen's half of the feature.
REM
REM The screen does not call the driver: it writes the policy file, the resident
REM service notices within a couple of seconds, and the service pushes the value
REM into the kernel filter. So what has to be true is that the file exists, that
REM the person using the machine can actually write it, and that editing it
REM changes what happens to a marked file.
REM
REM The ACL check is the one that would otherwise be discovered by a user who
REM clicks the setting and gets an error: the service runs as LocalSystem, so a
REM file it creates belongs to SYSTEM and is not writable by anyone else until
REM somebody widens it.
set OUT=C:\sr-test\ui-policy.txt
set D=C:\sr-test\uitest
set LOG=C:\ProgramData\SpaceRecycle\guard.log
set CFG=C:\ProgramData\SpaceRecycle\policy.ini
set MENU=C:\sr-test\bin\menutest.exe
set COUNT=powershell -NoProfile -Command "(Select-String -Path '%LOG%' -Pattern 'delete requested' -SimpleMatch -ErrorAction SilentlyContinue).Count"

echo === reset service === > "%OUT%"
taskkill /f /im srguard.exe >> "%OUT%" 2>&1
sc stop SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
sc delete SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul

echo. >> "%OUT%"
echo === delete the policy file so the service has to create it === >> "%OUT%"
del /q "%CFG%" >> "%OUT%" 2>&1
if exist "%CFG%" (echo   still there) else (echo   removed)

echo. >> "%OUT%"
echo === start the service === >> "%OUT%"
C:\sr-test\driver\srguard.exe install >> "%OUT%" 2>&1
sc start SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 12 127.0.0.1 > nul
sc query SpaceRecycleGuard | findstr /c:"4  RUNNING" > nul
if errorlevel 1 (
  echo SERVICE NOT RUNNING - ABORTING >> "%OUT%"
  goto :report
)

echo. >> "%OUT%"
echo === did the service create it, and can a normal user write it === >> "%OUT%"
if exist "%CFG%" (echo   file: created) else (echo   file: MISSING)
echo   --- content --- >> "%OUT%"
type "%CFG%" >> "%OUT%" 2>&1
echo   --- ACL --- >> "%OUT%"
icacls "%CFG%" >> "%OUT%" 2>&1

if not exist "%D%" mkdir "%D%"

echo. >> "%OUT%"
echo ============ set action_marked=deny, the way the screen would ============ >> "%OUT%"
echo action_marked=deny> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul
echo hello > "%D%\deny.txt"
"%MENU%" set "%D%\deny.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\deny.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\deny.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo ============ set action_marked=allow ============ >> "%OUT%"
echo action_marked=allow> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul
echo hello > "%D%\allow.txt"
"%MENU%" set "%D%\allow.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\allow.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\allow.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: deleted >> "%OUT%")

echo. >> "%OUT%"
echo ============ back to ask ============ >> "%OUT%"
echo action_marked=ask> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul
echo hello > "%D%\ask.txt"
"%MENU%" set "%D%\ask.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\ask.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\ask.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

:report
echo. >> "%OUT%"
echo === service log tail === >> "%OUT%"
powershell -NoProfile -Command "Get-Content '%LOG%' | Select-Object -Last 18" >> "%OUT%" 2>&1
echo done
