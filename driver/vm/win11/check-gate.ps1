<#
.SYNOPSIS
    Checks that the paid controls are actually disabled for a non-subscriber,
    and reads back what the app says about the Store.

.DESCRIPTION
    "The stored flags are false" and "the user cannot operate the control" are
    different claims, and only the second one is the feature. This reads
    IsEnabled off the accessibility tree, which is what the UI thread reports to
    assistive technology and to the input system alike - so a control that comes
    back disabled here is one a user cannot click.

    Also dumps the subscription block, because the source string it shows is the
    answer the whole exercise was for: "Microsoft Store (live)" means the app
    asked the Store and got a real reply, as opposed to falling back to
    unpackaged or not-configured.
#>
param(
    [string]$Aumid = "xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle"
)

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\gate-check.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

Get-Process -Name sparecycle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process "shell:appsFolder\$Aumid"
$lines += "session: $((Get-Process -Id $PID).SessionId)"

$root = [System.Windows.Automation.AutomationElement]::RootElement
$window = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $cond = New-Object System.Windows.Automation.AndCondition(
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
            [System.Windows.Automation.ControlType]::Window)),
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty, 'SpaceRecycle')))
    $window = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
    if ($window) { break }
}
if (-not $window) { $lines += 'FAIL: no window'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

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

# Scroll to the end so the quarantine radios and the subscription block are realised.
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

# Every radio button, with its enabled state. The OneDrive quarantine option is
# the one under test: it must be off for a non-subscriber.
$lines += '--- radio buttons ---'
$radioCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::RadioButton)
$radios = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $radioCond)
foreach ($r in $radios) {
    try {
        $lines += ("  name='{0}' enabled={1} selected={2}" -f $r.Current.Name, $r.Current.IsEnabled,
            ($r.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Current.IsSelected))
    } catch {
        $lines += "  name='$($r.Current.Name)' enabled=$($r.Current.IsEnabled) (selection unreadable)"
    }
}

# Every button, to see whether Connect OneDrive is reachable.
$lines += '--- buttons ---'
$btnCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::Button)
foreach ($b in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $btnCond)) {
    try { $lines += ("  name='{0}' enabled={1}" -f $b.Current.Name, $b.Current.IsEnabled) } catch { }
}

$lines += '--- subscription block ---'
$texts = @()
foreach ($n in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                               [System.Windows.Automation.Condition]::TrueCondition)) {
    try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $texts += $t.Trim() } } catch { }
}
$start = [Array]::IndexOf($texts, 'Subscription')
if ($start -ge 0) {
    $end = [Math]::Min($start + 8, $texts.Count - 1)
    for ($i = $start; $i -le $end; $i++) { $lines += "  $($texts[$i])" }
} else {
    $lines += '  Subscription section not found'
}

$lines | Set-Content $log -Encoding UTF8
