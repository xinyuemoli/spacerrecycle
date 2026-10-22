# Brings the Windows 11 test guest up with an interactive desktop and the app
# running on it, then reads the settings tab back through UI Automation.
#
# Three things about this VM made the obvious approaches fail, and each is
# handled explicitly rather than worked around:
#
#   Windows clears AutoAdminLogon and DefaultUserName after a successful
#   auto-logon, so a machine that has booted once comes up at the lock screen
#   with no session. The values are re-armed here before every reset, and the
#   session is then polled for rather than slept through.
#
#   vmrun's -interactive flag rejects an account even when it is logged on,
#   because the console sits behind LogonUI. A scheduled task with /it runs in
#   the logged-on user's session and needs no password.
#
#   A guest program started by vmrun lands in session 0, and an accessibility
#   client only sees its own session's desktop, so the automation has to run
#   from the same scheduled task mechanism.

param(
  [string]$Vmx = "<vm-dir>\Windows 11 x64.vmx",
  [string]$User = "sruser",
  # From the environment: a password committed to a public repository is public.
  [string]$Password = $env:SR_VM_PASSWORD,
  [int]$SessionTimeoutSec = 420,
  [switch]$SkipReset
)

$ErrorActionPreference = "Stop"
$vmrun = "C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe"

function VMRun { param([string[]]$Args) & $vmrun -T ws -gu $User -gp $Password @Args 2>&1 }
function Fetch { param([string]$Guest, [string]$Host_) VMRun @("copyFileFromGuestToHost", $Vmx, $Guest, $Host_) | Out-Null }
function Push { param([string]$Host_, [string]$Guest) VMRun @("copyFileFromHostToGuest", $Vmx, $Host_, $Guest) | Out-Null }
function RunIn { param([string]$Cmd) VMRun @("runProgramInGuest", $Vmx, $Cmd) | Out-Null }

function Wait-Tools {
  # Judged on what vmrun prints rather than on $LASTEXITCODE: the helper wraps
  # the call in a pipeline, and reading the exit code through a function call is
  # not reliable enough to wait eight minutes on. It reported "guest never came
  # back" for a guest that answered perfectly well afterwards.
  for ($i = 0; $i -lt 60; $i++) {
    $out = VMRun @("fileExistsInGuest", $Vmx, "C:\Windows\System32\kernel32.dll")
    if ($out -match "file exists") { return $true }
    Start-Sleep -Seconds 8
  }
  return $false
}

$work = "<repo>\_vmwork"
New-Item -ItemType Directory -Path $work -Force | Out-Null

# --- 1. re-arm auto-logon ---
if (-not $SkipReset) {
  Write-Output "=== re-arming auto-logon ==="
  $arm = @'
@echo off
set W=HKLM\SOFTWARE\Microsoft\Windows NT\CurrentVersion\Winlogon
reg add "%W%" /v AutoAdminLogon /t REG_SZ /d 1 /f >nul 2>&1
reg add "%W%" /v DefaultUserName /t REG_SZ /d sruser /f >nul 2>&1
reg add "%W%" /v DefaultDomainName /t REG_SZ /d SRWIN11 /f >nul 2>&1
reg add "%W%" /v DefaultPassword /t REG_SZ /d $Password /f >nul 2>&1
reg add "%W%" /v AutoLogonCount /t REG_DWORD /d 999 /f >nul 2>&1
echo armed
'@
  Set-Content -Path "$env:TEMP\arm.cmd" -Value $arm -Encoding ASCII
  Push "$env:TEMP\arm.cmd" "C:\arm.cmd"
  RunIn "C:\arm.cmd"

  Write-Output "=== resetting ==="
  VMRun @("reset", $Vmx, "hard") | Out-Null
  Start-Sleep -Seconds 40
}

if (-not (Wait-Tools)) { throw "guest never came back" }
Write-Output "  tools are up"

# --- 2. poll for the desktop ---
Write-Output "=== waiting for the interactive desktop ==="
$deadline = (Get-Date).AddSeconds($SessionTimeoutSec)
$sessionUp = $false
while ((Get-Date) -lt $deadline) {
  $probe = @'
@echo off
del /q C:\sr-desktop.txt 2>nul
tasklist /fi "imagename eq explorer.exe" | findstr /i explorer.exe > C:\sr-desktop.txt 2>&1
query user >> C:\sr-desktop.txt 2>&1
'@
  Set-Content -Path "$env:TEMP\dl.cmd" -Value $probe -Encoding ASCII
  Push "$env:TEMP\dl.cmd" "C:\dl.cmd"
  RunIn "C:\dl.cmd"
  Fetch "C:\sr-desktop.txt" "$work\desktop.txt"
  if (Test-Path "$work\desktop.txt") {
    $txt = Get-Content "$work\desktop.txt" -Raw
    if ($txt -match "explorer\.exe" -and $txt -match $User) { $sessionUp = $true; break }
  }
  Start-Sleep -Seconds 15
}
if (-not $sessionUp) { throw "no interactive session appeared within $SessionTimeoutSec seconds" }
Write-Output "  session is up"

# --- 3. launch the app on that desktop ---
Write-Output "=== launching the app ==="
Push "<repo>\driver\vm\win11\launch-app.cmd" "C:\sr-test\vm\launch-app.cmd"
RunIn "C:\sr-test\vm\launch-app.cmd"
Start-Sleep -Seconds 45

$check = @'
@echo off
tasklist /fi "imagename eq spacerrecycle.exe" > C:\sr-appcheck.txt 2>&1
'@
Set-Content -Path "$env:TEMP\ac.cmd" -Value $check -Encoding ASCII
Push "$env:TEMP\ac.cmd" "C:\ac.cmd"
RunIn "C:\ac.cmd"
Fetch "C:\sr-appcheck.txt" "$work\appcheck.txt"
$appTxt = Get-Content "$work\appcheck.txt" -Raw
if ($appTxt -notmatch "spacerrecycle\.exe") { throw "the app did not start: $appTxt" }
Write-Output "  app is running"

# --- 4. read the settings tab back ---
Write-Output "=== reading the settings tab ==="
foreach ($f in @("read-ui.ps1", "readui-task.cmd", "runui.cmd")) {
  Push "<repo>\driver\vm\win11\$f" "C:\sr-test\vm\$f"
}
RunIn "C:\sr-test\vm\runui.cmd"
Fetch "C:\sr-test\vm\runui.txt" "$work\runui.txt"

$result = Get-Content "$work\runui.txt" -Raw
Write-Output $result
