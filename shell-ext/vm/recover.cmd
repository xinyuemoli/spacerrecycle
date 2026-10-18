@echo off
REM Recovery check: did the shell come back, and why were the marks missing?
REM
REM The desktop is addressed through %USERPROFILE% rather than a literal path.
REM This harness previously carried the developer's own Windows user name, which
REM is a leak in a public repository and a name that does not exist on anyone
REM else's machine.
set D=%USERPROFILE%\Desktop
set OUT=C:\sr-test\recover.txt

echo === explorer === > "%OUT%"
tasklist /fi "imagename eq explorer.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === module === >> "%OUT%"
tasklist /m SpaceRecycleShell.dll >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === desktop listing === >> "%OUT%"
dir /b "%D%\sr-*.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === streams on one file === >> "%OUT%"
dir /r "%D%\sr-1-important.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === menu set attempt, with output === >> "%OUT%"
C:\sr-test\bin\menutest.exe set "%D%\sr-1-important.txt" 0 >> "%OUT%" 2>&1
C:\sr-test\bin\menutest.exe get "%D%\sr-1-important.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === streams after set === >> "%OUT%"
dir /r "%D%\sr-1-important.txt" >> "%OUT%" 2>&1

echo done