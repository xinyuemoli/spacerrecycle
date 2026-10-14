@echo off
REM Verifies the three requirements at once, using the guard log as the meter.
REM
REM The log records every delete the driver escalated. So "the log did not grow"
REM is a direct measurement of "no message crossed to user mode", which is what
REM requirements 1 and 2 are about, and what makes requirement 3 provable: the
REM disposition changes while the count stays flat.
REM
REM   A  default policy (ask)   unmarked delete -> allowed, log flat
REM                             marked delete   -> refused, log grows
REM   B  policy deny            marked delete   -> refused, log FLAT
REM   C  policy allow           marked delete   -> DELETED, log FLAT
REM   D  back to ask            marked delete   -> refused, log grows again
set OUT=C:\sr-test\policy-test.txt
set D=C:\sr-test\policytest
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
echo === install and start === >> "%OUT%"
C:\sr-test\driver\srguard.exe install >> "%OUT%" 2>&1
sc start SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 8 127.0.0.1 > nul

if not exist "%D%" mkdir "%D%"

echo. >> "%OUT%"
echo ============ PHASE A: default policy is "ask" ============ >> "%OUT%"
del /q "%CFG%" 2>nul
ping -n 5 127.0.0.1 > nul

echo hello > "%D%\a-unmarked.txt"
echo hello > "%D%\a-marked.txt"
%MENU% set "%D%\a-marked.txt" 0 > nul 2>&1

echo -- A1: delete the UNMARKED file -- >> "%OUT%"
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\a-unmarked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\a-unmarked.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: deleted >> "%OUT%")

echo -- A2: delete the MARKED file -- >> "%OUT%"
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\a-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\a-marked.txt" (echo    RESULT: still there ^(refused^) >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo ============ PHASE B: push policy action_marked=deny ============ >> "%OUT%"
echo action_marked=deny> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul

echo hello > "%D%\b-marked.txt"
%MENU% set "%D%\b-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\b-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\b-marked.txt" (echo    RESULT: still there ^(refused locally^) >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo ============ PHASE C: push policy action_marked=allow ============ >> "%OUT%"
echo action_marked=allow> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul

echo hello > "%D%\c-marked.txt"
%MENU% set "%D%\c-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\c-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\c-marked.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: deleted ^(allowed locally^) >> "%OUT%")

echo. >> "%OUT%"
echo ============ PHASE D: push policy back to ask ============ >> "%OUT%"
echo action_marked=ask> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
ping -n 6 127.0.0.1 > nul

echo hello > "%D%\d-marked.txt"
%MENU% set "%D%\d-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\d-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\d-marked.txt" (echo    RESULT: still there ^(refused, back to asking^) >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo ============ guard log (policy lines and decide lines) ============ >> "%OUT%"
powershell -NoProfile -Command "Select-String -Path '%LOG%' -Pattern 'policy applied|policy push|policy read-back|connected|guard starting' | ForEach-Object { $_.Line }" >> "%OUT%" 2>&1

echo done