<#
.SYNOPSIS
    Reads the Settings view of the newly installed build.

.DESCRIPTION
    Closes the running app by the identity of the window rather than by process
    name. The earlier version looked for "sparecycle" while the executable is
    "spacerrecycle", so nothing was ever closed and the reads were of a stale
    process from a previous install. Everything downstream of that - the text,
    the entitlement source, the enabled state of the controls - described a
    build that was no longer on disk.

    Reads twice: once at the top of Settings for the delete-guard card, once
    scrolled down for the quarantine card and the subscription block, because
    both were changed and they are not visible together.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\settings-read.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$windowCond = New-Object System.Windows.Automation.AndCondition(
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Window)),
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, 'SpaceRecycle')))

# Close whatever is on screen, identified by the window rather than by name.
$existing = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
if ($existing) {
    $lines += "closing existing window pid=$($existing.Current.ProcessId)"
    Stop-Process -Id $existing.Current.ProcessId -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 4
}

$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith($pkg.InstallLocation, 'OrdinalIgnoreCase') } |
    ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
Start-Sleep -Seconds 3

$before = Get-Date
Start-Process 'shell:appsFolder\xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle'

$window = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($window) { break }
}
if (-not $window) { $lines += 'FAIL: no window'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

$p = Get-Process -Id $window.Current.ProcessId -ErrorAction SilentlyContinue
$lines += "window pid=$($p.Id) started=$($p.StartTime) fresh=$($p.StartTime -gt $before)"
$lines += "exe: $($p.Path)"
Start-Sleep -Seconds 6

# Settings
$settings = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, 'Settings')))
if ($settings) {
    foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                       [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
        try { $settings.GetCurrentPattern($pat).Invoke(); break } catch { }
        try { $settings.GetCurrentPattern($pat).Select(); break } catch { }
    }
}
Start-Sleep -Seconds 6

function Snapshot([string]$label) {
    $script:lines += "--- $label ---"
    $radioCond = New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::RadioButton)
    foreach ($r in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $radioCond)) {
        try {
            $script:lines += ("  enabled={0,-5} selected={1,-5} {2}" -f `
                $r.Current.IsEnabled, `
                $r.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Current.IsSelected, `
                $r.Current.Name)
        } catch { }
    }
    $script:lines += '  --- text ---'
    $texts = @()
    foreach ($n in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                                   [System.Windows.Automation.Condition]::TrueCondition)) {
        try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $texts += $t.Trim() } } catch { }
    }
    $texts | Select-Object -Unique | ForEach-Object { $script:lines += "  $_" }
}

Snapshot 'top of settings'

# Scroll down for the quarantine card and the subscription block.
try {
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                           [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($n in $all) {
        try {
            $sp = $n.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
            if ($sp.Current.VerticallyScrollable) { $sp.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, 100); break }
        } catch { }
    }
} catch { }
Start-Sleep -Seconds 4
Snapshot 'bottom of settings'

$lines | Set-Content $log -Encoding UTF8
