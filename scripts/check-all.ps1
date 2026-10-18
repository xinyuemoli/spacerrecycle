# One place that runs everything, so "is it all green" has an answer that is not
# assembled by hand from several outputs.
$ErrorActionPreference = 'Continue'
$root = '<repo>'
Set-Location $root
$failed = @()

Write-Output "===== 1. Rust: lib + integration ====="
Push-Location src-tauri
$out = cmd /c "cargo test 2>&1"
Pop-Location
$totalPass = 0; $totalFail = 0
$out | Select-String "test result:" | ForEach-Object {
    Write-Output "  $($_.Line.Trim())"
    if ($_.Line -match '(\d+) passed; (\d+) failed') {
        $totalPass += [int]$Matches[1]; $totalFail += [int]$Matches[2]
    }
}
Write-Output "  ---- total $totalPass passed / $totalFail failed"
if ($totalFail -gt 0 -or ($out | Select-String "^error")) { $failed += "rust" }

Write-Output ""
Write-Output "===== 2. frontend harnesses ====="
Push-Location dist
$fp = 0; $ff = 0
foreach ($t in (Get-ChildItem _test_*.mjs | Sort-Object Name)) {
    $o = cmd /c "node $($t.Name) 2>&1"
    if ($LASTEXITCODE -eq 0) { $fp++; Write-Output "  ok   $($t.Name)" }
    else {
        $ff++; Write-Output "  FAIL $($t.Name)"
        $o | Where-Object { $_ -match "^FAIL|SyntaxError" } | Select-Object -First 3 | ForEach-Object { "         $_" }
    }
}
Pop-Location
Write-Output "  ---- $fp ok / $ff failed"
if ($ff -gt 0) { $failed += "frontend" }

Write-Output ""
Write-Output "===== 3. shell extension ====="
Push-Location shell-ext
$out = cmd /c "build.cmd 2>&1"
Pop-Location
if ($LASTEXITCODE -eq 0) { Write-Output "  ok   SpaceRecycleShell.dll built" } else { Write-Output "  FAIL shell-ext"; $failed += "shell-ext" }

Write-Output ""
Write-Output "===== 4. encoding: no mojibake, no BOM ====="
# The mojibake markers are built from code points rather than written literally.
#
# Windows PowerShell reads a BOM-less UTF-8 script using the system ANSI code
# page, so a literal here would be mangled before it ever ran - and a mangled
# multi-byte sequence can contain a quote, which breaks the parser rather than
# the check. Which is the same failure this check exists to catch, arriving
# through the check itself.
$markers = @(0x937C, 0x91A8, 0x951B, 0x9225, 0x9359, 0x949E, 0x951F) |
    ForEach-Object { [string][char]$_ }
$pattern = [string]::Join("|", $markers)
$bad = Get-ChildItem -Recurse -File -Include *.rs,*.js,*.mjs,*.json,*.md,*.html,*.ps1,*.cpp,*.h,*.toml |
    Where-Object { $_.FullName -notmatch '\\target\\|\\node_modules\\|\\.git\\|gen\\schemas|\\bin\\' } |
    Select-String -Pattern $pattern -ErrorAction SilentlyContinue
if ($bad) { Write-Output "  FAIL mojibake in $($bad.Count) places"; $bad | Select-Object -First 5 | ForEach-Object { "         $($_.Path):$($_.LineNumber)" }; $failed += "encoding" }
else { Write-Output "  ok   no mojibake" }

Write-Output ""
Write-Output "===== 5. leak check ====="
# The needles are built from code points rather than written literally.
#
# The first version hardcoded them, so this script matched itself and reported
# three leaking files that were three lines of the leak check. A check that
# finds itself is worse than no check: it fails on a clean tree, and the fix
# somebody reaches for is to stop running it.
$needles = @(
    [string][char]49 + [char]49 + [char]53 + [char]55 + [char]48,   # the old account number
    [string][char]120 + [char]105 + [char]97 + [char]111 + [char]100 + [char]117 + [char]111 + [char]116 + [char]101 + [char]99 + [char]104,
    [string][char]115 + [char]104 + [char]117 + [char]112 + [char]101 + [char]105
)
$needlePattern = [string]::Join("|", $needles)

$leak = 0
foreach ($c in (git rev-list HEAD 2>&1)) {
    $m = git log -1 --format='%B' $c 2>&1 | Out-String
    $real = [regex]::Matches($m, 'C:\\Users\\([^\s\\<]*)') | Where-Object { $_.Groups[1].Value -ne '' }
    if ($m -match $needlePattern -or $real.Count -gt 0) { $leak++ }
}
Write-Output "  commits with leaks in the message: $leak"

# Counted by file, not by matching line. The two agreed while every leak was a
# single line, which is exactly the kind of agreement that stops holding without
# anything saying so.
$contentLeaks = @(git grep -I -l -E $needlePattern -- . 2>$null | Where-Object { $_ })
Write-Output "  files containing a needle: $($contentLeaks.Count)"
if ($contentLeaks.Count -gt 0) {
    $contentLeaks | Select-Object -First 5 | ForEach-Object { "         $_" }
    $failed += "leak"
}
if ($leak -gt 0) { $failed += "leak" }

Write-Output ""
Write-Output "===== 6. remaining old-level references ====="
$old = git grep -n -E "Importance::(Critical|High|Normal|Low)|imp\.(critical|high|normal|low)" -- src-tauri dist shell-ext 2>$null
if ($old) { Write-Output "  FAIL:"; $old | ForEach-Object { "         $_" }; $failed += "old-levels" }
else { Write-Output "  ok   none" }

Write-Output ""
Write-Output "===== 7. the dev entitlement gate ====="
# Two halves, and both matter. With the feature off the paid features must be
# unreachable from a config file; with it on the override has to work, or the
# test package cannot test anything.
Push-Location src-tauri
$off = cmd /c "cargo test --lib 2>&1"
$on  = cmd /c "cargo test --lib --features dev-entitlement 2>&1"
Pop-Location
$offOk = ($off | Select-String "test result:.*0 failed").Count -gt 0
$onOk  = ($on  | Select-String "test result:.*0 failed").Count -gt 0
Write-Output "  feature OFF (the Store configuration): $(if ($offOk) { 'ok' } else { 'FAIL' })"
Write-Output "  feature ON  (the test configuration):  $(if ($onOk)  { 'ok' } else { 'FAIL' })"
if (-not $offOk -or -not $onOk) { $failed += "dev-entitlement" }

# The release binary is checked when one is present. A binary carrying the
# override while the feature is off is the exact thing this gate exists to
# prevent, and it is invisible from the source alone - a stale build looks
# identical.
$releaseExe = "target\release\spacerrecycle.exe"
if (Test-Path $releaseExe) {
    $carries = (Select-String -Path $releaseExe -Pattern "development override is enabled" -Encoding utf8 -List -ErrorAction SilentlyContinue) -ne $null
    if ($carries) {
        Write-Output "  WARN the release binary carries the override -"
        Write-Output "       it was built with -DevEntitlement. Do not submit it."
    } else {
        Write-Output "  ok   the release binary carries no override"
    }
} else {
    Write-Output "  --   no release binary to check"
}

Write-Output ""
if ($failed.Count -eq 0) {
    Write-Output "ALL GREEN"
} else {
    $list = [string]::Join(", ", $failed)
    Write-Output "FAILED: $list"
}
