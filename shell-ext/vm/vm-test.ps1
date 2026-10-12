# Runs inside the guest VM and verifies the shell extension end to end.
#
# Everything the host cannot check from outside: that the DLL registers on this
# Windows build, that the COM object can be created, that the submenu titles are
# intact, that the ADS is written and cleared, and that a second reader agrees.
#
# Windows 7 SP1 ships PowerShell 2.0, so: no $PSScriptRoot, no Get-CimInstance,
# no .NET 4.0 types, and no reliance on $MyInvocation - the root is an explicit
# parameter, because a log written to an unknown directory is a log nobody can
# find.
#
# Results go to <Root>\vm-test.log and are also echoed to stdout for callers
# that capture it.
#
# There is deliberately NO param block. When this runs as
# `cmd /c powershell -File ... > out 2>&1`, the quoting chain hands the script
# one stray whitespace argument - measured, not guessed: a script with no param
# block reports $args.Count = 1. Under PowerShell 2.0 that argument binds to the
# first parameter and silently overrides its default, so `Join-Path $Root ...`
# produced "\register.ps1". Plain assignments cannot be clobbered that way.

$ErrorActionPreference = 'Continue'

# The deployment root is fixed by our own staging, so a literal is the reliable
# answer; the script's own directory is only used if the literal is wrong.
$Root = 'C:\sr-test'
if (-not (Test-Path (Join-Path $Root 'register.ps1'))) {
    try {
        $here = Split-Path -Parent $MyInvocation.MyCommand.Path
        if ($here) { $Root = $here }
    } catch { }
}
$WorkDir = Join-Path $Root 'fixtures'

$log = Join-Path $Root 'vm-test.log'
$lines = New-Object System.Collections.Generic.List[string]

function Say($text) {
    Write-Host $text
    $lines.Add([string]$text) | Out-Null
    # Flushed every line: if the script dies partway, what it managed to report
    # is still on disk to be read back.
    $lines | Set-Content -Path $log -Encoding UTF8
}

Say "=== SpaceRecycle shell extension: guest verification ==="
Say "root       : $Root"

# Get-CimInstance needs PowerShell 3.0; Windows 7 SP1 ships 2.0.
$os = $null
try { $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop } catch { $os = Get-WmiObject Win32_OperatingSystem }
Say "guest OS   : $($os.Caption) build $($os.Version)"
# Is64BitOperatingSystem needs .NET 4.0, which a stock Windows 7 does not have -
# accessing the missing property yields $null rather than throwing, so guarding
# it with try/catch reports "32" on a 64-bit guest. The environment variables are
# what actually answer the question.
$bits = '32'
if ($env:PROCESSOR_ARCHITECTURE -eq 'AMD64') { $bits = '64' }
$bitsNote = if ($env:PROCESSOR_ARCHITEW6432) { "64-bit OS, 32-bit process (WOW64)" } else { "$bits-bit OS" }
Say "bitness    : $bitsNote"
Say "CLR        : $([Environment]::Version)"
Say "PowerShell : $($PSVersionTable.PSVersion)"
Say ""

# ---------------------------------------------------------------- register
Say "--- register the extension ---"
$reg = Join-Path $Root 'register.ps1'
if (-not (Test-Path $reg)) {
    Say "FATAL: missing $reg"
    exit 2
}
& $reg -Action register 2>&1 | ForEach-Object { Say $_ }
Say ""

# ---------------------------------------------------------------- fixture
New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
$file = Join-Path $WorkDir 'sample-file.txt'
$folder = Join-Path $WorkDir 'sample-folder'
# No -NoNewline: that parameter arrived in PowerShell 5.0, so on Windows 7's 2.0
# it fails parameter binding and the fixture is silently never created - which
# then looks exactly like the shell item API refusing a perfectly valid path.
Set-Content -Path $file -Value 'hello'
New-Item -ItemType Directory -Path $folder -Force | Out-Null
Remove-Item "$file`:SpaceRecycle.Importance" -Force -ErrorAction SilentlyContinue
Remove-Item "$folder`:SpaceRecycle.Importance" -Force -ErrorAction SilentlyContinue
Say "fixture    : $file  (exists: $(Test-Path $file))"
Say "fixture    : $folder  (exists: $(Test-Path $folder))"
if (-not (Test-Path $file)) {
    Say "FATAL: could not create the fixture file"
    exit 4
}
Say ""

$menu = Join-Path $Root 'bin\menutest.exe'
if (-not (Test-Path $menu)) {
    Say "FATAL: missing $menu"
    exit 3
}

function Menu([string[]]$menuArgs) {
    $out = & $menu $menuArgs 2>&1
    $out | ForEach-Object { Say $_ }
}

# ---------------------------------------------------------------- the run
Say "--- 1) submenu, unmarked ---"
Menu @('list', $file)
Say ""
Say "--- 2) mark as important (index 0) ---"
Menu @('set', $file, '0')
Say "--- 3) read the stream back ---"
Menu @('get', $file)
Say "--- 4) submenu should now show important checked ---"
Menu @('list', $file)
Say ""
Say "--- 5) mark as disposable (index 1) ---"
Menu @('set', $file, '1')
Menu @('get', $file)
Say "--- 6) clear (index 2) ---"
Menu @('set', $file, '2')
Menu @('get', $file)
Say ""
Say "--- 7) a folder must work too ---"
Menu @('set', $folder, '0')
Menu @('get', $folder)
Say ""

# ---------------------------------------------------------------- overlay slots
Say "--- overlay slots on this guest (for the overlay work) ---"
$key = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'
if (Test-Path $key) {
    $names = @(Get-ChildItem $key | Select-Object -ExpandProperty PSChildName)
    Say "registered : $($names.Count)  (Windows honours only the first 15)"
    $i = 0
    foreach ($n in $names) { $i++; if ($i -le 20) { Say ("  {0,2}. [{1}]" -f $i, $n) } }
} else {
    Say "  (no ShellIconOverlayIdentifiers key)"
}
Say ""

# ---------------------------------------------------------------- registry proof
Say "--- registration state ---"
$clsid = '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}'
$comKey = "HKCU:\Software\Classes\CLSID\$clsid\InprocServer32"
if (Test-Path $comKey) { Say "COM InprocServer32 : $((Get-ItemProperty $comKey).'(default)')" } else { Say "COM InprocServer32 : MISSING" }
foreach ($cls in @('*', 'Directory')) {
    $verb = "HKCU:\Software\Classes\$cls\shell\SpaceRecycle"
    if (Test-Path -LiteralPath $verb) {
        Say "verb [$cls] : $((Get-ItemProperty -LiteralPath $verb).ExplorerCommandHandler)"
    } else {
        Say "verb [$cls] : MISSING"
    }
}
Say ""
Say "DONE"
Say "log written: $log"