# Cut a release.
#
# The problem this exists to solve: versions used to be a command-line argument
# to build-msix.ps1 and nothing else. Seven packages went out (1.0.0.0 through
# 1.0.6.0) and the repository recorded none of them - tauri.conf.json still said
# 0.1.0, there were no tags, and no artifact could be traced back to a commit.
#
# So a release now means: the version lives in the source, the checks pass, the
# tree is clean, the build is made from that commit, and the commit is tagged.
# Every step that can be verified is verified rather than assumed.
#
# Usage:
#   scripts\release.ps1 -SetVersion 1.0.7 -DryRun      # what would happen
#   scripts\release.ps1 -SetVersion 1.0.7              # bump, test, build, tag
#   scripts\release.ps1                                # release the current version
#
# Nothing is pushed unless -Push is given: tagging locally first and looking at
# the result is the cheap way to notice a mistake.

[CmdletBinding()]
param(
    # New version to record in the source, e.g. 1.0.7. Omit to release whatever
    # the repository already says.
    [string]$SetVersion,

    # Skip the test suite. For a throwaway build only.
    [switch]$SkipChecks,

    # Build the package but do not tag.
    [switch]$NoTag,

    # Push the commit and the tag.
    [switch]$Push,

    [switch]$DryRun
)

$ErrorActionPreference = "Stop"

$root     = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $root "src-tauri"
$confPath = Join-Path $tauriDir "tauri.conf.json"
$cargoPath = Join-Path $tauriDir "Cargo.toml"

function Step([string]$text) {
    Write-Host ""
    Write-Host "==> $text" -ForegroundColor Cyan
}

function Fail([string]$text) {
    Write-Host "FAIL: $text" -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------- preconditions

Step "preconditions"

Push-Location $root
try {
    # A release must be built from a commit that contains everything in the
    # build, so modifications to tracked files are fatal: the artifact could not
    # be reproduced from the tag.
    #
    # Untracked files are only a warning. Build output, screenshots pulled out of
    # a VM, and other artifacts live in the tree without being part of any
    # commit, and refusing to release until they are deleted or stashed would
    # make the release path something people work around.
    $modified = git status --porcelain --untracked-files=no
    if ($modified -and -not $DryRun) {
        Write-Host "Tracked files have uncommitted changes:" -ForegroundColor Red
        $modified | ForEach-Object { Write-Host "  $_" }
        Fail "commit or stash them first - a tag that does not match the build is worse than no tag"
    }

    $untracked = git status --porcelain --untracked-files=normal |
        Where-Object { $_ -match '^\?\?' }
    if ($untracked) {
        Write-Host "  note: $($untracked.Count) untracked path(s), not part of this release:" -ForegroundColor Yellow
        $untracked | Select-Object -First 5 | ForEach-Object { Write-Host "    $_" }
        if ($untracked.Count -gt 5) { Write-Host "    ... and $($untracked.Count - 5) more" }
    }
    Write-Host "  no tracked changes"

    $branch = (git rev-parse --abbrev-ref HEAD).Trim()
    Write-Host "  branch: $branch"

    $head = (git rev-parse --short HEAD).Trim()
    Write-Host "  head:   $head"

    # ------------------------------------------------------------- version bump

    $conf = Get-Content $confPath -Raw | ConvertFrom-Json
    $current = $conf.version
    Write-Host "  current version: $current"

    if ($SetVersion) {
        $clean = $SetVersion.Trim()
        # Three parts, per semver. The Windows package version is this with a
        # trailing .0 added by build-msix.ps1.
        if ($clean -notmatch '^\d+\.\d+\.\d+$') {
            Fail "version must be Major.Minor.Patch, e.g. 1.0.7 (got '$clean')"
        }
        if ([int]($clean.Split('.')[0]) -lt 1) {
            Fail "the major part cannot be 0: the Store rejects such packages"
        }
        if ($clean -eq $current) {
            Write-Host "  version already $clean, nothing to bump"
        } else {
            Step "bumping version $current -> $clean"
            if (-not $DryRun) {
                $conf.version = $clean
                ($conf | ConvertTo-Json -Depth 100) | Set-Content $confPath -Encoding UTF8

                # Cargo.toml has to agree: the binary reports its own version, and
                # a package whose contained binary disagrees is confusing to
                # support and to read crash reports from.
                $cargo = [IO.File]::ReadAllText($cargoPath)
                $cargo = [regex]::Replace($cargo, '(?m)^version\s*=\s*"[^"]+"', "version = `"$clean`"", 1)
                [IO.File]::WriteAllText($cargoPath, $cargo, (New-Object Text.UTF8Encoding($false)))

                git add $confPath $cargoPath
                git commit --quiet -m "chore(release): $clean"
                Write-Host "  committed" -ForegroundColor Green
            } else {
                Write-Host "  (dry run: would write $clean to tauri.conf.json and Cargo.toml)"
            }
        }
        $current = $clean
    }

    $tagName = "v$current"
    if (git tag -l $tagName) {
        Write-Host "  tag $tagName already exists" -ForegroundColor Yellow
    }

    # ------------------------------------------------------------------- checks

    if (-not $SkipChecks) {
        Step "checks"
        if ($DryRun) {
            Write-Host "  (dry run: would run scripts\check-all.ps1)"
        } else {
            & (Join-Path $PSScriptRoot "check-all.ps1")
            if ($LASTEXITCODE -ne 0) { Fail "checks did not pass - not releasing" }
            Write-Host "  all green" -ForegroundColor Green
        }
    }

    # -------------------------------------------------------------------- build

    Step "build"
    $msixVersion = "$current.0"
    if ($DryRun) {
        Write-Host "  (dry run: would run build-msix.ps1 -Version $msixVersion)"
    } else {
        # Version is taken from the repository now, so it does not need passing;
        # this asserts that the repository and the package agree.
        & (Join-Path $PSScriptRoot "build-msix.ps1")
        if ($LASTEXITCODE -ne 0) { Fail "build failed" }
        $artifact = Join-Path $tauriDir "target\msix\SpaceRecycle_${msixVersion}_x64.msix"
        if (-not (Test-Path $artifact)) { Fail "expected artifact missing: $artifact" }
        Write-Host "  $artifact" -ForegroundColor Green
    }

    # ---------------------------------------------------------------------- tag

    if (-not $NoTag) {
        Step "tag"
        if ($DryRun) {
            Write-Host "  (dry run: would tag $tagName)"
        } elseif (git tag -l $tagName) {
            Write-Host "  tag $tagName already points at $((git rev-list -n 1 $tagName).Trim().Substring(0,7))"
        } else {
            # Annotated: the message is the only place that says what this version
            # was, and it costs nothing.
            git tag -a $tagName -m "SpaceRecycle $current"
            Write-Host "  tagged $tagName at $((git rev-parse --short HEAD).Trim())" -ForegroundColor Green
        }
    }

    if ($Push) {
        Step "push"
        if ($DryRun) {
            Write-Host "  (dry run: would push $branch and $tagName)"
        } else {
            git push origin $branch
            git push origin $tagName
            Write-Host "  pushed" -ForegroundColor Green
        }
    } else {
        Write-Host ""
        Write-Host "Not pushed. Review, then:" -ForegroundColor Yellow
        Write-Host "  git push origin $branch"
        Write-Host "  git push origin $tagName"
    }

    Write-Host ""
    Write-Host "DONE: $current" -ForegroundColor Green
} finally {
    Pop-Location
}
