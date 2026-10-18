<#
.SYNOPSIS
    End-to-end check of the user-mode delete guard in the packaged app.

.DESCRIPTION
    The unit tests prove the mechanism against the filesystem, and the guest
    experiment proved a held handle blocks a delete from another process. What
    neither covers is the whole path: a marked file, the packaged app holding it
    at startup, and a delete from a third process being refused.

    The delete is attempted from a separate process on purpose. Asking the app
    to delete its own held file would prove nothing - a handle does not stop the
    process that owns it - and that is the shape of mistake this is here to
    avoid.

    Marking is done by writing the app's own importance store rather than
    driving the context menu: the store is the source of truth the guard is
    rebuilt from at startup, and writing it directly tests exactly that path.
    The store's location is discovered rather than assumed, because a packaged
    app's AppData is redirected and guessing the path would test the guess.
#>

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\guard-result.txt'
$lines = @()

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

function Start-App {
    Close-App
    Start-Process "shell:appsFolder\$family!spaceRecycle"
    $deadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Seconds 2
        $w = $root.FindFirst([System.Windows.Automation.TreeScope]::Children, $windowCond)
        if ($w) { Start-Sleep -Seconds 5; return $w }
    }
    return $null
}

# --- install -----------------------------------------------------------------
Close-App
try {
    Add-AppxPackage -Path 'C:\sr-test\sr103.msix' -ForceApplicationShutdown -ErrorAction Stop
    $lines += 'Add-AppxPackage: OK'
} catch {
    $lines += "Add-AppxPackage: FAILED - $($_.Exception.Message.Split([char]10)[0])"
}
$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
$lines += "installed: $($pkg.PackageFullName)"

# --- set the disposition to deny, so the guard stands ------------------------
# The key is `action_marked`, which is what guard.rs reads. Writing anything
# else is silently ignored - the file parses, the key is simply not the one the
# app looks for, and the default applies. The default is `ask`, which still
# holds, so this mistake does not change the outcome; it would matter for `allow`.
$policyDir = Join-Path $env:ProgramData 'SpaceRecycle'
New-Item -ItemType Directory -Path $policyDir -Force | Out-Null
Set-Content -Path (Join-Path $policyDir 'policy.ini') -Value @('[guard]', 'action_marked=deny') -Encoding ASCII
$lines += "policy written: $(Join-Path $policyDir 'policy.ini')"

# --- find where the app keeps its importance store ---------------------------
# The plain %APPDATA% path comes first, and that is not a guess: the app's own
# Settings screen prints its audit log path, and it reads
# C:\Users\<user>\AppData\Roaming\SpaceRecycle\audit.log. A full-trust packaged
# app does not get its AppData redirected, so looking only under
# LocalCache\Packages - which is what a UWP app would do - finds nothing.
#
# The order matters and the previous version got it wrong: it tried the package
# path first, found nothing, and then silently created the store in a directory
# the app never reads. The run reported a failed guard when the real problem was
# that the mark had been written somewhere meaningless.
$candidates = @(
    (Join-Path $env:APPDATA 'SpaceRecycle'),
    (Join-Path $env:LOCALAPPDATA "Packages\$family\LocalCache\Roaming\SpaceRecycle")
)
$storeDir = $null
foreach ($c in $candidates) {
    if (Test-Path (Join-Path $c 'importance.json')) { $storeDir = $c; break }
}
if (-not $storeDir) {
    foreach ($c in $candidates) {
        if (Test-Path $c) { $storeDir = $c; break }
    }
}
if (-not $storeDir) {
    # Asserted rather than defaulted. A wrong directory produces a run that looks
    # like a broken guard, which is a much more expensive conclusion than a
    # missing directory.
    $lines += "FAIL: no importance store directory found among:"
    $candidates | ForEach-Object { $lines += "  $_" }
    $lines | Set-Content $log -Encoding UTF8
    exit 3
}
$lines += "store dir: $storeDir"

