@echo off
REM Diagnostics for the guest.
REM
REM Run as a file, never as an inline command: vmrun appends a stray whitespace
REM argument to the command line it builds, and strict parsers - tasklist, reg,
REM whoami - reject it outright with "invalid argument". A .cmd file is invoked
REM by path and never sees the extra token.
set OUT=C:\sr-test\dump.txt

echo === whoami === > "%OUT%"
whoami >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === sessions === >> "%OUT%"
query session >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === processes === >> "%OUT%"
tasklist >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === explorer present? === >> "%OUT%"
tasklist /fi "imagename eq explorer.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === overlay modules loaded in any process === >> "%OUT%"
tasklist /m SpaceRecycleShell.dll >> "%OUT%" 2>&1

echo done