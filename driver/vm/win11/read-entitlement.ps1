<#
.SYNOPSIS
    Launches the installed MSIX and reads what it decided about entitlement.

.DESCRIPTION
    Answers the one question none of the other tests can: what does the app do
    when it has real package identity but no Store entitlements? Every unit test
    injects a fake Store answer, and running the exe produces a process with no
    package identity at all, so neither reaches this state. A sideloaded MSIX
    reaches it exactly.

    Launching goes through shell:appsFolder rather than the exe path, because
    package identity comes from activation - starting the exe directly would
    report unpackaged and the run would prove nothing.

    Must execute in the interactive session, which is why a scheduled task with
    /it starts it. Two things depend on that: UIA can only see a desktop it can
    reach, and MSIX deployment needs a session with a logon.

    The Settings view is scrolled to the end because the subscription card is
    below the fold; without the scroll the capture shows the language and
    cleanup-rules sections and says nothing about entitlement.
#>
param(
    [string]$OutDir = "C:\sr-test\vm\entitlement",
    [string]$Aumid  = "xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle"
)

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\ent-text.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$lines += "session: $((Get-Process -Id $PID).SessionId)"

Get-Process -Name sparecycle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

Start-Process "shell:appsFolder\$Aumid"
$lines += 'launch issued'

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
if (-not $window) { $lines += 'FAIL: window never appeared'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

$wr = $window.Current.BoundingRectangle
$lines += ("window {0},{1} {2}x{3}" -f [int]$wr.Left, [int]$wr.Top, [int]$wr.Width, [int]$wr.Height)
Start-Sleep -Seconds 6

function Get-AllText($el) {
    $out = @()
    try {
        $nodes = $el.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                             [System.Windows.Automation.Condition]::TrueCondition)
        foreach ($n in $nodes) {
            try {
                $t = $n.Current.Name
                if ($t -and $t.Trim().Length -gt 0 -and $t.Trim().Length -lt 300) { $out += $t.Trim() }
            } catch { }
        }
    } catch { }
    return $out
}

function Save-Shot([string]$name) {
    try {
        $full = New-Object System.Drawing.Bitmap(1920, 1200)
        $g = [System.Drawing.Graphics]::FromImage($full)
        $g.CopyFromScreen(0, 0, 0, 0, $full.Size)
        $g.Dispose()
        $x = [Math]::Max(0, [int]$wr.Left); $y = [Math]::Max(0, [int]$wr.Top)
        $w = [Math]::Min([int]$wr.Width, $full.Width - $x)
        $h = [Math]::Min([int]$wr.Height, $full.Height - $y)
        $crop = New-Object System.Drawing.Bitmap($w, $h)
        $cg = [System.Drawing.Graphics]::FromImage($crop)
        $cg.DrawImage($full, (New-Object System.Drawing.Rectangle(0,0,$w,$h)),
                             (New-Object System.Drawing.Rectangle($x,$y,$w,$h)),
                             [System.Drawing.GraphicsUnit]::Pixel)
        $cg.Dispose(); $full.Dispose()
        $crop.Save((Join-Path $OutDir "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        $crop.Dispose()
        $lines += "saved $name"
    } catch { $lines += "capture $name failed: $($_.Exception.Message)" }
}

# --- switch to Settings -------------------------------------------------------
$lines += '--- looking for the Settings tab ---'
$settings = $window.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, 'Settings')))
if ($settings) {
    foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                       [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
        try { $settings.GetCurrentPattern($pat).Invoke(); break } catch { }
        try { $settings.GetCurrentPattern($pat).Select(); break } catch { }
    }
    Start-Sleep -Seconds 5
}

# --- scroll the settings pane to the bottom ----------------------------------
# The subscription card is the last section, so a capture without this shows the
# language and cleanup sections and nothing about entitlement at all.
$scrolled = 'no scrollable element found'
try {
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                           [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($n in $all) {
        try {
            $sp = $n.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
            if ($sp.Current.VerticallyScrollable) {
                $sp.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, 100)
                $scrolled = "scrolled ok"
                break
            }
        } catch { }
    }
} catch { $scrolled = "scroll failed: $($_.Exception.Message)" }
$lines += "scroll: $scrolled"
Start-Sleep -Seconds 4

Save-Shot 'settings-bottom'

$lines += '--- all text in the window ---'
Get-AllText $window | Select-Object -Unique | ForEach-Object { $lines += "  $_" }

$lines | Set-Content $log -Encoding UTF8
