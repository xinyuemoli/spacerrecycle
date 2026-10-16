@echo off
REM Runs during the unattended Windows install, from the CD that carries
REM autounattend.xml. That CD has no fixed drive letter, so autounattend finds
REM this file by name and calls it; its path is not known in advance.
REM
REM Everything happens in the specialize pass, which runs as SYSTEM. The obvious
REM place would be FirstLogonCommands, but those run as the logged-on user, and
REM with UAC on an unelevated process cannot install a driver package - the
REM VMware Tools installer would fail silently and the machine would come up
REM unreachable, because every later step here talks to the guest through vmrun
REM and vmrun's guest operations need Tools running inside it.
REM
REM The tools CD is identified by setup64.exe: the Windows install media also has
REM a setup.exe at its root, so matching that name alone would try to reinstall
REM Windows over itself.
setlocal enabledelayedexpansion
set LOG=C:\sr-setup.log
echo === stage %1 at %DATE% %TIME% === >> "%LOG%"

if /i not "%~1"=="specialize" (
  echo stage %1 has nothing to do >> "%LOG%"
  goto :end
)

echo --- looking for the VMware Tools CD --- >> "%LOG%"
set FOUND=0
for %%d in (D: E: F: G: H: I: J:) do (
  if exist "%%d\setup64.exe" (
    echo found VMware Tools at %%d >> "%LOG%"
    set FOUND=1
    start /wait "" "%%d\setup64.exe" /s /v"/qn REBOOT=R ADDLOCAL=ALL"
    echo tools installer exit=!ERRORLEVEL! >> "%LOG%"
  )
)
if "!FOUND!"=="0" echo TOOLS CD NOT FOUND >> "%LOG%"

echo --- keep the desktop visible so screenshots show something --- >> "%LOG%"
powercfg /change monitor-timeout-ac 0 >> "%LOG%" 2>&1
powercfg /change standby-timeout-ac 0 >> "%LOG%" 2>&1
powercfg /hibernate off >> "%LOG%" 2>&1

echo --- stop Windows Update from competing for the machine --- >> "%LOG%"
sc config wuauserv start= demand >> "%LOG%" 2>&1
sc stop wuauserv >> "%LOG%" 2>&1

echo --- let the desktop finish starting without the lock screen --- >> "%LOG%"
reg add "HKLM\SOFTWARE\Policies\Microsoft\Windows\Personalization" /v NoLockScreen /t REG_DWORD /d 1 /f >> "%LOG%" 2>&1
reg add "HKLM\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System" /v EnableFirstLogonAnimation /t REG_DWORD /d 0 /f >> "%LOG%" 2>&1

:end
echo === stage %1 finished === >> "%LOG%"
endlocal
