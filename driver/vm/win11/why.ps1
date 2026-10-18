# Everything needed to explain why the entitlement run produced no output.
#
# Written to a file and copied out, because runProgramInGuest returns no stdout
# at all. The command that produced this file was first run inline, printed
# nothing, and exited 0 - which reads like "the task never ran" when it is really
# "this channel never carries text".

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\why-result.txt'
$lines = @()

$lines += "user: $env:USERNAME"
$lines += "session: $((Get-Process -Id $PID).SessionId)"

$lines += '--- srcheck task ---'
$q = & schtasks /query /tn srcheck /v /fo list 2>&1
if ($q) {
    $q | Select-String 'Status|Last Result|Task To Run|Run As User|Scheduled Task State' |
        ForEach-Object { $lines += "  $($_.Line.Trim())" }
} else {
    $lines += '  task not found'
}

$lines += '--- C:\sr-test ---'
Get-ChildItem 'C:\sr-test' -File -ErrorAction SilentlyContinue |
    ForEach-Object { $lines += ("  {0,-34} {1,8} B  {2}" -f $_.Name, $_.Length, $_.LastWriteTime.ToString('HH:mm:ss')) }

$lines += '--- entitlement output dir ---'
Get-ChildItem 'C:\sr-test\vm\entitlement' -File -ErrorAction SilentlyContinue |
    ForEach-Object { $lines += "  $($_.Name)" }

$lines += '--- app process ---'
$p = Get-Process -Name sparecycle -ErrorAction SilentlyContinue
if ($p) {
    foreach ($x in $p) { $lines += "  pid=$($x.Id) session=$($x.SessionId) started=$($x.StartTime)" }
} else {
    $lines += '  not running'
}

$lines += '--- installed package ---'
$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
if ($pkg) { $lines += "  $($pkg.PackageFullName)" } else { $lines += '  not installed' }

# Run the capture script in the foreground, here, in this session, and keep the
# transcript. Scheduling it hid its errors: the task ran, the script crashed, and
# the only trace was a missing file.
$lines += '--- foreground run of check-entitlement.ps1 ---'
try {
    $out = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File 'C:\sr-test\check-entitlement.ps1' 2>&1
    $out | Select-Object -First 40 | ForEach-Object { $lines += "  $_" }
    $lines += "  exit: $LASTEXITCODE"
} catch {
    $lines += "  threw: $($_.Exception.Message)"
}

$lines += '--- result file now? ---'
$lines += "  exists: $(Test-Path 'C:\sr-test\entitlement-result.txt')"
if (Test-Path 'C:\sr-test\entitlement-result.txt') {
    Get-Content 'C:\sr-test\entitlement-result.txt' | ForEach-Object { $lines += "  $_" }
}

$lines | Set-Content $log -Encoding UTF8
