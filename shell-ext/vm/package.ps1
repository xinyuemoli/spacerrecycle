# Stages the shell extension where the guest VM can reach it.
#
# The VM's shared folder is the developer's Downloads directory, mapped to
# "Downloads" in the guest, and it is configured read-only from the guest's
# side, so this is a one-way host-to-guest channel. That is enough to deliver
# build output and scripts; results have to come back another way (vmrun, or the
# caller reading stdout).
#
# The destination is derived from $env:USERPROFILE rather than written out. The
# literal path that used to be here named the developer's own Windows account,
# which is a leak in a public repository, and it silently wrote nowhere on any
# other machine.
[CmdletBinding()]
param(
    [string]$Dest = (Join-Path $env:USERPROFILE 'Downloads\spacerecycle-shell-test')
)

$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$bin = Join-Path $root 'bin'

foreach ($needed in @('SpaceRecycleShell.dll', 'menutest.exe')) {
    if (-not (Test-Path (Join-Path $bin $needed))) {
        throw "not built: $needed  (run build.cmd first)"
    }
}

if (Test-Path $Dest) { Remove-Item $Dest -Recurse -Force }
New-Item -ItemType Directory -Path (Join-Path $Dest 'bin\icons') -Force | Out-Null

Copy-Item (Join-Path $bin 'SpaceRecycleShell.dll') (Join-Path $Dest 'bin\') -Force
Copy-Item (Join-Path $bin 'menutest.exe') (Join-Path $Dest 'bin\') -Force
Copy-Item (Join-Path $bin 'icons\*.ico') (Join-Path $Dest 'bin\icons\') -Force
Copy-Item (Join-Path $root 'register.ps1') $Dest -Force
Copy-Item (Join-Path $root 'vm\vm-test.ps1') $Dest -Force
Copy-Item (Join-Path $root 'README.md') $Dest -Force

# A .cmd wrapper, because the guest's PowerShell may be 2.0 and its default
# execution policy on Windows 7 is Restricted.
@'
@echo off
REM Run from the guest: double-click, or from a command prompt in this folder.
powershell -NoProfile -ExecutionPolicy Bypass -File "%~dp0vm-test.ps1"
echo.
echo --- exit code %ERRORLEVEL% ---
pause
'@ | Set-Content -Path (Join-Path $Dest 'RUN-TEST.cmd') -Encoding ASCII

$zip = "$Dest.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $Dest '*') -DestinationPath $zip

Write-Host "staged:"
Get-ChildItem $Dest -Recurse -File | ForEach-Object { "  " + $_.FullName.Substring($Dest.Length + 1) + "  (" + $_.Length + " bytes)" }
Write-Host "zip   : $zip"