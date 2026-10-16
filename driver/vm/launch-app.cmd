@echo off
REM Launches the app on the guest's interactive desktop.
REM
REM vmrun's -interactive flag needs the credentials of whoever is logged on, and
REM the auto-logon account here has no password, so an empty -gp gets dropped on
REM the way through the command line and vmrun swallows the next argument
REM instead. A scheduled task with /it runs in the interactive session when that
REM user is logged on and stores no password, which sidesteps the whole problem.
set OUT=C:\sr-test\launch.txt
set APP=C:\sr-test\app\spacerrecycle.exe

echo === taskkill any previous instance (as the logged-on user) === > "%OUT%"
taskkill /f /im spacerrecycle.exe >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul

echo. >> "%OUT%"
echo === create an interactive task === >> "%OUT%"
schtasks /delete /tn srapp /f >> "%OUT%" 2>&1
schtasks /create /tn srapp /tr "%APP%" /sc once /st 23:59 /ru "%USERNAME%" /it /f >> "%OUT%" 2>&1
echo create exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === run it === >> "%OUT%"
schtasks /run /tn srapp >> "%OUT%" 2>&1
echo run exit=%ERRORLEVEL% >> "%OUT%"
ping -n 30 127.0.0.1 > nul

echo. >> "%OUT%"
echo === is it running, and in which session === >> "%OUT%"
tasklist /fi "imagename eq spacerrecycle.exe" >> "%OUT%" 2>&1
echo done
