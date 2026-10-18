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
$leak = 0
foreach ($c in (git rev-list HEAD 2>&1)) {
    $m = git log -1 --format='%B' $c 2>&1 | Out-String
    $real = [regex]::Matches($m, 'C:\\Users\\([^\s\\<]*)') | Where-Object { $_.Groups[1].Value -ne '' }
    if ($m -match '11570|xiaoduotech|shupei' -or $real.Count -gt 0) { $leak++ }
}
Write-Output "  commits with leaks in the message: $leak"
$contentLeaks = @(git grep -I "11570" -- . 2>$null)
Write-Output "  files containing 11570: $($contentLeaks.Count)"
if ($leak -gt 0 -or $contentLeaks.Count -gt 0) { $failed += "leak" }

Write-Output ""
Write-Output "===== 6. remaining old-level references ====="
$old = git grep -n -E "Importance::(Critical|High|Normal|Low)|imp\.(critical|high|normal|low)" -- src-tauri dist shell-ext 2>$null
if ($old) { Write-Output "  FAIL:"; $old | ForEach-Object { "         $_" }; $failed += "old-levels" }
else { Write-Output "  ok   none" }

Write-Output ""
if ($failed.Count -eq 0) {
    Write-Output "ALL GREEN"
} else {
    $list = [string]::Join(", ", $failed)
    Write-Output "FAILED: $list"
}
