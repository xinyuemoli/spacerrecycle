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
try {
    $out = cmd /c "build.cmd 2>&1"
    $buildCode = $LASTEXITCODE
} finally {
    Pop-Location
}
if ($buildCode -eq 0) {
    Write-Output "  ok   SpaceRecycleShell.dll built"
} else {
    # Say why. A check that reports FAIL without its output costs the next person
    # the same investigation this just cost.
    Write-Output "  FAIL shell-ext (exit $buildCode)"
    $out | Select-Object -Last 12 | ForEach-Object { Write-Output "       $_" }
    $failed += "shell-ext"
}

# The handler hands the shell a path from GetOverlayInfo. Nothing checks that the
# path exists, and the shell draws no overlay when it does not - which is exactly
# what happened when only the DLL was deployed and the .ico files were left
# behind. The files are part of the extension, so they are checked like it.
$iconsOk = $true
foreach ($name in @('important.ico', 'disposable.ico', 'clear.ico')) {
    $p = Join-Path 'shell-ext\bin\icons' $name
    if (-not (Test-Path $p)) {
        Write-Output "  FAIL icon missing: $name"
        $iconsOk = $false
    } elseif ((Get-Item $p).Length -lt 1000) {
        Write-Output "  FAIL icon too small: $name ($((Get-Item $p).Length) bytes)"
        $iconsOk = $false
    }
}
if ($iconsOk) { Write-Output "  ok   all three overlay icons are present" } else { $failed += "shell-ext-icons" }

# Every COM out parameter must be written even on the failure paths. Leaving one
# uninitialised is what crashed Explorer: the shell hashed an untouched overlay
# icon buffer and dereferenced null inside shell32.
Push-Location shell-ext
$outParams = cmd /c "bin\menutest.exe outparams 2>&1"
Pop-Location
if ($LASTEXITCODE -eq 0) {
    Write-Output "  ok   every COM out parameter is written"
} else {
    Write-Output "  FAIL out parameters"
    $outParams | Select-String "NOT written|FAIL" | ForEach-Object { Write-Output "       $($_.Line.Trim())" }
    $failed += "shell-ext-outparams"
}

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

# A byte order mark, which the heading of this section used to promise was
# checked but was not.
#
# A BOM is not whitespace to a JSON parser: Tauri's build script fails with
# "expected value at line 1 column 1" and says nothing about the BOM, and the
# build simply stops. PowerShell 5.1's `Set-Content -Encoding UTF8` writes one,
# so any script that rewrites a JSON file reintroduces the problem - which is
# exactly how tauri.conf.json broke during a release.
#
# JSON and TOML fail hard on it. Rust and JavaScript accept it, so those are
# reported without failing the run.
$strictBom = @('*.json', '*.toml')
$bomStrict = @()
$bomLoose = @()
$candidates = Get-ChildItem -Recurse -File -Include *.json,*.toml,*.rs,*.js,*.mjs,*.cpp,*.h,*.md,*.ps1 |
    Where-Object { $_.FullName -notmatch '\\target\\|\\node_modules\\|\\.git\\|gen\\schemas|\\bin\\|\\promo\\store\\edgeprofile|\\acl-recovery\\' }
foreach ($f in $candidates) {
    try {
        $fs = [IO.File]::OpenRead($f.FullName)
        $head = New-Object byte[] 3
        $n = $fs.Read($head, 0, 3)
        $fs.Close()
    } catch { continue }
    if ($n -lt 3) { continue }
    if ($head[0] -eq 0xEF -and $head[1] -eq 0xBB -and $head[2] -eq 0xBF) {
        $isStrict = $false
        foreach ($pat in $strictBom) { if ($f.Name -like $pat) { $isStrict = $true } }
        if ($isStrict) { $bomStrict += $f.FullName } else { $bomLoose += $f.FullName }
    }
}
if ($bomStrict.Count -gt 0) {
    Write-Output "  FAIL BOM in $($bomStrict.Count) JSON/TOML file(s) - these fail to parse:"
    $bomStrict | Select-Object -First 5 | ForEach-Object { Write-Output "         $_" }
    $failed += "encoding"
} else {
    Write-Output "  ok   no BOM in JSON or TOML"
}
if ($bomLoose.Count -gt 0) {
    Write-Output "  note BOM in $($bomLoose.Count) other file(s) (parsers accept it, but it is inconsistent)"
}

Write-Output ""
Write-Output "===== 5. leak check ====="
# The needles are built from code points rather than written literally.
#
# The first version hardcoded them, so this script matched itself and reported
# three leaking files that were three lines of the leak check. A check that
# finds itself is worse than no check: it fails on a clean tree, and the fix
# somebody reaches for is to stop running it.
$needles = @(
    (-join @([char]49, [char]49, [char]53, [char]55, [char]48)),
    (-join @([char]120, [char]105, [char]97, [char]111, [char]100, [char]117,
              [char]111, [char]116, [char]101, [char]99, [char]104)),
    (-join @([char]115, [char]104, [char]117, [char]112, [char]101, [char]105))
)
# -join over an explicit char array, not `[string][char]49 + [char]49 + ...`
# with commas between. The old form was parsed as a single expression: the array
# held one 24-character string, Join("|") left a trailing empty alternative in
# the regex, and the pattern matched every commit in the repository. A check that
# matches everything reports a stable number regardless of what the history
# contains, which looks exactly like a working check until the pattern is read.
if ($needles.Count -ne 3 -or ($needles | Where-Object { $_.Length -lt 5 }).Count -gt 0) {
    Write-Output "  FAIL the needle list did not build correctly"
    $failed += "leak-needles"
}
$needlePattern = [string]::Join("|", $needles)

