@echo off
REM Recovery check: did the shell come back, and why were the marks missing?
set OUT=C:\sr-test\recover.txt

echo === explorer === > "%OUT%"
tasklist /fi "imagename eq explorer.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === module === >> "%OUT%"
tasklist /m SpaceRecycleShell.dll >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === desktop listing === >> "%OUT%"
dir /b "%USERPROFILE%\Desktop\sr-*.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === streams on one file === >> "%OUT%"
dir /r "%USERPROFILE%\Desktop\sr-1-important.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === menu set attempt, with output === >> "%OUT%"
C:\sr-test\bin\menutest.exe set "%USERPROFILE%\Desktop\sr-1-important.txt" 0 >> "%OUT%" 2>&1
C:\sr-test\bin\menutest.exe get "%USERPROFILE%\Desktop\sr-1-important.txt" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === streams after set === >> "%OUT%"
dir /r "%USERPROFILE%\Desktop\sr-1-important.txt" >> "%OUT%" 2>&1

echo done