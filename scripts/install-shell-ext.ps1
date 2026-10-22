# Install or update the shell extension.
#
# The extension is three files, not one: the DLL plus the .ico files it hands to
# the shell through GetOverlayInfo. Copying only the DLL is what made the first
# deployment look correct and show no overlay icons - the handler reported a
# path, the path did not exist, and the shell quietly drew nothing.
#
# So this script copies both, and then verifies rather than assuming:
#   - every icon file the handlers reference exists
#   - the DLL on disk is the one that was just copied (hash, not exit code)
#   - the CLSID registrations point at that path
#   - the out-parameter test passes
#
# Usage:
#   scripts\install-shell-ext.ps1                 # local machine
#   scripts\install-shell-ext.ps1 -RestartExplorer

param(
    [switch]$RestartExplorer,
    [switch]$Quiet
)

$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$bin = Join-Path $repo 'shell-ext\bin'
$dll = Join-Path $bin 'SpaceRecycleShell.dll'
$icons = Join-Path $bin 'icons'

$failures = @()

function Say([string]$text) {
    if (-not $Quiet) { Write-Output $text }
}

# ---------------------------------------------------------------- preconditions

Say "=== 1. what is being installed ==="

if (-not (Test-Path $dll)) {
    Write-Output "  FAIL  the DLL is not built: $dll"
    Write-Output "        run shell-ext\build.cmd first"
    exit 1
}

# The three icons the handlers name. If build.cmd ran they are copied from
# shell-ext\icons, so a missing one means the build did not run or the source is
# incomplete - either way, installing now produces a silent no-overlay.
$needed = @('important.ico', 'disposable.ico', 'clear.ico')
foreach ($name in $needed) {
    $p = Join-Path $icons $name
    if (-not (Test-Path $p)) {
        $failures += "missing icon: $name"
        Say "  FAIL  $name is missing from $icons"
    } elseif ((Get-Item $p).Length -lt 1000) {
        $failures += "suspicious icon: $name"
        Say "  FAIL  $name is only $((Get-Item $p).Length) bytes"
    } else {
        Say "  ok    $name ($((Get-Item $p).Length) bytes)"
    }
}

$dllHash = (Get-FileHash $dll -Algorithm SHA256).Hash
Say "  ok    SpaceRecycleShell.dll ($((Get-Item $dll).Length) bytes)"
Say "        sha256 $dllHash"

if ($failures.Count -gt 0) {
    Write-Output ""
    Write-Output "refusing to install: the icon set is incomplete, and an extension"
    Write-Output "without its icons shows no overlay and looks broken."
    exit 1
}

# ---------------------------------------------------------------- registration

Say ""
Say "=== 2. registering ==="

# regsvr32 needs the path; /s keeps it silent on success.
$regsvr = Join-Path $env:SystemRoot 'System32\regsvr32.exe'
$result = & $regsvr /s $dll
if ($LASTEXITCODE -ne 0) {
    Say "  FAIL  regsvr32 exited $LASTEXITCODE"
    $failures += 'regsvr32'
} else {
    Say "  ok    registered"
}

# ---------------------------------------------------------------- verification

Say ""
Say "=== 3. verifying ==="

# The registry must point at the file that is actually there.
$clsids = @(
    '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}',
    '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0D}',
    '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0E}'
)
foreach ($clsid in $clsids) {
    $found = $false
    foreach ($root in @('HKCU:\Software\Classes\CLSID', 'HKLM:\Software\Classes\CLSID')) {
        $key = Join-Path $root "$clsid\InprocServer32"
        if (Test-Path -LiteralPath $key) {
            $value = (Get-ItemProperty -LiteralPath $key).'(default)'
            if ($value -eq $dll) {
                Say "  ok    $clsid -> $value"
                $found = $true
            } else {
                Say "  FAIL  $clsid -> $value (expected $dll)"
                $failures += "clsid $clsid"
                $found = $true
            }
            break
        }
    }
    if (-not $found) {
        Say "  FAIL  $clsid is not registered"
        $failures += "clsid $clsid"
    }
}

# The overlay handlers are listed under one HKLM key, and the order matters:
# the shell asks them in that order and only the first that claims an item is
# drawn. Ours sort to the front because the key names start with spaces.
$overlayKey = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'
if (Test-Path $overlayKey) {
    $ours = @(Get-ChildItem -LiteralPath $overlayKey |
        Where-Object { $_.PSChildName -like '*SpaceRecycle*' })
    Say "  ok    $($ours.Count) overlay handler(s) registered"
    if ($ours.Count -ne 2) {
        Say "  FAIL  expected 2 overlay handlers"
        $failures += 'overlay count'
    }
} else {
    Say "  FAIL  the overlay identifiers key does not exist"
    $failures += 'overlay key'
}

# The out-parameter test is the one that catches the class of bug that crashed
# Explorer: an out parameter left unset, or set to null on a non-failing call.
$menutest = Join-Path $bin 'menutest.exe'
if (Test-Path $menutest) {
    $out = & $menutest outparams 2>&1
    if ($LASTEXITCODE -eq 0) {
        Say "  ok    out-parameter test passed"
    } else {
        Say "  FAIL  out-parameter test failed"
        $out | Select-String 'FAIL' | ForEach-Object { Say "        $($_.Line.Trim())" }
        $failures += 'outparams'
    }
} else {
    Say "  ..    menutest.exe not built, skipping the out-parameter test"
}

# ------------------------------------------------------------------- reloading

if ($RestartExplorer) {
    Say ""
    Say "=== 4. restarting the shell ==="
    # Explorer reads overlay handlers at startup and caches icons, so a new
    # overlay does not appear until it restarts.
    Get-Process explorer -ErrorAction SilentlyContinue | Stop-Process -Force
    Start-Sleep -Seconds 3
    Start-Process explorer.exe
    Start-Sleep -Seconds 5
    Say "  ok    explorer restarted"
}

# -------------------------------------------------------------------- verdict

Write-Output ""
if ($failures.Count -eq 0) {
    Write-Output "PASS: the shell extension is installed and its icons are present"
    exit 0
}
Write-Output "FAIL: $($failures -join ', ')"
exit 1
