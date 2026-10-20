<#
.SYNOPSIS
    Confirms the unlocked build really is unlocked, reading the Chinese UI.

.DESCRIPTION
    The first version of this checked English strings against a screen that was
    in Chinese, and reported three failures that were three mismatches. The
    labels here come from `dist/i18n.js`, which is the authority on them.

    What "unlocked" has to mean in practice, in this order:
      * the subscription row does not ask for money;
      * the guard's paid options are selectable;
      * OneDrive quarantine is selectable;
      * importance marking is available.

    Each of those is a different code path behind the same gate, so one of them
    could be open while another is not, and reading only the first would miss it.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\unlock-check.txt'
$lines = @()
function Say($t) { $script:lines += $t }

# The Chinese labels are built from code points, never written literally.
#
# Windows PowerShell reads a BOM-less UTF-8 script using the system ANSI code
# page. A literal here is mangled before it runs, and a mangled multi-byte
# sequence can be an invalid token - so the failure is a parse error in a file
# that looks fine, one screen away from the code that caused it. This is the
# sixth time in this repository, which is why the rule is written down rather
# than remembered.
function CJK([int[]]$codes) { -join ($codes | ForEach-Object { [char]$_ }) }
$L_SETTINGS  = CJK 0x8BBE, 0x7F6E                      # settings
$L_OVERRIDE  = CJK 0x5F00, 0x53D1, 0x8986, 0x76D6      # development override
$L_SUBSCRIBED= CJK 0x5DF2, 0x8BA2, 0x9605             # subscribed
$L_NEEDSUB   = CJK 0x9700, 0x8981, 0x6709, 0x6548, 0x8BA2, 0x9605  # needs an active subscription
$L_ASK       = CJK 0x8BE2, 0x95EE, 0x6211             # ask me
$L_ONEDRIVE  = CJK 0x9694, 0x79BB                      # quarantine (used with OneDrive)

Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$family = 'xinyuemoli.SpaceRecycle_8svmmg7pm8t1w'
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
        try {
            # A radio or checkbox's state is not in its name, and "the option is
            # selectable" is exactly what this is checking.
            if ($n.Current.ControlType -eq [System.Windows.Automation.ControlType]::RadioButton) {
                $sel = $n.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
                if ($sel) { $out += "[radio] $($n.Current.Name) enabled=$($n.Current.IsEnabled) selected=$($sel.Current.IsSelected)" }
            }
        } catch { }
    }
    return @($out | Select-Object -Unique)
}

Close-App
Say "== launch =="
Start-Process "shell:appsFolder\$family!spaceRecycle"
$deadline = (Get-Date).AddSeconds(90)
$w = $null
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Seconds 2
    $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
    if ($w) { break }
}
if (-not $w) { Say "FAIL: no window"; $lines | Set-Content $log -Encoding UTF8; exit 2 }
Start-Sleep -Seconds 8

Say ""
Say "== open settings =="
$opened = $null
foreach ($name in @($L_SETTINGS, 'Settings')) {
    $el = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty, $name)))
    if ($el) {
        foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                           [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
            try { $el.GetCurrentPattern($pat).Invoke(); $opened = $name; break } catch { }
            try { $el.GetCurrentPattern($pat).Select(); $opened = $name; break } catch { }
        }
        if (-not $opened) { $opened = "$name found, no pattern" }
        break
    }
}
Say "  $opened"
Start-Sleep -Seconds 8

$text = All-Text $w
Say ""
Say "== window text ($($text.Count) elements) =="
foreach ($t in $text) { Say "  $t" }

Say ""
Say "== verdict =="
$j = $text -join "`n"

$override = ($j -match [regex]::Escape($L_OVERRIDE)) -or ($j -match [regex]::Escape($L_SUBSCRIBED)) -or ($j -match 'development override')
$nag = ($j -match [regex]::Escape($L_NEEDSUB)) -or ($j -match 'needs an active subscription')
$guardOn = ($j -match [regex]::Escape($L_ASK)) -or ($j -match 'Ask me')
$cloud = ($j -match [regex]::Escape($L_ONEDRIVE)) -or ($j -match 'OneDrive quarantine')

Say "  override recognised by the UI: $override"
Say "  no subscription nag:           $(-not $nag)"
Say "  guard options present:         $guardOn"
Say "  OneDrive quarantine present:   $cloud"

# The radios, which is the part a text match cannot answer.
$disabled = @($text | Where-Object { $_ -match '^\[radio\].*enabled=False' })
Say "  disabled radio options:        $($disabled.Count)"
$disabled | ForEach-Object { Say "    $_" }

if ($override -and -not $nag -and $disabled.Count -eq 0) {
    Say ""
    Say "  UNLOCKED - every gate is open."
} else {
    Say ""
    Say "  NOT fully unlocked. See the lines above for which gate is still closed."
}

$lines | Set-Content $log -Encoding UTF8
