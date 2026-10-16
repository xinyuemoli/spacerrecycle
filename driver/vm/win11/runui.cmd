@echo off
REM Runs the UI automation in the interactive session and reports everything in
REM one file, including the timestamps that prove the output is from this run.
REM
REM Earlier attempts read a stale copy: the task writes ui.txt, so fetching a
REM different file, or fetching before the task finished, silently reports the
REM previous run's result. Everything needed to tell the difference is printed
REM here side by side.
set OUT=C:\sr-test\vm\runui.txt
echo === start %DATE% %TIME% === > "%OUT%"

echo --- delete any previous result --- >> "%OUT%"
del /q C:\sr-test\vm\ui.txt >> "%OUT%" 2>&1
if exist C:\sr-test\vm\ui.txt (echo ui.txt STILL PRESENT) else (echo ui.txt cleared)

echo. >> "%OUT%"
echo --- (re)create the task --- >> "%OUT%"
schtasks /delete /tn sruia /f >> "%OUT%" 2>&1
schtasks /create /tn sruia /tr "C:\sr-test\vm\readui-task.cmd" /sc once /st 23:59 /ru sruser /it /f >> "%OUT%" 2>&1
echo create exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo --- run it --- >> "%OUT%"
schtasks /run /tn sruia >> "%OUT%" 2>&1
echo run exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo --- waiting for the result --- >> "%OUT%"
set WAITED=0
:wait
if exist C:\sr-test\vm\ui.txt goto ready
ping -n 4 127.0.0.1 > nul
set /a WAITED+=3
if %WAITED% LSS 120 goto wait
echo TIMED OUT after %WAITED%s >> "%OUT%"
goto report

:ready
echo ui.txt appeared after about %WAITED%s >> "%OUT%"
REM Give the script time to finish writing it.
ping -n 8 127.0.0.1 > nul

:report
echo. >> "%OUT%"
echo === ui.txt timestamps === >> "%OUT%"
dir C:\sr-test\vm\ui.txt >> "%OUT%" 2>&1
echo. >> "%OUT%"
echo === ui.txt contents === >> "%OUT%"
type C:\sr-test\vm\ui.txt >> "%OUT%" 2>&1
echo. >> "%OUT%"
echo === end %DATE% %TIME% === >> "%OUT%"
