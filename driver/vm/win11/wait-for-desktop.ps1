# Waits for the unattended Windows install to finish and for the desktop to
# come up, then reports the state that matters.
#
# The desktop is what this waits for, not just a reachable guest: the previous
# build of this machine had VMware Tools answering while it sat at the sign-in
# screen, which looks healthy to every check except the one that counts.

param(
  [string]$Vmx = "<vm-dir>\Windows 11 x64.vmx",
  [string]$User = "sruser",
  # From the environment: a password committed to a public repository is public.
  [string]$Password = $env:SR_VM_PASSWORD,
  [int]$InstallTimeoutMin = 60,
  [int]$DesktopTimeoutMin = 15
)

$vmrun = "C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe"
$work = "<repo>\_vmwork"
New-Item -ItemType Directory -Path $work -Force | Out-Null

function Guest([string[]]$Args) { & $vmrun -T ws -gu $User -gp $Password @Args 2>&1 }

Write-Output "=== waiting for the install to finish ==="
$deadline = (Get-Date).AddMinutes($InstallTimeoutMin)
$tools = $false
$lastReport = Get-Date
while ((Get-Date) -lt $deadline) {
  $out = Guest @("fileExistsInGuest", $Vmx, "C:\Windows\System32\kernel32.dll")
  if ($out -match "file exists") { $tools = $true; break }
  if (((Get-Date) - $lastReport).TotalMinutes -ge 5) {
    $gb = [math]::Round((Get-Item "<vm-dir>\Windows 11 x64.vmdk").Length / 1GB, 2)
    Write-Output "  ...still installing, disk $gb GB"
    $lastReport = Get-Date
  }
  Start-Sleep -Seconds 30
}
if (-not $tools) { Write-Output "install never completed"; exit 1 }
Write-Output "  VMware Tools is answering"

Write-Output "=== waiting for the desktop ==="
$probe = @'
@echo off
del /q C:\sr-state.txt 2>nul
tasklist /fi "imagename eq explorer.exe" | findstr /i explorer > C:\sr-state.txt 2>&1
query user >> C:\sr-state.txt 2>&1
echo --- arm log --- >> C:\sr-state.txt
type C:\sr-armlogon.log >> C:\sr-state.txt 2>&1
echo --- setuplog tail --- >> C:\sr-state.txt
powershell -NoProfile -Command "Get-Content C:\sr-setup.log -Tail 20" >> C:\sr-state.txt 2>&1
'@
Set-Content -Path "$env:TEMP\state.cmd" -Value $probe -Encoding ASCII
Guest @("copyFileFromHostToGuest", $Vmx, "$env:TEMP\state.cmd", "C:\state.cmd") | Out-Null

$desktop = $false
$deadline = (Get-Date).AddMinutes($DesktopTimeoutMin)
while ((Get-Date) -lt $deadline) {
  Guest @("runProgramInGuest", $Vmx, "C:\state.cmd") | Out-Null
  Guest @("copyFileFromGuestToHost", $Vmx, "C:\sr-state.txt", "$work\state.txt") | Out-Null
  if (Test-Path "$work\state.txt") {
    $txt = Get-Content "$work\state.txt" -Raw
    if ($txt -match "explorer" -and $txt -match $User) { $desktop = $true; break }
  }
  Start-Sleep -Seconds 20
}

Write-Output ""
if ($desktop) { Write-Output "=== DESKTOP IS UP ===" } else { Write-Output "=== NO DESKTOP ===" }
if (Test-Path "$work\state.txt") { Get-Content "$work\state.txt" -Raw }
