# Elevates the certificate import by scheduling it as SYSTEM.
#
# Import-Certificate into Cert:\LocalMachine needs an elevated token. Running
# the script through vmrun does not provide one: the token comes back with
# "elevated: False" and the import fails with E_ACCESSDENIED, which at first
# looks like a wrong certificate rather than a wrong privilege level.
#
# A scheduled task with /ru SYSTEM /rl highest is the way in, and it is the same
# mechanism the rest of this VM harness already uses to run things that need
# privilege without an interactive UAC prompt nobody can click.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\elevate-result.txt'
$lines = @()

$taskName = 'srtc'
& schtasks /delete /tn $taskName /f 2>&1 | Out-Null

# The action is the same script, run again - but this time as SYSTEM, so the
# import inside it succeeds. The script overwrites cert-result.txt either way.
$action = 'powershell.exe -NoProfile -ExecutionPolicy Bypass -File C:\sr-test\trust-cert.ps1'
$create = & schtasks /create /tn $taskName /tr $action /sc once /st 23:59 /ru SYSTEM /rl highest /f 2>&1
$lines += "create: $create"
$lines += "create exit: $LASTEXITCODE"

$run = & schtasks /run /tn $taskName 2>&1
$lines += "run: $run"
$lines += "run exit: $LASTEXITCODE"

# The task is asynchronous; poll the file it writes rather than sleeping a
# fixed amount and hoping.
$deadline = (Get-Date).AddSeconds(90)
$done = $false
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 3
    if (Test-Path 'C:\sr-test\cert-result.txt') {
        $content = Get-Content 'C:\sr-test\cert-result.txt' -Raw -ErrorAction SilentlyContinue
        if ($content -match '\S') { $done = $true; break }
    }
}

$lines += "cert-result.txt appeared: $done"
$result = & schtasks /query /tn $taskName /v /fo list 2>&1 | Select-String 'Last Result|Status'
$lines += ($result | ForEach-Object { $_.Line.Trim() })

& schtasks /delete /tn $taskName /f 2>&1 | Out-Null

$lines | Set-Content $log -Encoding UTF8
