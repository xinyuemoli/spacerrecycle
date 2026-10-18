<#
.SYNOPSIS
    Launches the installed MSIX from the Store-style activation path and
    captures the app's own view of its entitlement.

.DESCRIPTION
    The question this answers cannot be answered any other way. Every existing
    test injects a fake Store answer, and running the exe gives a process with no
    package identity at all. An installed package has identity but, having been
    sideloaded rather than downloaded, no entitlements - so StoreContext is asked
    a real question and returns a real, empty answer. What the app then decides
    is exactly the behaviour the gate exists for.

    Launching goes through shell:appsFolder rather than the exe path on purpose.
    Package identity comes from activation, so starting the exe directly would
    produce a process that reports itself as unpackaged and the run would prove
    nothing.

    This must execute in the interactive session. It is started by a scheduled
    task with /it for that reason, which is also what makes the MSIX deployment
    itself possible.
#>
param(
    [string]$OutDir = "C:\sr-test\vm\entitlement",
    [string]$Aumid  = "xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle"
)

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\entitlement-result.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$lines += "aumid: $Aumid"
$lines += "session: $((Get-Process -Id $PID).SessionId)"

# --- launch ------------------------------------------------------------------
Get-Process -Name sparecycle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2

try {
    Start-Process "shell:appsFolder\$Aumid"
    $lines += 'launch issued'
} catch {
    $lines += "launch failed: $($_.Exception.Message)"
}

# --- wait for the window -----------------------------------------------------
# Polled rather than slept on: a fixed wait is either too short and records a
# failure that is really impatience, or long enough to make every run slow.
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

if (-not $window) {
    $lines += 'FAIL: app window never appeared'
    $proc = Get-Process -Name sparecycle -ErrorAction SilentlyContinue
    $lines += "process running: $(@($proc).Count)"
    $lines | Set-Content $log -Encoding UTF8
    exit 2
}

$wr = $window.Current.BoundingRectangle
$lines += ("window rect {0},{1} {2}x{3}" -f [int]$wr.Left, [int]$wr.Top, [int]$wr.Width, [int]$wr.Height)
Start-Sleep -Seconds 5

# --- read the entitlement out of the accessibility tree -----------------------
# The Settings view renders the source string, which is the actual answer:
# "store" means the app asked the Store and got a real response, "unpackaged"
# would mean the package identity was lost, "not_configured" would mean the
# compiled-in add-on ID is missing.
$lines += '--- text found in the window ---'
try {
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
        [System.Windows.Automation.Condition]::TrueCondition)
    $texts = @()
    foreach ($e in $all) {
        try {
            $n = $e.Current.Name
            if ($n -and $n.Trim().Length -gt 0 -and $n.Trim().Length -lt 200) { $texts += $n.Trim() }
        } catch { }
    }
    $texts | Select-Object -Unique | Select-Object -First 90 | ForEach-Object { $lines += "  $_" }
} catch {
    $lines += "  tree walk failed: $($_.Exception.Message)"
}

# --- capture ------------------------------------------------------------------
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
        $file = Join-Path $OutDir "$name.png"
        $crop.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
        $crop.Dispose()
        $lines += "saved $file (${w}x${h})"
    } catch {
        $lines += "capture $name failed: $($_.Exception.Message)"
    }
}

Save-Shot 'entitlement-overview'

# --- switch to Settings and capture it ---------------------------------------
$settingsCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, 'Settings')
$tab = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $settingsCond)
if ($tab) {
    $switched = $false
    foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                       [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
        try { $tab.GetCurrentPattern($pat).Invoke(); $switched = $true; break } catch { }
        try { $tab.GetCurrentPattern($pat).Select(); $switched = $true; break } catch { }
    }
    $lines += "settings switched: $switched"
    Start-Sleep -Seconds 6
    Save-Shot 'entitlement-settings'

    $lines += '--- text after switching to Settings ---'
    try {
        $all2 = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
            [System.Windows.Automation.Condition]::TrueCondition)
        $texts2 = @()
        foreach ($e in $all2) {
            try {
                $n = $e.Current.Name
                if ($n -and $n.Trim().Length -gt 0 -and $n.Trim().Length -lt 200) { $texts2 += $n.Trim() }
            } catch { }
        }
        $texts2 | Select-Object -Unique | Select-Object -First 90 | ForEach-Object { $lines += "  $_" }
    } catch { }
} else {
    $lines += 'settings tab not found'
}

$lines | Set-Content $log -Encoding UTF8
