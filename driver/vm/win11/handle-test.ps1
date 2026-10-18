<#
.SYNOPSIS
    Tests whether holding a file open can stop it being deleted, in user mode.

.DESCRIPTION
    The ACL experiment settled why a deny ACE does not protect a file in place:
    the file's own DELETE right is not the only thing that authorises a delete,
    and denying FILE_DELETE_CHILD on the parent would freeze the whole folder.
    That leaves the minifilter as the only way to intercept the operation - which
    needs a kernel driver, an EV certificate and a Hardware Dev Center account,
    none of which an individual developer can get.

    There is a third option, and this measures it. DeleteFile fails with
    ERROR_SHARING_VIOLATION when an open handle to the file was created without
    FILE_SHARE_DELETE. That is enforcement by the I/O manager, not by an ACL, so
    it does not care what the parent directory grants and it does not need a
    driver. A resident user-mode process holding a handle per protected file is
    the whole mechanism.

    It is not free, and the script measures the costs alongside the benefit:

      - the handle must be held by a process that stays alive, so the protection
        ends when the guard process does;
      - a file held this way cannot be renamed or moved either, because rename
        needs DELETE - so an "allow this once" decision has to close the handle
        first and reopen it after;
      - it is visible as a sharing violation, which is a more confusing message
        than "access denied" but is at least honest about what is happening.

    The holder runs in a separate process, because a handle held by the same
    process that deletes would not model the real arrangement - the guard is not
    the thing doing the deleting.
#>

$ErrorActionPreference = 'Continue'
$log = "C:\sr-test\handle-result-$env:USERNAME.txt"
$lines = @()

function Write-Log {
    try { $script:lines | Set-Content $script:log -Encoding UTF8 } catch { }
}

$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($id)
$lines += "user:          $($env:USERDOMAIN)\$($env:USERNAME)"
$lines += "administrator: $($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"

$base = Join-Path $env:USERPROFILE ("handletest-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
New-Item -ItemType Directory -Path $base -Force | Out-Null
$f = Join-Path $base 'protected.txt'
Set-Content -Path $f -Value 'important' -Encoding UTF8
$lines += "file:          $f"

function Try-Delete([string]$label) {
    & cmd.exe /c "del /f /q `"$f`"" 2>&1 | Out-Null
    $out = & cmd.exe /c "if exist `"$f`" (echo PRESENT) else (echo GONE)" 2>&1
    $state = ($out | Out-String).Trim()
    $script:lines += "  $label -> $state"
    return $state -eq 'PRESENT'
}

$lines += '--- 1. with no handle held ---'
$present = Try-Delete 'del'

# Recreate, since the first attempt removes it.
if (-not $present) { Set-Content -Path $f -Value 'important' -Encoding UTF8 }

$lines += '--- 2. while another process holds it open without FILE_SHARE_DELETE ---'

# A separate PowerShell process, so the handle is not held by the process doing
# the deleting. It prints the moment it has the handle, and then waits: the
# caller has to know the handle exists before it tests, or the test races the
# holder and passes for the wrong reason.
$holderScript = @"
`$fs = [IO.File]::Open('$f', [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::Read)
Write-Output 'HELD'
Start-Sleep -Seconds 60
`$fs.Close()
"@
$holderFile = Join-Path $base 'holder.ps1'
Set-Content -Path $holderFile -Value $holderScript -Encoding UTF8

$holder = Start-Process -FilePath 'powershell.exe' `
    -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $holderFile) `
    -PassThru -WindowStyle Hidden

# Wait for the handle to actually exist rather than sleeping and hoping. The
# holder writes nothing to disk, so the check is on the file being unopenable
# for delete - which is the state under test.
$deadline = (Get-Date).AddSeconds(30)
$held = $false
while ((Get-Date) -lt $deadline) {
    Start-Sleep -Milliseconds 500
    try {
        # If a second open without FILE_SHARE_DELETE fails, the holder is in.
        $probe = [IO.File]::Open($f, [IO.FileMode]::Open, [IO.FileAccess]::Read, [IO.FileShare]::None)
        $probe.Close()
    } catch {
        $held = $true
        break
    }
}
$lines += "  holder process running: $(-not $holder.HasExited)"
$lines += "  handle appears to be held: $held"

$stillPresent = Try-Delete 'del'

$lines += '--- 3. after the holder goes away ---'
try { Stop-Process -Id $holder.Id -Force -ErrorAction SilentlyContinue } catch { }
Start-Sleep -Seconds 3
$afterRelease = Try-Delete 'del'
$lines += "  file survived the held attempt: $stillPresent"
$lines += "  file deleted once released:     $(-not $afterRelease)"

$lines += '--- verdict ---'
if ($stillPresent -and -not $afterRelease) {
    $lines += '  HOLDING WORKS: the file could not be deleted while held, and could'
    $lines += '  be deleted the moment the handle closed. This is a user-mode delete'
    $lines += '  guard that needs no driver and no code-signing certificate.'
} elseif (-not $stillPresent) {
    $lines += '  HOLDING FAILED: the file was deleted anyway, so a held handle is not'
    $lines += '  a usable mechanism here.'
} else {
    $lines += '  INCONCLUSIVE: it survived while held but also after release.'
}

Write-Log
