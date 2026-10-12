# Runs inside the guest VM and verifies the shell extension end to end.
#
# Everything the host cannot check from outside: that the DLL registers on this
# Windows build, that the COM object can be created, that the submenu titles are
# intact, that the ADS is written and cleared, and that a second reader agrees.
#
# Output is written to a log next to the script AND echoed, so the result can be
# captured whether the caller is reading stdout or a file.
[CmdletBinding()]
param(
    [string]$WorkDir = "$env:TEMP\sr-shelltest"
)

$ErrorActionPreference = 'Continue'
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$log = Join-Path $here 'vm-test.log'
$lines = New-Object System.Collections.Generic.List[string]

function Say($text) {
    Write-Host $text
    $lines.Add([string]$text) | Out-Null
}

Say "=== SpaceRecycle shell extension: guest verification ==="
# Get-CimInstance needs PowerShell 3.0; Windows 7 SP1 ships 2.0.
$os = $null
try { $os = Get-CimInstance Win32_OperatingSystem -ErrorAction Stop } catch { $os = Get-WmiObject Win32_OperatingSystem }
Say "guest OS   : $($os.Caption) build $($os.Version)"
Say "64-bit     : $([Environment]::Is64BitOperatingSystem)"
Say "PowerShell : $($PSVersionTable.PSVersion)"
Say "script dir : $here"
Say ""

# ---------------------------------------------------------------- register
Say "--- register ---"
& (Join-Path $here 'register.ps1') -Action register 2>&1 | ForEach-Object { Say $_ }
Say ""

# ---------------------------------------------------------------- fixture
New-Item -ItemType Directory -Path $WorkDir -Force | Out-Null
$file = Join-Path $WorkDir 'sample-file.txt'
$folder = Join-Path $WorkDir 'sample-folder'
Set-Content -Path $file -Value 'hello' -NoNewline
New-Item -ItemType Directory -Path $folder -Force | Out-Null
Remove-Item "$file`:SpaceRecycle.Importance" -Force -ErrorAction SilentlyContinue
Remove-Item "$folder`:SpaceRecycle.Importance" -Force -ErrorAction SilentlyContinue
Say "fixture    : $file"
Say "fixture    : $folder"
Say ""

$menu = Join-Path $here 'bin\menutest.exe'

function Menu($args) {
    $out = & $menu $args 2>&1
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

$lines | Set-Content -Path $log -Encoding UTF8
Say "log written: $log"