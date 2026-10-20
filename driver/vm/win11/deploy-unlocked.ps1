<#
.SYNOPSIS
    Installs the fully-unlocked build and sets it up for hands-on testing.

.DESCRIPTION
    Three things have to be true before the paid features are visible, and the
    third is the one that is easy to forget:

      1. the package is installed;
      2. it was built with the `dev-entitlement` feature, so the override exists
         at all;
      3. the config file says `devSubscriberOverride: true`.

    Done in that order because the config is read at launch. A config written
    before the package exists is fine - it is read later - but a launch before
    the config is written shows the unsubscribed screens, and the first
    impression of "it did not work" is hard to undo.

    The shell extension is registered too, elevated, because the icon overlays
    live in HKLM. Without it the right-click menu is missing and nothing has an
    overlay, which is half of what there is to look at.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\deploy-result.txt'
$lines = @()
function Say($t) { $script:lines += $t }

$family = 'xinyuemoli.SpaceRecycle_8svmmg7pm8t1w'
$pkgName = 'xinyuemoli.SpaceRecycle'

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
$windowCond = New-Object System.Windows.Automation.AndCondition(
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
        [System.Windows.Automation.ControlType]::Window)),
    (New-Object System.Windows.Automation.PropertyCondition(
        [System.Windows.Automation.AutomationElement]::NameProperty, 'SpaceRecycle')))

function Close-App {
    $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($w) { Stop-Process -Id $w.Current.ProcessId -Force -ErrorAction SilentlyContinue }
    $pkg = Get-AppxPackage -Name $pkgName -ErrorAction SilentlyContinue
    if ($pkg) {
        Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($pkg.InstallLocation, 'OrdinalIgnoreCase') } |
            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    }
    Start-Sleep -Seconds 3
}

# --- 1. install ---------------------------------------------------------------
Say "== 1. install =="
Close-App
try {
    Add-AppxPackage -Path 'C:\sr-test\sr105.msix' -ForceApplicationShutdown -ErrorAction Stop
    Say "  Add-AppxPackage: OK"
} catch {
    Say "  Add-AppxPackage: FAILED - $($_.Exception.Message.Split([char]10)[0])"
}
$pkg = Get-AppxPackage -Name $pkgName -ErrorAction SilentlyContinue
Say "  installed: $($pkg.PackageFullName)"
if (-not $pkg) { Say 'FATAL: not installed'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

# --- 2. verify the binary carries the override --------------------------------
Say ""
Say "== 2. the override is compiled in =="
$exe = Join-Path $pkg.InstallLocation 'spacerrecycle.exe'
$marker = "development override is enabled in the config file"
$carries = (Select-String -Path $exe -Pattern $marker -Encoding utf8 -List -ErrorAction SilentlyContinue) -ne $null
Say "  binary carries the override: $carries"
if (-not $carries) {
    Say "  FATAL: this package was built without -DevEntitlement."
    Say "         The config key does nothing in it. Nothing below will help."
    $lines | Set-Content $log -Encoding UTF8
    exit 3
}

# --- 3. write the config ------------------------------------------------------
Say ""
Say "== 3. config =="
# Written to every location the app might read, for the same reason the
# importance store is: a packaged app's %APPDATA% is redirected, and which path
# resolves is not visible from outside. Writing both is cheap and removes the
# question.
$candidates = @(
    (Join-Path $env:APPDATA 'SpaceRecycle'),
    (Join-Path $env:LOCALAPPDATA "Packages\$family\LocalCache\Roaming\SpaceRecycle")
)
$json = '{"devSubscriberOverride": true}'
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
foreach ($dir in $candidates) {
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    $path = Join-Path $dir 'config.json'
    # Merged rather than overwritten: the app stores its own settings here, and
    # replacing the file would reset the age thresholds the user may have set.
    $existing = @{}
    if (Test-Path $path) {
        try {
            $obj = Get-Content $path -Raw | ConvertFrom-Json
            foreach ($p in $obj.PSObject.Properties) { $existing[$p.Name] = $p.Value }
        } catch { }
    }
    $existing['devSubscriberOverride'] = $true
    [IO.File]::WriteAllText($path, ($existing | ConvertTo-Json -Depth 6), $utf8NoBom)
    Say "  wrote $path"
}
Say "  config contains: $json"

# --- 4. register the shell extension -----------------------------------------
Say ""
Say "== 4. shell extension =="
$regScript = 'C:\sr-test\shell-ext\register.ps1'
if (Test-Path $regScript) {
    $out = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $regScript -Action register 2>&1
    $out | Select-Object -Last 14 | ForEach-Object { Say "    $_" }
} else {
    Say "  register.ps1 not staged - skipping"
}

# Explorer caches overlay handlers at start. Without a restart the overlays do
# not appear even though the registration is correct, which reads as a broken
# extension.
if (Get-Process explorer -ErrorAction SilentlyContinue) {
    Stop-Process -Name explorer -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 4
    Say "  explorer restarted (overlay handlers are read at start)"
}

# --- 5. launch and read the screens ------------------------------------------
Say ""
Say "== 5. launch =="
Start-Process "shell:appsFolder\$family!spaceRecycle"
$deadline = (Get-Date).AddSeconds(90)
$w = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($w) { break }
}
if (-not $w) {
    Say "  FAIL: no window"
} else {
    Start-Sleep -Seconds 8
    $settings = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty, 'Settings')))
    if ($settings) {
        foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                           [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
            try { $settings.GetCurrentPattern($pat).Invoke(); break } catch { }
            try { $settings.GetCurrentPattern($pat).Select(); break } catch { }
        }
    }
    Start-Sleep -Seconds 8

    $text = @()
    foreach ($n in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                              [System.Windows.Automation.Condition]::TrueCondition)) {
        try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $text += $t.Trim() } } catch { }
    }
    $text = @($text | Select-Object -Unique)
    Say "  window text ($($text.Count) elements):"
    foreach ($t in $text) { Say "    $t" }

    Say ""
    Say "== checks =="
    # Built from code points: Windows PowerShell reads a BOM-less UTF-8 script
    # with the system ANSI code page, so a literal here is mangled before it can
    # match anything - and a mangled multi-byte sequence can be an invalid token,
    # which turns a string comparison into a parse error.
    function CJK([int[]]$codes) { -join ($codes | ForEach-Object { [char]$_ }) }
    $lSubscribed = CJK 0x5DF2, 0x8BA2, 0x9605         # subscribed
    $lNeedSub    = CJK 0x9700, 0x8981, 0x6709, 0x6548, 0x8BA2, 0x9605  # needs a subscription

    $joined = $text -join "`n"
    Say "  subscription shows as active: $(($joined -match 'development override|Subscribed') -or ($joined -match [regex]::Escape($lSubscribed)))"
    Say "  no subscription nag:          $(-not (($joined -match 'needs an active subscription') -or ($joined -match [regex]::Escape($lNeedSub))))"
    Say "  OneDrive quarantine offered:  $($joined -match 'OneDrive quarantine')"
}
Say ""
Say "== done =="

$lines | Set-Content $log -Encoding UTF8
