<#
.SYNOPSIS
    Answers one question: does launching the app through its app id keep its
    package identity?

.DESCRIPTION
    The first attempt at this reported a failure. It had not read any text at
    all - the Settings screen was not open yet - and it concluded from the
    absence of data that the identity was missing. Those are different findings
    and only one of them is about the app, so this version separates them: it
    waits for the text to appear, and says "could not read" when it could not.

    Why it matters: an app started from its exe path inside WindowsApps runs
    without package identity. Every branch keyed on identity then takes the
    unpackaged route, which resolves the entitlement to "nothing is on sale
    here" and opens every paid feature for free - silently, from inside the
    Store's own install directory. The Registry Run value is the mechanism that
    is supposed to avoid that, and this is the only place its effect is visible.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\a-result.txt'
$lines = @()
function Say($t) { $script:lines += $t }

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$family = 'xinyuemoli.SpaceRecycle_8svmmg7pm8t1w'
$aumid = "$family!spaceRecycle"
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
    $pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
    if ($pkg) {
        Get-Process -ErrorAction SilentlyContinue |
            Where-Object { $_.Path -and $_.Path.StartsWith($pkg.InstallLocation, 'OrdinalIgnoreCase') } |
            ForEach-Object { Stop-Process -Id $_.Id -Force -ErrorAction SilentlyContinue }
    }
    Start-Sleep -Seconds 3
}

function All-Text($w) {
    $out = @()
    foreach ($n in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                              [System.Windows.Automation.Condition]::TrueCondition)) {
        try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $out += $t.Trim() } } catch { }
    }
    return @($out | Select-Object -Unique)
}

function Click-Settings($w) {
    # The nav label is localised, and a literal cannot be trusted here: Windows
    # PowerShell reads a BOM-less UTF-8 script with the system ANSI code page, so
    # a Chinese label written literally arrives mangled and matches nothing. Both
    # spellings are therefore built from code points next to the plain English
    # one, which is what an English-locale machine shows.
    $settingsZh = -join (@(0x8BBE, 0x7F6E) | ForEach-Object { [char]$_ })
    foreach ($name in @('Settings', $settingsZh)) {
        $el = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
            (New-Object System.Windows.Automation.PropertyCondition(
                [System.Windows.Automation.AutomationElement]::NameProperty, $name)))
        if ($el) {
            foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                               [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
                try { $el.GetCurrentPattern($pat).Invoke(); return $name } catch { }
                try { $el.GetCurrentPattern($pat).Select(); return $name } catch { }
            }
            # A nav tab in a WebView may expose neither pattern; clicking its
            # centre is the fallback, and it is what a person would do.
            try {
                $r = $el.Current.BoundingRectangle
                if ($r.Width -gt 0) {
                    Add-Type -AssemblyName System.Windows.Forms -ErrorAction SilentlyContinue
                    [System.Windows.Forms.Cursor]::Position = New-Object System.Drawing.Point(
                        [int]($r.X + $r.Width / 2), [int]($r.Y + $r.Height / 2))
                    return "$name (cursor moved to $([int]$r.X),$([int]$r.Y))"
                }
            } catch { }
            return "$name (found, no pattern)"
        }
    }
    return $null
}

Close-App
Say "== launch through the app id, the way the Run key does =="
$command = "cmd /c start `"`" `"shell:AppsFolder\$aumid`""
Say "  command: $command"
cmd.exe /c $command | Out-Null

$w = $null
$deadline = (Get-Date).AddSeconds(90)
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($w) { break }
}
if (-not $w) {
    Say "  FAIL: no window appeared at all"
    $lines | Set-Content $log -Encoding UTF8
    exit 2
}
$p = Get-Process -Id $w.Current.ProcessId -ErrorAction SilentlyContinue
Say "  pid=$($p.Id)  path=$($p.Path)"
Say "  the path alone proves nothing: an exe started directly from WindowsApps"
Say "  has the same path and no identity."

Start-Sleep -Seconds 8
Say ""
Say "== before touching the tabs =="
$t0 = All-Text $w
Say "  elements with text: $($t0.Count)"
foreach ($t in $t0) { Say "    $t" }

Say ""
Say "== open Settings =="
$clicked = Click-Settings $w
Say "  click result: $(if ($clicked) { $clicked } else { 'the Settings element was not found' })"

# Polled rather than sampled once. The single biggest mistake in the first
# attempt was reading the window before the tab had rendered and treating the
# empty result as an answer.
$found = $null
$waited = 0
while ($waited -lt 60 -and -not $found) {
    Start-Sleep -Seconds 3
    $waited += 3
    $t = All-Text $w
    $found = $t | Where-Object { $_ -match 'Checked via|Subscribed|Not subscribed' }
}
Say "  waited ${waited}s for the entitlement text"

$t = All-Text $w
Say ""
Say "== window text after opening Settings =="
Say "  elements with text: $($t.Count)"
foreach ($x in $t) { Say "    $x" }

Say ""
Say "== verdict =="
$identity = $t | Where-Object { $_ -match 'Checked via' }
$sub = $t | Where-Object { $_ -match 'Subscribed|Not subscribed' }

if ($t.Count -eq 0) {
    Say "  INCONCLUSIVE - the window exposed no text at all, so nothing was read."
    Say "  This says nothing about the app's identity; the reading is what failed."
} elseif (-not $identity) {
    Say "  INCONCLUSIVE - text was read but no entitlement line was among it."
    Say "  The Settings tab may not have opened. Elements seen: $($t.Count)."
} elseif ($identity -match 'Microsoft Store') {
    Say "  PASS: $identity"
    Say "  The identity survived the app-id launch. This is the branch the Run"
    Say "  key has to produce, and the one that gates the paid features."
} else {
    Say "  FAIL: $identity"
    Say "  The app does not believe it has a package identity, so everything"
    Say "  keyed on identity is on the unpackaged branch."
}

$lines | Set-Content $log -Encoding UTF8
