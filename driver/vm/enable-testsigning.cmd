@echo off
REM Prepare the guest to load a test kernel driver.
REM
REM testsigning lets Windows accept a driver signed with a locally created
REM certificate. nointegritychecks is belt-and-braces: on Windows 7 it can allow
REM an unsigned image outright, which removes signing from the list of things
REM that can go wrong while the driver itself is still being brought up.
REM
REM Run as a file - vmrun appends a stray whitespace argument that bcdedit
REM rejects outright.
set OUT=C:\sr-test\bcd.txt

echo === before === > "%OUT%"
bcdedit /enum {current} >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === enabling test signing === >> "%OUT%"
bcdedit /set testsigning on >> "%OUT%" 2>&1
echo exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === disabling integrity checks === >> "%OUT%"
bcdedit /set nointegritychecks on >> "%OUT%" 2>&1
echo exit=%ERRORLEVEL% >> "%OUT%"

echo. >> "%OUT%"
echo === after === >> "%OUT%"
bcdedit /enum {current} >> "%OUT%" 2>&1

echo done