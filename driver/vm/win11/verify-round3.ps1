<#
.SYNOPSIS
    Verifies the three things this round added that a unit test cannot reach.

.DESCRIPTION
    The guard mechanism itself was verified in an earlier round: a held file
    cannot be deleted from another process, and can be once the app closes. That
    result still holds and is not re-tested here.

    What is tested here is what the unit tests and the front-end harnesses
    cannot see, because each one only exists once a real package is installed
    and a real second process is involved:

      A. Autostart through the app id. The command string is unit-tested; its
         *effect* is not. If `shell:AppsFolder\<aumid>` activates the app without
         its package identity, the app resolves its entitlement as "unpackaged"
         and opens every paid feature for free - from inside the Store's own
         install directory, with nothing on screen to say so.

      B. The shell extension's new index writer. IndexAdd is about a hundred
         lines of new C++ that have never executed. If it writes a malformed
         file, the app reads an empty index and a right-click mark protects
         nothing - which is the gap it was written to close.

      C. The ADS round trip across processes. The format matches by construction
         and by test, but "the shell writes what the app reads" is a claim about
         two languages agreeing at runtime.

    B and C are driven through menutest.exe, which instantiates the real COM
    objects Explorer would and invokes the real menu entries. Nothing here
    simulates the extension: the code path that runs is the shipped one.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\round3-result.txt'
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

function Wait-Window([int]$seconds = 90) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
        if ($w) { Start-Sleep -Seconds 5; return $w }
    }
    return $null
}

function Window-Text($w) {
    $out = @()
    foreach ($n in $w.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                              [System.Windows.Automation.Condition]::TrueCondition)) {
        try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $out += $t.Trim() } } catch { }
    }
    return ($out | Select-Object -Unique)
}

function Show-Settings($w) {
    $settings = $w.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
        (New-Object System.Windows.Automation.PropertyCondition(
            [System.Windows.Automation.AutomationElement]::NameProperty, 'Settings')))
    if (-not $settings) { return $false }
    foreach ($pat in @([System.Windows.Automation.InvokePattern]::Pattern,
                       [System.Windows.Automation.SelectionItemPattern]::Pattern)) {
        try { $settings.GetCurrentPattern($pat).Invoke(); return $true } catch { }
        try { $settings.GetCurrentPattern($pat).Select(); return $true } catch { }
    }
    return $false
}

# --- install -----------------------------------------------------------------
Close-App
Say "== install =="
try {
    Add-AppxPackage -Path 'C:\sr-test\sr104.msix' -ForceApplicationShutdown -ErrorAction Stop
    Say "  Add-AppxPackage: OK"
} catch {
    Say "  Add-AppxPackage: FAILED - $($_.Exception.Message.Split([char]10)[0])"
}
$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
Say "  installed: $($pkg.PackageFullName)"
if (-not $pkg) { Say 'FATAL: not installed'; $lines | Set-Content $log -Encoding UTF8; exit 2 }

# --- A: autostart through the app id -----------------------------------------
Say ''
Say "== A. autostart through the app id =="
$runKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Run'
$command = "cmd /c start `"`" `"shell:AppsFolder\$aumid`""
Say "  command written: $command"
try {
    New-Item -Path $runKey -Force | Out-Null
    Set-ItemProperty -Path $runKey -Name 'SpaceRecycle' -Value $command
    Say "  Run key: written"
} catch { Say "  Run key: FAILED - $($_.Exception.Message)" }

# The value is read back the way the app's own is_enabled() reads it, so a
# mismatch here is the mismatch the app would see.
$readBack = (Get-ItemProperty -Path $runKey -Name 'SpaceRecycle' -ErrorAction SilentlyContinue).SpaceRecycle
Say "  Run key reads back: $readBack"
Say "  round-trips exactly: $($readBack -eq $command)"