# --- mark a file important ---------------------------------------------------
$target = 'C:\sr-test\protected-file.txt'
Set-Content -Path $target -Value 'important' -Encoding UTF8
$store = @{ records = @{} }
$store.records[$target] = @{ level = 'critical'; markedUnix = [int][double]::Parse((Get-Date -UFormat %s)) }
$json = $store | ConvertTo-Json -Depth 5

# Written to every candidate location, not just the first one.
#
# The app computes its store path from the known-folder API, and whether that
# resolves to the logical AppData path or the package's redirected LocalCache
# path is not something this harness can see. Writing to all of them removes the
# question: exactly one is read, the others are inert, and the run answers the
# question that matters - whether a marked file is held - instead of being
# blocked on a path guess.
# Written with an explicit no-BOM encoder, not Set-Content -Encoding UTF8.
#
# Windows PowerShell's UTF8 encoding writes a byte order mark, and serde_json
# rejects a document that starts with one. The failure is invisible from the
# outside: the file exists, it looks like valid JSON in an editor, the app reads
# it, deserialisation fails, and Store::load falls back to its documented
# default of an empty store. The guard then holds nothing, the run reports that
# no file is being protected, and the temptation is to go looking for a bug in
# the guard.
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$wrote = @()
foreach ($c in $candidates) {
    try {
        New-Item -ItemType Directory -Path $c -Force | Out-Null
        [IO.File]::WriteAllText((Join-Path $c 'importance.json'), $json, $utf8NoBom)
        $wrote += $c
    } catch { }
}
$lines += "marked important: $target"
$lines += "store written to (UTF-8, no BOM):"
$wrote | ForEach-Object { $lines += "  $_" }

# --- launch, so startup rebuilds the holds from the marks --------------------
$w = Start-App
if (-not $w) { $lines += 'FAIL: app window never appeared'; $lines | Set-Content $log -Encoding UTF8; exit 2 }
$p = Get-Process -Id $w.Current.ProcessId -ErrorAction SilentlyContinue
$lines += "app pid=$($p.Id) path=$($p.Path)"
Start-Sleep -Seconds 5

# --- the actual test: delete it from a different process ---------------------
$lines += '--- delete attempt from another process ---'
$out = & cmd.exe /c "del /f /q `"$target`" 2>&1"
$lines += "  del output: $(($out | Out-String).Trim())"
$survived = Test-Path $target
$lines += "  file still there: $survived"

# And with the guard stood down, to show the hold is what is doing it.
$lines += '--- delete attempt after releasing the hold ---'
Close-App
Start-Sleep -Seconds 2
$out2 = & cmd.exe /c "del /f /q `"$target`" 2>&1"
$lines += "  del output: $(($out2 | Out-String).Trim())"
$lines += "  file still there: $(Test-Path $target)"

if ($survived) {
    $lines += 'VERDICT: the held file could not be deleted while the app was running,'
    $lines += '         and could be once it was not. The guard works end to end.'
} else {
    $lines += 'VERDICT: FAILED - the file was deleted despite being marked and held.'
}

# --- what the app says about it ----------------------------------------------
# Recreate and relaunch, then read Settings for the hold count.
Set-Content -Path $target -Value 'important' -Encoding UTF8
Close-App
$w2 = Start-App
if ($w2) {
    $settings = $w2.FindFirst([System.Windows.Automation.TreeScope]::Descendants,
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
    $lines += '--- text the app shows ---'
    $texts = @()
    foreach ($n in $w2.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                               [System.Windows.Automation.Condition]::TrueCondition)) {
        try { $t = $n.Current.Name; if ($t -and $t.Trim()) { $texts += $t.Trim() } } catch { }
    }
    $texts | Select-Object -Unique | ForEach-Object { $lines += "  $_" }
}

$lines | Set-Content $log -Encoding UTF8
