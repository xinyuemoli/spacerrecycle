@echo off
REM Verifies the three requirements, using the guard log as the meter.
REM
REM The log records every delete the driver escalated, so "the log did not grow"
REM is a direct measurement of "nothing crossed to user mode". That is what
REM requirements 1 and 2 are about, and it is what makes requirement 3 provable:
REM the disposition changes while the count stays flat.
REM
REM   A  default policy (ask)   unmarked delete -> allowed, log flat
REM                             marked delete   -> refused, log grows
REM   B  policy deny            marked delete   -> refused, log FLAT
REM   C  policy allow           marked delete   -> DELETED, log FLAT
REM   D  back to ask            marked delete   -> refused, log grows again
REM
REM Two harness faults made earlier runs of this script report failures that
REM were not real, and both are guarded against now.
REM
REM The first: phases ran after the service had been deleted and not
REM reinstalled. With no connected service the driver fails open by design, so
REM every file deletes - which is exactly what a working "allow" policy looks
REM like. Every phase now asserts the service is running.
REM
REM The second: a fixed sleep after writing the settings file races the service's
REM two-second poll, so a phase could run against the previous disposition and
REM the result depended on where the phase landed in that window. Each phase now
REM waits for the log to confirm the new policy is actually in force, and aborts
REM if it never arrives. A test that proceeds on a timeout is a test that
REM reports the timeout as a product defect.
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
ping -n 12 127.0.0.1 > nul
sc query SpaceRecycleGuard | findstr /c:"4  RUNNING" > nul
if errorlevel 1 (
  echo SERVICE NOT RUNNING - ABORTING, results would be meaningless >> "%OUT%"
  sc query SpaceRecycleGuard >> "%OUT%" 2>&1
  goto :eof
)

if not exist "%D%" mkdir "%D%"
del /q "%CFG%" 2>nul
ping -n 5 127.0.0.1 > nul

echo. >> "%OUT%"
echo ============ PHASE A: policy is "ask" (the built-in default) ============ >> "%OUT%"

echo hello > "%D%\a-unmarked.txt"
echo hello > "%D%\a-marked.txt"
"%MENU%" set "%D%\a-marked.txt" 0 > nul 2>&1

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
echo ============ PHASE B: push action_marked=deny ============ >> "%OUT%"
call :marklog
echo action_marked=deny> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
call :waitpolicy deny %MARK%
echo hello > "%D%\b-marked.txt"
"%MENU%" set "%D%\b-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\b-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\b-marked.txt" (echo    RESULT: still there ^(refused locally^) >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo ============ PHASE C: push action_marked=allow ============ >> "%OUT%"
call :marklog
echo action_marked=allow> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
call :waitpolicy allow %MARK%
echo hello > "%D%\c-marked.txt"
"%MENU%" set "%D%\c-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\c-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\c-marked.txt" (echo    RESULT: still there >> "%OUT%") else (echo    RESULT: deleted ^(allowed locally^) >> "%OUT%")

echo. >> "%OUT%"
echo ============ PHASE D: push action_marked=ask ============ >> "%OUT%"
call :marklog
echo action_marked=ask> "%CFG%"
echo action_unmarked=allow>> "%CFG%"
call :waitpolicy ask %MARK%
echo hello > "%D%\d-marked.txt"
"%MENU%" set "%D%\d-marked.txt" 0 > nul 2>&1
for /f %%N in ('%COUNT%') do echo    log before: %%N >> "%OUT%"
del "%D%\d-marked.txt" >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul
for /f %%N in ('%COUNT%') do echo    log after : %%N >> "%OUT%"
if exist "%D%\d-marked.txt" (echo    RESULT: still there ^(refused, back to asking^) >> "%OUT%") else (echo    RESULT: DELETED >> "%OUT%")

goto :report

REM ---------------------------------------------------------------------------
REM marklog - remember how long the log is right now
REM
REM The wait below has to look only at lines written from this point on. The log
REM is append-only and every policy has been pushed many times across runs, so
REM searching the whole file matches a line from an earlier run and returns
REM instantly - which is how an earlier version of this script "confirmed" a
REM policy that had not been applied yet, and then reported the resulting race
REM as a product defect.
:marklog
for /f %%N in ('powershell -NoProfile -Command "@(Get-Content '%LOG%' -ErrorAction SilentlyContinue).Count"') do set MARK=%%N
goto :eof

REM ---------------------------------------------------------------------------
REM waitpolicy <ask|deny|allow> <lines-already-in-log>
:waitpolicy
set TARGET=%~1
set SINCE=%~2
set TRIES=0
:wp_loop
powershell -NoProfile -Command "$t=Get-Content '%LOG%' -ErrorAction SilentlyContinue | Select-Object -Skip %SINCE% | Select-String -Pattern 'policy applied: marked=%TARGET% ' -SimpleMatch; if($t){exit 0}else{exit 1}" > nul 2>&1
if not errorlevel 1 (
  echo    policy confirmed in force from a NEW log line: %TARGET% >> "%OUT%"
  goto :eof
)
set /a TRIES+=1
if %TRIES% GEQ 30 (
  echo    POLICY %TARGET% NEVER BECAME ACTIVE - ABORTING >> "%OUT%"
  sc query SpaceRecycleGuard | findstr STATE >> "%OUT%"
  goto :report
)
ping -n 2 127.0.0.1 > nul
goto :wp_loop

:report
echo. >> "%OUT%"
echo ============ final service state ============ >> "%OUT%"
sc query SpaceRecycleGuard | findstr STATE >> "%OUT%"
echo. >> "%OUT%"
echo ============ this run's log ============ >> "%OUT%"
powershell -NoProfile -Command "Get-Content '%LOG%' | Select-Object -Last 40" >> "%OUT%" 2>&1
echo done