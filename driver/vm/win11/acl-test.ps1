<#
.SYNOPSIS
    Decides whether a DENY DELETE ACE can stop a deletion, and if not, why not.

.DESCRIPTION
    Run as a STANDARD user, not an administrator. The first attempt at this ran
    as sruser, who is an Administrator, and whoami reports "High Mandatory
    Level" for that token - so the deny ACE was never consulted and the result
    said nothing about whether the technique works. A standard user is the only
    caller whose outcome is informative.

    Two explanations are being separated, and they need different fixes:

      (a) the deny ACE is not enforced at all, or
      (b) it is enforced, but Windows never consults DELETE on the file.

    (b) is worth testing first. Windows grants deletion rights from two places:
    DELETE on the object, or FILE_DELETE_CHILD on its parent directory. A user's
    own folder grants FILE_DELETE_CHILD by default, so denying DELETE on a file
    inside it changes nothing - the parent already authorises the delete.

    The probe that separates them is an explicit CreateFile with DELETE access.
    "The file was deleted" alone cannot tell them apart; "the file's own ACL
    denies DELETE and it was deleted anyway" points squarely at the parent.

    Three cases, each in its own directory so parent ACLs cannot interfere:

      control    no ACE                              expect: deleted
      file-only  DENY DELETE on the file             expect: deleted if (b) holds
      parent     DENY DELETE on the file AND
                 DENY FILE_DELETE_CHILD on the dir   expect: blocked if (b) holds

    Every case is wrapped: the first version of this script produced no case
    output at all and no error, because a failure inside a case aborted it
    silently and the trailing summary still printed as though the run were fine.
#>

$ErrorActionPreference = 'Continue'

# Logged per user. A single fixed filename meant that whichever account ran last
# overwrote the other's result, and the two accounts are the entire point of the
# experiment - the administrator's run proves the ACE is enforced, the standard
# user's run says whether that enforcement matters.
$log = "C:\sr-test\acl-result-$env:USERNAME.txt"
$logFallback = Join-Path $env:USERPROFILE "acl-result-$env:USERNAME.txt"
$lines = @()

trap {
    $lines += "UNCAUGHT: $($_.Exception.GetType().Name): $($_.Exception.Message)"
    Write-Log
    exit 9
}

function Write-Log {
    try { $script:lines | Set-Content $script:log -Encoding UTF8 }
    catch { $script:lines | Set-Content $script:logFallback -Encoding UTF8 }
}

$id = [Security.Principal.WindowsIdentity]::GetCurrent()
$principal = New-Object Security.Principal.WindowsPrincipal($id)
$lines += "user:            $($env:USERDOMAIN)\$($env:USERNAME)"
$lines += "sid:             $($id.User.Value)"
$lines += "profile:         $env:USERPROFILE"
$lines += "temp:            $env:TEMP"
$lines += "administrator:   $($principal.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"

$lines += '--- whoami /groups (filtered) ---'
try {
    (& whoami /groups 2>&1) | Select-String 'Administrators|Mandatory Label|Everyone|OWNER RIGHTS' |
        ForEach-Object { $lines += "  $($_.Line.Trim())" }
} catch { $lines += "  whoami failed: $($_.Exception.Message)" }

# --- the probe ---------------------------------------------------------------
$lines += '--- building the probe ---'
$probeOk = $false
try {
    Add-Type -Namespace SrAcl -Name Probe -MemberDefinition @'
[DllImport("kernel32.dll", SetLastError = true, CharSet = CharSet.Unicode)]
public static extern IntPtr CreateFileW(string name, uint access, uint share,
    IntPtr sa, uint disposition, uint flags, IntPtr template);
[DllImport("kernel32.dll", SetLastError = true)]
public static extern bool CloseHandle(IntPtr h);
public static int ProbeDelete(string path) {
    IntPtr h = CreateFileW(path, 0x00010000, 0, IntPtr.Zero, 3, 0x02000000, IntPtr.Zero);
    if (h == new IntPtr(-1)) { return Marshal.GetLastWin32Error(); }
    CloseHandle(h);
    return 0;
}
'@ -ErrorAction Stop
    $probeOk = $true
    $lines += '  ok'
} catch {
    $lines += "  FAILED: $($_.Exception.Message)"
    $lines += "  inner:  $($_.Exception.InnerException.Message)"
}

