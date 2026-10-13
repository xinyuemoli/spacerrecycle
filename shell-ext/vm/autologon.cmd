@echo off
REM Before rebooting: will the console come back by itself?
REM
REM AutoAdminLogon decides whether a reboot returns to the desktop on its own or
REM stops at the login screen, which matters because programs launched through
REM guest operations run in session 0 and cannot start a shell in session 1.
set OUT=C:\sr-test\autologon.txt

echo === winlogon autologon settings === > "%OUT%"
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v AutoAdminLogon >> "%OUT%" 2>&1
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v DefaultUserName >> "%OUT%" 2>&1
reg query "HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon" /v DefaultDomainName >> "%OUT%" 2>&1

echo. >> "%OUT%"
echo === local accounts === >> "%OUT%"
net user >> "%OUT%" 2>&1

echo done