@echo off
REM Re-arms the automatic logon.
REM
REM This runs from a scheduled task at every startup, and that is the whole
REM point: Windows removes AutoAdminLogon and DefaultUserName once an automatic
REM logon has succeeded, so a machine that has booted once stops logging itself
REM in and comes up at the sign-in screen. Writing the values a single time
REM during setup therefore only works until the first reboot. Re-writing them on
REM every boot puts them back before Winlogon reads them.
REM
REM It is a scheduled task with an at-startup trigger rather than anything
REM that runs at logon, because a logon is precisely what is missing when this
REM matters.
set W=HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon

reg add "%W%" /v AutoAdminLogon   /t REG_SZ    /d 1            /f >nul 2>&1
reg add "%W%" /v DefaultUserName  /t REG_SZ    /d sruser       /f >nul 2>&1
reg add "%W%" /v DefaultDomainName /t REG_SZ   /d SRWIN11      /f >nul 2>&1
reg add "%W%" /v DefaultPassword  /t REG_SZ    /d <redacted-password>   /f >nul 2>&1
reg add "%W%" /v AutoLogonCount   /t REG_DWORD /d 999          /f >nul 2>&1

echo %DATE% %TIME% re-armed autologon >> C:\sr-armlogon.log
exit /b 0
