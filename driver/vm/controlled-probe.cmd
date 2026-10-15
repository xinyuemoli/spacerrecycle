@echo off
REM Controlled experiment: the service is definitely installed and running, and
REM the driver's counters are read before and after a single marked delete.
REM
REM The counters are what make this unambiguous. "decisions +1, escalated +1"
REM means the driver asked and was answered. "decisions +1, escalated +0" means
REM the driver decided by itself - which, for a marked file under ask, can only
REM mean the mark check said the file was unmarked.
set OUT=C:\sr-test\controlled.txt
set D=C:\sr-test\ctl
set MENU=C:\sr-test\bin\menutest.exe
set LOG=C:\ProgramData\SpaceRecycle\guard.log

echo === ensure the service is installed and running === > "%OUT%"
taskkill /f /im srguard.exe >> "%OUT%" 2>&1
sc stop SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
sc delete SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
C:\sr-test\driver\srguard.exe install >> "%OUT%" 2>&1
sc start SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 12 127.0.0.1 > nul
sc query SpaceRecycleGuard | findstr STATE >> "%OUT%"

echo. >> "%OUT%"
echo === counters before === >> "%OUT%"
powershell -NoProfile -Command "Get-Content '%LOG%' | Select-Object -Last 6" >> "%OUT%" 2>&1

if not exist "%D%" mkdir "%D%"
del /q "%D%\*.txt" 2>nul
echo hello > "%D%\one.txt"
"%MENU%" set "%D%\one.txt" 0 > nul 2>&1
echo. >> "%OUT%"
echo === the file and its mark === >> "%OUT%"
"%MENU%" get "%D%\one.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === single delete === >> "%OUT%"
del "%D%\one.txt" >> "%OUT%" 2>&1
ping -n 5 127.0.0.1 > nul
if exist "%D%\one.txt" (echo   RESULT: SURVIVED >> "%OUT%") else (echo   RESULT: DELETED >> "%OUT%")

echo. >> "%OUT%"
echo === log after (watch decisions/escalated and how many requests) === >> "%OUT%"
powershell -NoProfile -Command "Get-Content '%LOG%' | Select-Object -Last 12" >> "%OUT%" 2>&1
echo done
