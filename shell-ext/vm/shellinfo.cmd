@echo off
REM Before killing the shell, establish whether Windows will bring it back.
REM
REM AutoRestartShell=1 means winlogon restarts explorer.exe automatically when it
REM exits, which is what makes "taskkill explorer + wait" a safe way to make
REM Explorer re-read the overlay handler list. At 0 the session would be left
REM without a shell, and restarting it would need the console user's password.
set OUT=C:\sr-test\shellinfo.txt

echo === AutoRestartShell === > "%OUT%"
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v AutoRestartShell >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === Shell value === >> "%OUT%"
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v Shell >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === current explorer === >> "%OUT%"
tasklist /fi "imagename eq explorer.exe" >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === installed dotnet === >> "%OUT%"
dir /b C:\Windows\Microsoft.NET\Framework64 >> "%OUT%" 2>&1

echo done