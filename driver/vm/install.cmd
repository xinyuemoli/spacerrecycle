@echo off
REM Install and load the SpaceRecycle delete filter in the guest.
REM
REM Three things a minifilter needs, none of which is just "copy the .sys":
REM
REM 1. The test certificate in the machine's trusts, so the image verifies.
REM 2. A service entry, so the filter manager has something to load.
REM 3. An altitude under the service's Instances key, which is where a
REM    minifilter sits in the stack. Normally an INF writes that; without one
REM    the key has to be written by hand.
REM
REM Demand-start, not boot-start: a filter that only loads when asked cannot
REM stop the machine from booting, so a fault costs a reboot rather than the
REM recovery console.
REM
REM Run as a file - vmrun appends a stray whitespace argument that sc, reg and
REM fltmc all reject.
set OUT=C:\sr-test\driver-install.txt
set SRC=C:\sr-test\driver

echo === trust the test certificate === > "%OUT%"
certutil -addstore -f Root "%SRC%\sr-test.cer" >> "%OUT%" 2>&1
echo root exit=%ERRORLEVEL% >> "%OUT%"
certutil -addstore -f TrustedPublisher "%SRC%\sr-test.cer" >> "%OUT%" 2>&1
echo publisher exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === install image === >> "%OUT%"
copy /y "%SRC%\srdelete.sys" C:\Windows\System32\drivers\srdelete.sys >> "%OUT%" 2>&1
echo copy exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === service (idempotent) === >> "%OUT%"
sc delete srdelete >> "%OUT%" 2>&1
sc create srdelete type= filesys start= demand binPath= System32\drivers\srdelete.sys >> "%OUT%" 2>&1
echo sc exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === altitude === >> "%OUT%"
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances" /v DefaultInstance /t REG_SZ /d "srdelete Instance" /f >> "%OUT%" 2>&1
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances\srdelete Instance" /v Altitude /t REG_SZ /d "385100" /f >> "%OUT%" 2>&1
reg add "HKLM\SYSTEM\CurrentControlSet\Services\srdelete\Instances\srdelete Instance" /v Flags /t REG_DWORD /d 0 /f >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === load === >> "%OUT%"
fltmc load srdelete >> "%OUT%" 2>&1
echo load exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === loaded filters === >> "%OUT%"
fltmc filters >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === instances === >> "%OUT%"
fltmc instances >> "%OUT%" 2>&1

echo done