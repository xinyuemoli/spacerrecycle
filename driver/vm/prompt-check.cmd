@echo off
REM Is the prompt dialog actually on screen? A message box belonging to a
REM service process shows up as that process's window title in tasklist /v, so
REM this is a check that does not depend on looking at the screen.
REM
REM The delete is started in the background so the window can be inspected while
REM the dialog is still up; the dialog's own timeout would otherwise close it
REM before the check ran.
set OUT=C:\sr-test\promptcheck.txt
set D=C:\sr-test\guardtest

echo hello > "%D%\prompt-check.txt"
C:\sr-test\bin\menutest.exe set "%D%\prompt-check.txt" 0 > nul 2>&1

start /b cmd /c "del \"%D%\prompt-check.txt\""
ping -n 4 127.0.0.1 > nul

echo === srguard window title while the prompt is up === > "%OUT%"
tasklist /v /fi "imagename eq srguard.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === guard log tail === >> "%OUT%"
powershell -NoProfile -Command "Get-Content C:\ProgramData\SpaceRecycle\guard.log -Tail 10" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === did the file survive === >> "%OUT%"
if exist "%D%\prompt-check.txt" (echo SURVIVED - the delete was refused >> "%OUT%") else (echo GONE >> "%OUT%")

echo done