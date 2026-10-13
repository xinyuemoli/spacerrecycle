# Builds a folder that shows all three overlay states side by side.
#
# The overlay handlers cannot report whether Explorer actually used them - they
# are only asked about paths, they never learn what was drawn. So the check has
# to be visual: this lays out marked and unmarked files, and the caller takes a
# screenshot of the guest after opening the folder.
#
# No param block: cmd/vmrun append a stray whitespace argument that PowerShell
# 2.0 binds to the first parameter.
$ErrorActionPreference = 'Continue'

$log = 'C:\sr-test\overlay-demo.log'
$o = New-Object System.Collections.Generic.List[string]
function L($t) { $o.Add([string]$t) | Out-Null; $o | Set-Content $log -Encoding UTF8 }

$root = 'C:\sr-test'
$dir = 'C:\sr-ovtest'
$menu = Join-Path $root 'bin\menutest.exe'

L "=== overlay demo folder ==="
if (-not (Test-Path $menu)) { L "FATAL: missing $menu"; exit 2 }

Remove-Item $dir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $dir -Force | Out-Null

$names = @('1-important.txt', '2-important.txt', '3-disposable.txt', '4-disposable.txt',
           '5-unmarked.txt', '6-unmarked.txt')
foreach ($n in $names) {
    Set-Content -Path (Join-Path $dir $n) -Value 'demo'
}

# 0 = important, 1 = disposable
& $menu set (Join-Path $dir '1-important.txt')  0 | Out-Null
& $menu set (Join-Path $dir '2-important.txt')  0 | Out-Null
& $menu set (Join-Path $dir '3-disposable.txt') 1 | Out-Null
& $menu set (Join-Path $dir '4-disposable.txt') 1 | Out-Null

L ""
L "folder: $dir"
foreach ($f in (Get-ChildItem $dir -File)) {
    $stream = "$($f.FullName):SpaceRecycle.Importance"
    $mark = '(unmarked)'
    try {
        $v = Get-Content -LiteralPath $stream -ErrorAction Stop
        $mark = [string]$v
    } catch { }
    L ("  {0,-20} {1}" -f $f.Name, $mark)
}

# Does each handler claim the right files? This is the same question Explorer
# asks, minus Explorer.
L ""
L "--- handler answers (independent of Explorer) ---"
$out = & $menu overlay (Join-Path $dir '1-important.txt') (Join-Path $dir '3-disposable.txt') (Join-Path $dir '5-unmarked.txt') 2>&1
foreach ($line in $out) { L $line }

L ""
L "DONE"