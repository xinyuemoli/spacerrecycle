# Installs the rebuilt package and captures the Settings view for the two UI
# fixes, in one interactive-session run.
#
# Add-AppxPackage cannot complete from session 0 ("Failed to initialize PLM"),
# so this is started by a scheduled task with /it, the same as the first
# verification. Keeping the install and the capture in one run avoids the
# round-trip that made the first attempt read a result file the task had not
# finished writing.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\ui-fix-result.txt'
$lines = @()

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing
$outDir = 'C:\sr-test\vm\uifix'
New-Item -ItemType Directory -Path $outDir -Force | Out-Null

$lines += "session: $((Get-Process -Id $PID).SessionId)"

# Close the running copy before installing, not after. Add-AppxPackage fails with
# 0x80073D02 ("resources it modifies are currently in use") while the previous
# version is still running, and the first version of this script only killed it
# once the install had already failed - so the reinstall never happened and the
# capture showed the old build's UI, which looked like the fix not working.
Get-Process -Name sparecycle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 3
$still = Get-Process -Name sparecycle -ErrorAction SilentlyContinue
$lines += "app running before install: $(@($still).Count)"

# --- install -----------------------------------------------------------------
# -ForceApplicationShutdown, because killing sparecycle is not enough: the
# previous registration is still held by the WebView2 runtime processes the app
# spawned, and 0x80073D02 means one of them has a file open in the package.
# Windows exposes this switch precisely for replacing a running packaged app, so
# the fix is to use it rather than to hunt processes by name.
try {
    Add-AppxPackage -Path 'C:\sr-test\sr102.msix' -ForceApplicationShutdown -ErrorAction Stop
    $lines += 'Add-AppxPackage: OK'
} catch {
    $lines += "Add-AppxPackage: FAILED - $($_.Exception.Message.Split([char]10)[0])"
    # Second attempt after a full removal, which clears a registration the
    # in-place upgrade refused to replace.
    try {
        $old = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
        if ($old) { Remove-AppxPackage -Package $old.PackageFullName -ForceApplicationShutdown -ErrorAction SilentlyContinue }
        Start-Sleep -Seconds 3
        Add-AppxPackage -Path 'C:\sr-test\sr102.msix' -ForceApplicationShutdown -ErrorAction Stop
        $lines += 'Add-AppxPackage (after removal): OK'
    } catch {
        $lines += "Add-AppxPackage (after removal): FAILED - $($_.Exception.Message.Split([char]10)[0])"
    }
}
$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
$lines += "installed: $($pkg.PackageFullName)"

Start-Sleep -Seconds 3
Get-Process -Name sparecycle -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
Start-Sleep -Seconds 2
Start-Process 'shell:appsFolder\xinyuemoli.SpaceRecycle_8svmmg7pm8t1w!spaceRecycle'
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
Start-Sleep -Seconds 6

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

function Save-Shot([string]$name) {
    try {
        $full = New-Object System.Drawing.Bitmap(1920, 1200)
        $g = [System.Drawing.Graphics]::FromImage($full)
        $g.CopyFromScreen(0, 0, 0, 0, $full.Size); $g.Dispose()
        $x = [Math]::Max(0, [int]$wr.Left); $y = [Math]::Max(0, [int]$wr.Top)
        $w = [Math]::Min([int]$wr.Width, $full.Width - $x)
        $h = [Math]::Min([int]$wr.Height, $full.Height - $y)
        $crop = New-Object System.Drawing.Bitmap($w, $h)
        $cg = [System.Drawing.Graphics]::FromImage($crop)
        $cg.DrawImage($full, (New-Object System.Drawing.Rectangle(0,0,$w,$h)),
                             (New-Object System.Drawing.Rectangle($x,$y,$w,$h)),
                             [System.Drawing.GraphicsUnit]::Pixel)
        $cg.Dispose(); $full.Dispose()
        $crop.Save((Join-Path $outDir "$name.png"), [System.Drawing.Imaging.ImageFormat]::Png)
        $crop.Dispose(); $lines += "saved $name"
    } catch { $lines += "capture $name failed: $($_.Exception.Message)" }
}

# The guard section first, scrolled to the top of Settings.
Save-Shot 'guard-section'

# Then the quarantine section, which is further down.
try {
    $all = $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                           [System.Windows.Automation.Condition]::TrueCondition)
    foreach ($n in $all) {
        try {
            $sp = $n.GetCurrentPattern([System.Windows.Automation.ScrollPattern]::Pattern)
            if ($sp.Current.VerticallyScrollable) { $sp.SetScrollPercent([System.Windows.Automation.ScrollPattern]::NoScroll, 55); break }
        } catch { }
    }
} catch { }
Start-Sleep -Seconds 3
Save-Shot 'quarantine-section'

# --- read the states that were fixed ------------------------------------------
$lines += '--- radio buttons ---'
$radioCond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
    [System.Windows.Automation.ControlType]::RadioButton)
foreach ($r in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants, $radioCond)) {
    try { $lines += ("  enabled={0,-5} name='{1}'" -f $r.Current.IsEnabled, $r.Current.Name) } catch { }
}

$lines += '--- text ---'
$texts = @()
foreach ($n in $window.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                               [System.Windows.Automation.Condition]::TrueCondition)) {
    try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $texts += $t.Trim() } } catch { }
}
$texts | Select-Object -Unique | ForEach-Object { $lines += "  $_" }

$lines | Set-Content $log -Encoding UTF8