# A fresh directory per run, never removed. The previous version reused one path
# and cleared it at the start with Remove-Item -Recurse, which hangs once an
# earlier run has left a DENY FILE_DELETE_CHILD ACE on a subdirectory: the
# cleanup cannot delete the very thing the case was built to make undeletable,
# and it does not fail - it waits. Leaking a small directory is cheaper than a
# run that stops without saying why.
$base = Join-Path $env:USERPROFILE ("acltest-" + [guid]::NewGuid().ToString('N').Substring(0, 6))
$lines += "test root:       $base"
New-Item -ItemType Directory -Path $base -Force | Out-Null

# $script: is not decoration. Inside a function, `$lines += ...` is an
# assignment, and PowerShell resolves an assignment to a *local* variable even
# when a variable of that name exists in the parent scope - so every case would
# append to a throwaway copy and the report would contain the preamble, the
# trailing summary, and nothing in between. That is exactly what the first two
# runs of this script produced, with no error to explain it.
function Run-Case([string]$case, [string]$fileAce, [string]$dirAce) {
    $script:lines += "--- case: $case ---"
    try {
        $dir = Join-Path $base $case
        New-Item -ItemType Directory -Path $dir -Force | Out-Null
        $f = Join-Path $dir 'target.txt'
        Set-Content -Path $f -Value 'protected' -Encoding UTF8

        if ($dirAce)  { $script:lines += "  icacls dir:  $((& icacls $dir /deny $dirAce 2>&1 | Select-Object -First 1))" }
        if ($fileAce) { $script:lines += "  icacls file: $((& icacls $f /deny $fileAce 2>&1 | Select-Object -First 1))" }

        if ($probeOk) {
            $rc = [SrAcl.Probe]::ProbeDelete($f)
            $verdict = if ($rc -eq 0) { 'GRANTED' }
                       elseif ($rc -eq 5) { 'ACCESS_DENIED - the ACE is being enforced' }
                       else { "error $rc" }
            $script:lines += "  DELETE access on the file itself: $verdict"
        }

        $script:lines += '  file acl:'
        (& icacls $f 2>&1) | ForEach-Object { $script:lines += "    $($_.Trim())" }
        if ($dirAce) {
            $script:lines += '  parent acl:'
            (& icacls $dir 2>&1) | ForEach-Object { $script:lines += "    $($_.Trim())" }
        }

        & cmd.exe /c "del /f /q `"$f`"" 2>&1 | Out-Null
        $gone = -not (Test-Path $f)
        $script:lines += "  after cmd del:    $(if ($gone) { 'DELETED' } else { 'STILL THERE' })"

        if (-not $gone) {
            try { [IO.File]::Delete($f); $script:lines += '  after File.Delete: DELETED' }
            catch { $script:lines += "  after File.Delete: BLOCKED - $($_.Exception.Message.Split([char]10)[0])" }
        } else {
            $script:lines += '  after File.Delete: (not attempted)'
        }
    } catch {
        $script:lines += "  CASE FAILED: $($_.Exception.GetType().Name): $($_.Exception.Message)"
    }
}

Run-Case 'control'   ''              ''
Run-Case 'file-only' '*S-1-1-0:(D)'  ''
Run-Case 'parent'    '*S-1-1-0:(D)'  '*S-1-1-0:(DC)'

$lines += '--- how to read it ---'
$lines += '  file-only ACCESS_DENIED but DELETED anyway  -> the parent directory'
$lines += '     authorised it via FILE_DELETE_CHILD; the ACE was never deciding.'
$lines += '  file-only GRANTED                            -> the deny was not applied;'
$lines += '     the problem is the ACE, not the delete path.'
$lines += '  control GRANTED                              -> expected baseline.'

Write-Log
