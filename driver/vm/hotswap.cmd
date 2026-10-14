@echo off
REM Replaces the loaded filter with a freshly built one, without a reboot.
REM
REM The unload callback was added specifically so this is possible; without it
REM the filter can never detach and every iteration costs a reboot. Two things
REM are easy to get wrong and both are handled here:
REM   - the service must be stopped first, because it holds the client port
REM   - unloading erases the Instances key, so DefaultInstance and Altitude have
REM     to be rewritten or the next load fails with a misleading 0x80070002
set OUT=C:\sr-test\deploy.txt

echo === stop the service first (it holds the port) === > "%OUT%"
taskkill /f /im srguard.exe >> "%OUT%" 2>&1
sc stop SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 6 127.0.0.1 > nul
sc delete SpaceRecycleGuard >> "%OUT%" 2>&1
ping -n 5 127.0.0.1 > nul

echo. >> "%OUT%"
echo === unload the filter === >> "%OUT%"
fltmc unload srdelete >> "%OUT%" 2>&1
echo unload exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === swap the image and rewrite the instance key === >> "%OUT%"
copy /y C:\sr-test\driver\srdelete.sys C:\Windows\System32\drivers\srdelete.sys >> "%OUT%" 2>&1
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances" /v DefaultInstance /t REG_SZ /d "srdelete Instance" /f >> "%OUT%" 2>&1
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances\srdelete Instance" /v Altitude /t REG_SZ /d "385100" /f >> "%OUT%" 2>&1
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances\srdelete Instance" /v Flags /t REG_DWORD /d 0 /f >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === load === >> "%OUT%"
fltmc load srdelete >> "%OUT%" 2>&1
echo load exit=%ERRORLEVEL% >> "%OUT%"
fltmc filters >> "%OUT%" 2>&1
echo done
