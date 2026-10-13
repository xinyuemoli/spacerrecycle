@echo off
REM Does the listener work at all, run synchronously?
REM
REM -seconds makes it exit on its own, so this can capture its output. vmrun
REM does not relay a guest program's stdout back to the host, so everything is
REM written to a result file that the host then copies out.
set SRC=C:\sr-test\driver
set RES=C:\sr-test\listener-probe.txt

if exist "%SRC%\direct.log" del /q "%SRC%\direct.log"

echo === running listener for 3 seconds === > "%RES%"
"%SRC%\srlisten.exe" "%SRC%\direct.log" -seconds 3 > "%SRC%\stdout.txt" 2>&1
echo exit=%ERRORLEVEL% >> "%RES%"

echo. >> "%RES%"
echo === stdout === >> "%RES%"
if exist "%SRC%\stdout.txt" (type "%SRC%\stdout.txt" >> "%RES%") else (echo (none) >> "%RES%")

echo. >> "%RES%"
echo === listener file log === >> "%RES%"
if exist "%SRC%\direct.log" (type "%SRC%\direct.log" >> "%RES%") else (echo (none) >> "%RES%")

echo. >> "%RES%"
echo === is the filter still loaded? === >> "%RES%"
fltmc filters >> "%RES%" 2>&1

echo done