@echo off
REM Swap in a new build of the filter without rebooting.
REM
REM A loaded driver's image file is locked, so it has to be unloaded before it
REM can be replaced. Demand-start is what makes this possible at all.
set SRC=C:\sr-test\driver
set OUT=C:\sr-test\driver-update.txt

echo === unload === > "%OUT%"
fltmc unload srdelete >> "%OUT%" 2>&1
echo unload exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === replace image === >> "%OUT%"
copy /y "%SRC%\srdelete.sys" C:\Windows\System32\drivers\srdelete.sys >> "%OUT%" 2>&1
echo copy exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === reload === >> "%OUT%"
fltmc load srdelete >> "%OUT%" 2>&1
echo load exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === loaded === >> "%OUT%"
fltmc filters >> "%OUT%" 2>&1

echo done