# Launched through the registered command rather than by path, because that is
# the whole question: does this preserve the package identity?
Close-App
Say "  launching through the Run key command..."
cmd.exe /c $command | Out-Null
$w = Wait-Window 90
if (-not $w) {
    Say "  FAIL: nothing appeared"
} else {
    $p = Get-Process -Id $w.Current.ProcessId -ErrorAction SilentlyContinue
    Say "  app pid=$($p.Id)"
    Show-Settings $w | Out-Null

    # Polled, not sampled once.
    #
    # The first version of this read the window a fixed six seconds after the
    # click, found no entitlement line, and reported "FAILED - the app does not
    # think it has a package identity". It had read nothing at all. Concluding a
    # negative from absent data is the mistake this whole round is about, and
    # the tool was making it.
    $text = @()
    $waited = 0
    while ($waited -lt 45) {
        Start-Sleep -Seconds 3
        $waited += 3
        $text = Window-Text $w
        if ($text | Where-Object { $_ -match 'Checked via' }) { break }
    }
    Say "  waited ${waited}s for the entitlement line"

    $store = $text | Where-Object { $_ -match 'Checked via' }
    $sub = $text | Where-Object { $_ -match 'Not subscribed|Subscribed|订阅' }
    Say "  identity line: $(if ($store) { $store } else { '(none read)' })"
    Say "  subscription line: $(if ($sub) { $sub } else { '(none read)' })"

    if ($text.Count -eq 0) {
        Say "  INCONCLUSIVE: the window exposed no text, so nothing was read."
        Say "                This says nothing about the app's identity."
    } elseif (-not $store) {
        Say "  INCONCLUSIVE: text was read ($($text.Count) elements) but no"
        Say "                entitlement line was among it - the tab may not have"
        Say "                opened. Not a statement about identity."
    } elseif ($store -match 'Microsoft Store') {
        Say "  VERDICT A: PASS - identity survived the app-id launch."
    } else {
        Say "  VERDICT A: FAIL - the app does not think it has a package identity:"
        Say "             $store"
        Say "             Everything keyed on identity is on the wrong branch."
    }
}

# --- B: the shell extension's index writer -----------------------------------
Say ''
Say "== B. menutest writes a mark, and the index records it =="
$aTarget = 'C:\sr-test\round3-important.txt'
Set-Content -Path $aTarget -Value 'important' -Encoding ASCII
Remove-Item 'C:\ProgramData\SpaceRecycle\marked.txt' -Force -ErrorAction SilentlyContinue

$mt = 'C:\sr-test\menutest.exe'
$dll = 'C:\sr-test\SpaceRecycleShell.dll'
Say "  --- menutest list ---"
$listOut = & $mt list $aTarget $dll 2>&1
$listOut | ForEach-Object { Say "    $_" }

# The important entry's index is read out of the enumeration rather than
# assumed, so inserting a level later cannot silently make this test write a
# different one.
$idx = $null
$prev = ''
foreach ($line in $listOut) {
    if ($prev -match 'important' -or $prev -match '\u91cd\u8981') {
        $m = [regex]::Match($line, '(?:index|idx)\s*[=:]\s*(\d+)')
        if ($m.Success) { $idx = [int]$m.Groups[1].Value; break }
    }
    $m2 = [regex]::Match($line, '^\s*\[?(\d+)\]?\s*.*(important)')
    if ($m2.Success) { $idx = [int]$m2.Groups[1].Value; break }
    $prev = $line
}
if ($null -eq $idx) { $idx = 0; Say "  (index not parsed; falling back to 0)" }
Say "  using index $idx"

Say "  --- menutest set ---"
$setOut = & $mt set $aTarget $idx $dll 2>&1
$setOut | ForEach-Object { Say "    $_" }

Say "  --- menutest get (independent reader) ---"
$getOut = & $mt get $aTarget $dll 2>&1
$getOut | ForEach-Object { Say "    $_" }
$sawImportant = ($getOut | Out-String) -match 'important'
Say "  independent reader sees a mark: $sawImportant"

