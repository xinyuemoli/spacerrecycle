# Runs the MSIX install in the user's interactive session.
#
# Add-AppxPackage cannot complete from session 0. The deployment log says why,
# and it is not a signature or policy problem: the signature validates, the
# certificate is trusted in all four stores, and the failure is
#
#     Failed to initialize PLM with error 0x80070005.
#     Failure in the PackagesInUseClosed state handler.
#
# PLM is the process lifetime manager, and it has nothing to attach to in a
# session with no interactive logon. vmrun's runProgramInGuest always lands in
# session 0, which is why every attempt so far failed at the same point - after
# the payload had already been written and validated.
#
# schtasks with /it runs the task in the session of the named user instead, and
# that is the whole fix. /rl highest is not needed: the certificate is already
# trusted machine-wide, so the install needs no elevation.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\session-install-result.txt'
$lines = @()

# Confirm there is an interactive session to target before relying on one.
$sessions = & query user 2>&1
$lines += '--- query user ---'
$lines += ($sessions | ForEach-Object { "  $_" })

$taskName = 'srinstall'
& schtasks /delete /tn $taskName /f 2>&1 | Out-Null

$action = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\sr-test\install-msix.ps1'
$lines += "create: $(& schtasks /create /tn $taskName /tr $action /sc once /st 23:59 /ru sruser /it /f 2>&1)"
$lines += "run:    $(& schtasks /run /tn $taskName 2>&1)"
Start-Sleep -Seconds 5

# Poll for the result file the task writes; a fixed sleep would either be too
# short and report a false failure, or waste a minute every run.
$deadline = (Get-Date).AddSeconds(180)
$done = $false
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 5
    if (Test-Path 'C:\sr-test\install-result.txt') {
        $c = Get-Content 'C:\sr-test\install-result.txt' -Raw -ErrorAction SilentlyContinue
        if ($c -match 'Add-AppxPackage') { $done = $true; break }
    }
}
$lines += "install-result.txt written: $done"

$q = & schtasks /query /tn $taskName /v /fo list 2>&1 | Select-String 'Status|Last Result'
$lines += ($q | ForEach-Object { "  $($_.Line.Trim())" })

& schtasks /delete /tn $taskName /f 2>&1 | Out-Null

$lines | Set-Content $log -Encoding UTF8
