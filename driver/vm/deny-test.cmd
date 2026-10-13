@echo off
REM Isolation test: does the driver honour a refusal at all?
REM
REM srlisten is told to refuse everything. If deletes still succeed, the fault is
REM in the driver's completion path and has nothing to do with ADS policy or the
REM service. If deletes are blocked, the driver is fine and the problem is on the
REM service side.
REM
REM The listener has to run as SYSTEM: the filter port's default security
REM descriptor admits nothing else.
set SRC=C:\sr-test\driver
set OUT=C:\sr-test\deny-test.txt
set LOG=%SRC%\deny.log

if exist "%LOG%" del /q "%LOG%"

echo === start a refusing listener as SYSTEM === > "%OUT%"
schtasks /delete /tn srdeny /f >> "%OUT%" 2>&1
schtasks /create /tn srdeny /tr "\"%SRC%\srlisten.exe\" \"%LOG%\" -deny -seconds 25" /sc once /st 23:59 /ru SYSTEM /rl highest /f >> "%OUT%" 2>&1
echo create exit=%ERRORLEVEL% >> "%OUT%"
schtasks /run /tn srdeny >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul

echo. >> "%OUT%"
echo === fixture (unmarked, so only the driver decides) === >> "%OUT%"
if not exist C:\sr-test\denytest mkdir C:\sr-test\denytest
echo hello > C:\sr-test\denytest\blocked.txt
if exist C:\sr-test\denytest\blocked.txt (echo fixture created >> "%OUT%") else (echo FIXTURE MISSING >> "%OUT%")

echo. >> "%OUT%"
echo === delete it while the listener refuses everything === >> "%OUT%"
del C:\sr-test\denytest\blocked.txt >> "%OUT%" 2>&1
echo del exit=%ERRORLEVEL% >> "%OUT%"
if exist C:\sr-test\denytest\blocked.txt (echo RESULT: BLOCKED - driver honours the refusal >> "%OUT%") else (echo RESULT: DELETED - driver ignores the refusal >> "%OUT%")

echo. >> "%OUT%"
echo === listener log === >> "%OUT%"
ping -n 24 127.0.0.1 > nul
schtasks /delete /tn srdeny /f >> "%OUT%" 2>&1
type "%LOG%" >> "%OUT%" 2>&1

echo done