$leak = 0
# Two needle sets, because the two places mean different things.
#
# In a commit message, a personal identifier is a leak and the name of a purged
# promotion channel is not: the commit that removes Xiaohongshu material has to
# say what it is removing. Flagging that made this check fail on the very commit
# that cleaned the repo up, and the natural response to a check that fails on
# good work is to stop running it.
#
# In file contents, any trace of the purged material counts - that is how a
# leftover script gets caught.
$messageNeedles = @(
    $needles[0],   # the old account number
    $needles[2]    # the personal handle fragment
)
$messagePattern = [string]::Join("|", $messageNeedles)

# Test-machine account names, which are not personal data.
#
# `C:\Users\sruser` appears in commit messages describing work done in the
# verification VM. The pattern below catches any profile path, so without this
# the check flags the test account as if it were somebody's real account - and
# then the only commits it reports are the ones that mention the test harness,
# which are the ones least worth looking at.
$testAccounts = @('sruser', 'me', 'test', 'user', 'runner', 'Administrator')

foreach ($c in (git rev-list HEAD 2>&1)) {
    $m = git log -1 --format='%B' $c 2>&1 | Out-String
    $real = [regex]::Matches($m, 'C:\\Users\\([A-Za-z0-9._-]+)') |
        Where-Object { $testAccounts -notcontains $_.Groups[1].Value }
    if ($m -match $messagePattern -or $real.Count -gt 0) { $leak++ }
}
Write-Output "  commits with personal data in the message: $leak"

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
Write-Output "===== 8. version control ====="
# The version used to exist only as a command-line argument to build-msix.ps1.
# Seven packages went out and the repository recorded none of them: no tags, and
# tauri.conf.json still said 0.1.0. These checks make that state impossible to
# return to silently.
$confVersion = $null
try {
    $confVersion = (Get-Content 'src-tauri\tauri.conf.json' -Raw | ConvertFrom-Json).version
} catch { }
if (-not $confVersion) {
    Write-Output "  FAIL cannot read the version from tauri.conf.json"
    $failed += "version"
} else {
    Write-Output "  version in tauri.conf.json: $confVersion"

    # Three parts, per semver. build-msix appends the fourth for the package.
    if ($confVersion -notmatch '^\d+\.\d+\.\d+$') {
        Write-Output "  FAIL version must be Major.Minor.Patch"
        $failed += "version"
    }

    # Cargo.toml has to agree: the binary reports its own version, and a package
    # whose contained binary disagrees misleads every crash report.
    $cargoLine = Select-String -Path 'src-tauri\Cargo.toml' -Pattern '^version\s*=\s*"([^"]+)"' | Select-Object -First 1
    if ($cargoLine) {
        $cargoVersion = $cargoLine.Matches[0].Groups[1].Value
        if ($cargoVersion -ne $confVersion) {
            Write-Output "  FAIL Cargo.toml says $cargoVersion, tauri.conf.json says $confVersion"
            $failed += "version"
        } else {
            Write-Output "  ok   Cargo.toml agrees ($cargoVersion)"
        }
    }

    # A tag for the current version is what makes the artifact traceable. Once the
    # version is tagged, the tag must still be an ancestor of HEAD rather than
    # floating somewhere else.
    $tagName = "v$confVersion"
    $tagCommit = git rev-list -n 1 $tagName 2>$null
    if (-not $tagCommit) {
        Write-Output "  WARN $tagName is not tagged yet - run scripts\release.ps1 before publishing"
    } else {
        $reachable = git merge-base --is-ancestor $tagName HEAD 2>$null
        if ($LASTEXITCODE -eq 0) {
            Write-Output "  ok   $tagName is an ancestor of HEAD"
        } else {
            Write-Output "  FAIL $tagName is not reachable from HEAD"
            $failed += "version"
        }

        # The version only moves forward. A repository version below the newest
        # tag means a bump was forgotten or reverted.
        $newest = git tag -l 'v*' --sort=-v:refname | Select-Object -First 1
        if ($newest -and $newest -ne $tagName) {
            $newestNum = $newest.TrimStart('v')
            $cmp = [version]($confVersion + '.0')
            $cmpNew = [version]($newestNum + $(if ($newestNum.Split('.').Count -eq 3) { '.0' } else { '' }))
            if ($cmp -lt $cmpNew) {
                Write-Output "  FAIL version $confVersion is behind the newest tag $newest"
                $failed += "version"
            } else {
                Write-Output "  ok   newest tag is $newest"
            }
        }
    }
}

Write-Output ""
if ($failed.Count -eq 0) {
    Write-Output "ALL GREEN"
    # The exit code is the contract, not the last line of output. This script
    # used to end at the Write-Output, so anything that ran it and looked at
    # $LASTEXITCODE saw whatever the last native command in here happened to
    # return - which made a clean run look like a failure, and would have made a
    # failing run look like a pass just as easily.
    exit 0
} else {
    $list = [string]::Join(", ", $failed)
    Write-Output "FAILED: $list"
    exit 1
}