Say "  --- the index the app reads ---"
$indexPath = 'C:\ProgramData\SpaceRecycle\marked.txt'
if (Test-Path $indexPath) {
    $indexText = Get-Content $indexPath -Raw
    Say "  exists, $((Get-Item $indexPath).Length) bytes"
    Get-Content $indexPath | ForEach-Object { Say "    $_" }
    Say "  contains the marked path: $($indexText -match [regex]::Escape($aTarget))"
    $bytes = [IO.File]::ReadAllBytes($indexPath)
    $bom = ($bytes.Length -ge 3 -and $bytes[0] -eq 0xEF -and $bytes[1] -eq 0xBB -and $bytes[2] -eq 0xBF)
    Say "  written with a BOM (serde_json would reject the app's file): $bom"
} else {
    Say "  FAIL: the index was not written at all"
}

# --- C: the app reads what the menu wrote ------------------------------------
Say ''
Say "== C. the app protects a file the menu marked =="
# The guard has to be standing. The policy file is machine-wide, so it is
# written directly; the key is action_marked, which is what guard.rs reads.
$policyDir = 'C:\ProgramData\SpaceRecycle'
New-Item -ItemType Directory -Path $policyDir -Force | Out-Null
Set-Content -Path (Join-Path $policyDir 'policy.ini') -Value @('[guard]', 'action_marked=deny') -Encoding ASCII

# The app's own store must be empty, so anything held came from the menu's mark
# rather than from a JSON file this script wrote. That is the difference between
# testing the new path and testing the old one.
$storeFiles = @(
    (Join-Path $env:APPDATA 'SpaceRecycle\importance.json'),
    (Join-Path $env:LOCALAPPDATA "Packages\$family\LocalCache\Roaming\SpaceRecycle\importance.json")
)
foreach ($f in $storeFiles) { Remove-Item $f -Force -ErrorAction SilentlyContinue }
Say "  app store cleared: $(($storeFiles | Where-Object { -not (Test-Path $_) }).Count)/$($storeFiles.Count)"

Close-App
$w2 = $null
Start-Process "shell:appsFolder\$aumid"
$w2 = Wait-Window 90
if (-not $w2) {
    Say "  FAIL: the app did not start"
} else {
    Start-Sleep -Seconds 6
    $text2 = Window-Text $w2
    $prot = $text2 | Where-Object { $_ -match 'held open|currently held|could not be protected' }
    Say "  protection status: $prot"

    Say "  --- delete attempt from another process ---"
    $delOut = & cmd.exe /c "del /f /q `"$aTarget`" 2>&1"
    Say "    del: $(($delOut | Out-String).Trim())"
    $survived = Test-Path $aTarget
    Say "    still there: $survived"

    Show-Settings $w2 | Out-Null
    Start-Sleep -Seconds 5
    $text3 = Window-Text $w2
    $prot2 = $text3 | Where-Object { $_ -match 'held open|currently held|could not be protected' }
    Say "  protection status after: $prot2"

    Close-App
    Start-Sleep -Seconds 2
    & cmd.exe /c "del /f /q `"$aTarget`" 2>&1" | Out-Null
    Say "  after the app closed, deleted: $(-not (Test-Path $aTarget))"

    if ($survived) {
        Say "  VERDICT C: a file marked only through the menu is protected, and the"
        Say "             protection comes from the extension's index rather than"
        Say "             from any store this script wrote."
    } else {
        Say "  VERDICT C: FAILED - the menu's mark did not reach the guard."
    }
}

Say ''
Say "== cleanup =="
Remove-ItemProperty -Path $runKey -Name 'SpaceRecycle' -Force -ErrorAction SilentlyContinue
Remove-Item $aTarget -Force -ErrorAction SilentlyContinue
Say "  Run key entry removed; test file removed"

$lines | Set-Content $log -Encoding UTF8
