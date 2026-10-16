@echo off
REM Starts the app on the guest's interactive desktop.
REM
REM vmrun's -interactive flag insists the credentials it was given belong to
REM whoever is logged on at the console, and it rejects the pair even when
REM `query user` shows that account active on session 1 - the console here is
REM sitting behind LogonUI, which vmrun does not accept as an interactive
REM logon. A scheduled task with /it runs in the logged-on user's session, needs
REM no password stored, and is started by the service control manager rather
REM than by vmrun, so it sidesteps the check entirely.
set OUT=C:\sr-launch.txt
set APP=C:\sr-test\app\spacerrecycle.exe

echo === previous instance === > "%OUT%"
taskkill /f /im spacerrecycle.exe >> "%OUT%" 2>&1
ping -n 3 127.0.0.1 > nul

echo. >> "%OUT%"
echo === create the interactive task === >> "%OUT%"
schtasks /delete /tn srapp /f >> "%OUT%" 2>&1
schtasks /create /tn srapp /tr "%APP%" /sc once /st 23:59 /ru sruser /it /f >> "%OUT%" 2>&1
echo create exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === run it === >> "%OUT%"
schtasks /run /tn srapp >> "%OUT%" 2>&1
echo run exit=%ERRORLEVEL% >> "%OUT%"

ping -n 40 127.0.0.1 > nul

echo. >> "%OUT%"
echo === is it running, and in which session === >> "%OUT%"
tasklist /fi "imagename eq spacerrecycle.exe" >> "%OUT%" 2>&1
tasklist /fi "imagename eq msedgewebview2.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === task result === >> "%OUT%"
schtasks /query /tn srapp /fo list | findstr /i "result status" >> "%OUT%" 2>&1
echo done
