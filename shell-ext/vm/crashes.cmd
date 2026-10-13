@echo off
REM Was the shell restarted, and did it die again?
REM
REM If Explorer started, loaded our overlay handler and then crashed, the event
REM log will name the faulting module - which decides whether this is a broken
REM DLL or simply winlogon not relaunching the shell.
set OUT=C:\sr-test\crashes.txt

echo === explorer present? === > "%OUT%"
tasklist /fi "imagename eq explorer.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === application errors, newest 15 === >> "%OUT%"
wevtutil qe Application /c:40 /rd:true /f:text /q:"*[System[(EventID=1000 or EventID=1001 or EventID=1002)]]" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === winlogon / shell events === >> "%OUT%"
wevtutil qe System /c:20 /rd:true /f:text >> "%OUT%" 2>&1

echo done