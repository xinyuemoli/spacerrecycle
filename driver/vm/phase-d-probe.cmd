@echo off
REM Minimal reproduction of the phase-D anomaly, with no script ambiguity.
REM
REM Phase D of policy-test.cmd reports a marked file as deleted even though the
REM guard log shows the driver escalating it and the service refusing it. This
REM probe removes everything else from the picture: one file, one mark, one
REM delete, and every intermediate fact printed.
set OUT=C:\sr-test\phase-d.txt
set D=C:\sr-test\pdt
set MENU=C:\sr-test\bin\menutest.exe

echo === setup === > "%OUT%"
if not exist "%D%" mkdir "%D%"
del /q "%D%\*.txt" 2>nul
echo hello > "%D%\marked.txt"
echo   created, dir listing: >> "%OUT%"
dir /b "%D%" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === the mark === >> "%OUT%"
"%MENU%" set "%D%\marked.txt" 0 >> "%OUT%" 2>&1
echo   get before delete: >> "%OUT%"
"%MENU%" get "%D%\marked.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === the delete === >> "%OUT%"
del "%D%\marked.txt"
echo   del errorlevel=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === immediate state === >> "%OUT%"
if exist "%D%\marked.txt" (echo   EXISTS right after del >> "%OUT%") else (echo   GONE right after del >> "%OUT%")
dir /b "%D%" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === state after 5 more seconds === >> "%OUT%"
ping -n 6 127.0.0.1 > nul
if exist "%D%\marked.txt" (echo   EXISTS after 5s >> "%OUT%") else (echo   GONE after 5s >> "%OUT%")
dir /b "%D%" >> "%OUT%" 2>&1

echo done