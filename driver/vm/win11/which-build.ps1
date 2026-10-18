<#
.SYNOPSIS
    Identifies which binary is on screen, and closes it without guessing names.

.DESCRIPTION
    The previous version of this script looked for a process called
    "sparecycle". The executable is "spacerrecycle" - space + recycle - so the
    lookup matched nothing, every kill was a no-op, and Start-Process simply
    raised the already-running window belonging to an older build. The capture
    then reported that older build's text and entitlement, which looked like the
    fix had not worked and like the compiled-in add-on ID was missing, neither
    of which was true.

    The lesson is not "spell it right". It is that a process lookup by name is a
    guess that fails silently, so this resolves the process from the window it
    is about to read: whatever owns the SpaceRecycle window is what gets closed
    and what gets reported.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\which-build.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$lines += "session: $((Get-Process -Id $PID).SessionId)"

$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
$lines += "registered package: $($pkg.PackageFullName)"
$lines += "install location:   $($pkg.InstallLocation)"

$root = [System.Windows.Automation.AutomationElement]::RootElement
$windowCond = New-Object System.Windows.Automation.AndCondition(
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Window)),
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, 'SpaceRecycle')))

# --- close whatever is already on screen, by identity rather than by name ----
$existing = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
if ($existing) {
    $pid0 = $existing.Current.ProcessId
    $lines += "closing existing window, pid=$pid0"
    try {
        $p = Get-Process -Id $pid0 -ErrorAction Stop
        $lines += "  process name: $($p.ProcessName)"
        $lines += "  path:         $($p.Path)"
        $lines += "  started:      $($p.StartTime)"
        Stop-Process -Id $pid0 -Force -ErrorAction SilentlyContinue
    } catch { $lines += "  could not inspect: $($_.Exception.Message)" }
    Start-Sleep -Seconds 4
} else {
    $lines += 'no existing window'
}

# Every process holding the package's executable, found by path rather than name.
$pkgDir = $pkg.InstallLocation
Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($pkgDir, 'OrdinalIgnoreCase') } |
    ForEach-Object {
        $lines += "still running from the package: pid=$($_.Id) $($_.ProcessName)"
        Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue
    }
Start-Sleep -Seconds 3

$before = Get-Date
Start-Process 'shell:appsFolder\xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle'
$lines += 'launch issued'

$window = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($window) { break }
}
if (-not $window) { $lines += 'FAIL: no window'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

# --- who owns the window now -------------------------------------------------
$pid1 = $window.Current.ProcessId
$lines += "window owned by pid=$pid1"
try {
    $p = Get-Process -Id $pid1 -ErrorAction Stop
    $lines += "  process name: $($p.ProcessName)"
    $lines += "  path:         $($p.Path)"
    $lines += "  started:      $($p.StartTime)  (after this run began: $($p.StartTime -gt $before))"
    $lines += "  exe version:  $((Get-Item $p.Path).VersionInfo.FileVersion)"
} catch { $lines += "  inspect failed: $($_.Exception.Message)" }

Start-Sleep -Seconds 6
$lines += '--- rendered text ---'
$texts = @()
foreach ($n in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                               [System.Windows.Automation.Condition]::TrueCondition)) {
    try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $texts += $t.Trim() } } catch { }
}
$texts | Select-Object -Unique | ForEach-Object { $lines += "  $_" }

$lines | Set-Content $log -Encoding UTF8
