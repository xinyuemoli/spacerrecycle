@echo off
REM Second-stage diagnostics: which user owns the console session, and has
REM explorer loaded our overlay handler yet?
REM
REM Run as a file: vmrun appends a stray whitespace argument that tasklist
REM rejects outright when the command is passed inline.
set OUT=C:\sr-test\dump2.txt

echo === module load check === > "%OUT%"
tasklist /m SpaceRecycleShell.dll >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === console session owners === >> "%OUT%"
tasklist /v /fi "sessionname eq Console" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === explorer start time === >> "%OUT%"
wmic process where "name='explorer.exe'" get ProcessId,CreationDate,CommandLine /format:list >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === overlay slots === >> "%OUT%"
reg query "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === overlay clsid inproc === >> "%OUT%"
reg query "HKLM\SOFTWARE\Classes\CLSID\{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0D}\InprocServer32" >> "%OUT%" 2>&1

echo done