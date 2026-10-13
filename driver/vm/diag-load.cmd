@echo off
REM Why can the filter no longer be loaded?
set OUT=C:\sr-test\diag-load.txt

echo === image on disk === > "%OUT%"
dir C:\Windows\System32\drivers\srdelete.sys >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === staged copy === >> "%OUT%"
dir C:\sr-test\driver\srdelete.sys >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === service registration === >> "%OUT%"
sc query srdelete >> "%OUT%" 2>&1
sc qc srdelete >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === service key === >> "%OUT%"
reg query HKLM\SYSTEM\CurrentControlSet\Services\srdelete /s >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === try load again === >> "%OUT%"
fltmc load srdelete >> "%OUT%" 2>&1
echo load exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === filters === >> "%OUT%"
fltmc filters >> "%OUT%" 2>&1

echo done