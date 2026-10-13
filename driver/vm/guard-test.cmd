@echo off
REM Milestone 2 + 3 test: policy, prompt, and a service that stays answerable.
REM
REM Expected: the unmarked file disappears, the important one does not, and a
REM dialog appears in the console session asking about the important one.
REM
REM The previous version of the guard blocked inside that dialog, which starved
REM the only thread calling FilterGetMessage; the driver then timed out and
REM failed open, so subsequent deletes were allowed silently. This run deletes
REM TWO protected files in a row: the second one is what proves the loop stayed
REM available while the first prompt was on screen.
set SRC=C:\sr-test\driver
set OUT=C:\sr-test\guard-test.txt
set DIR=C:\sr-test\guardtest

echo === hard reset of any previous service === > "%OUT%"
REM taskkill first: a service stuck in STOP_PENDING keeps the image and the log
REM file locked, and then both the redeploy and CreateService fail with
REM "marked for deletion" (1072) - which looks like an install problem and is
REM actually a leftover-process problem.
taskkill /f /im srguard.exe >> "%OUT%" 2>&1
sc stop SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
sc delete SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
sc query SpaceRecycleGuard >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === clear the log === >> "%OUT%"
del /q C:\ProgramData\SpaceRecycle\guard.log >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === install and start === >> "%OUT%"
"%SRC%\srguard.exe" install >> "%OUT%" 2>&1
echo install exit=%ERRORLEVEL% >> "%OUT%"
sc start SpaceRecycleGuard >> "%OUT%" 2>&1
echo start exit=%ERRORLEVEL% >> "%OUT%"
ping -n 6 127.0.0.1 > nul
sc query SpaceRecycleGuard | findstr STATE >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === fixture === >> "%OUT%"
if not exist "%DIR%" mkdir "%DIR%"
echo hello > "%DIR%\ordinary.txt"
echo hello > "%DIR%\important-A.txt"
echo hello > "%DIR%\important-B.txt"
C:\sr-test\bin\menutest.exe set "%DIR%\important-A.txt" 0 > nul 2>&1
C:\sr-test\bin\menutest.exe set "%DIR%\important-B.txt" 0 > nul 2>&1
C:\sr-test\bin\menutest.exe get "%DIR%\important-A.txt" >> "%OUT%" 2>&1
C:\sr-test\bin\menutest.exe get "%DIR%\important-B.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === delete the unmarked file (expected: succeeds) === >> "%OUT%"
del "%DIR%\ordinary.txt" >> "%OUT%" 2>&1
echo del ordinary exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === delete important-A (expected: refused, prompt appears) === >> "%OUT%"
del "%DIR%\important-A.txt" >> "%OUT%" 2>&1
echo del important-A exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === delete important-B while the first prompt is up (expected: also refused) === >> "%OUT%"
del "%DIR%\important-B.txt" >> "%OUT%" 2>&1
echo del important-B exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === what survived === >> "%OUT%"
if exist "%DIR%\ordinary.txt" (echo ordinary.txt  STILL THERE >> "%OUT%") else (echo ordinary.txt  deleted >> "%OUT%")
if exist "%DIR%\important-A.txt" (echo important-A   STILL THERE >> "%OUT%") else (echo important-A   DELETED >> "%OUT%")
if exist "%DIR%\important-B.txt" (echo important-B   STILL THERE >> "%OUT%") else (echo important-B   DELETED >> "%OUT%")

echo. >> "%OUT%"
echo === guard log === >> "%OUT%"
type C:\ProgramData\SpaceRecycle\guard.log >> "%OUT%" 2>&1

echo done