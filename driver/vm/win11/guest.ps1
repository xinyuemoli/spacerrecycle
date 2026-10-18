<#
.SYNOPSIS
    Runs a command in the test guest and returns its output reliably.

.DESCRIPTION
    vmrun's runProgramInGuest does not hand back stdout in any usable form, and
    with -interactive it can block until the guest's own timeout instead of
    returning. The complaint that produced this file was a query that simply
    never came back, which looks identical to a guest that is switched off.

    The mechanism that does work: the guest writes to a file, and vmrun copies
    the file out. So the caller passes a command line, this wraps it so that both
    streams land in one file, and then reads that file back. -interactive is
    deliberately not passed: the commands this is used for are queries, and
    session 0 is enough for those.

    The output file is named per call rather than reused. A shared filename meant
    that a command which failed to start left the previous call's contents in
    place, and the caller read a stale answer as if it were the new one.

.PARAMETER Command
    The command line, as the guest's cmd.exe should receive it.

.PARAMETER Elevated
    Run it elevated. session 0 is already high mandatory level in this guest, so
    this is only needed for genuinely privileged operations such as installing a
    certificate into the machine store.

.EXAMPLE
    .\guest.ps1 -Command "Get-AppxPackage *SpaceRecycle* | Format-List Name,PackageFullName"
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)][string]$Command,
    [int]$TimeoutSeconds = 120,
    [switch]$Elevated
)

$ErrorActionPreference = 'Continue'

$vmrun = 'C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe'
$vmx   = '<vm-dir>\Windows 11 x64.vmx'
$user  = 'sruser'
$pass  = '<redacted-password>'

if (-not (Test-Path $vmrun)) { throw "vmrun not found at $vmrun" }
if (-not (Test-Path $vmx))   { throw "vmx not found at $vmx" }

$stamp    = [guid]::NewGuid().ToString('N').Substring(0, 8)
$guestOut = "C:\sr-test\guest-$stamp.txt"
$hostOut  = Join-Path $env:TEMP "sr-guest-$stamp.txt"

# cmd needs the whole thing as one /c argument, and nested quotes survive better
# when the inner command is written without them around the redirect.
$ps = "& { $Command } *> '$guestOut'"

$args = @('-T', 'ws', '-gu', $user, '-gp', $pass, 'runProgramInGuest', $vmx)
if ($Elevated) { $args += '-interactive' }
$args += @('C:\Windows\System32\WindowsPowerShell\v1.0\powershell.exe',
           '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-Command', $ps)

$job = Start-Job -ScriptBlock {
    param($exe, $a)
    & $exe @a 2>&1
} -ArgumentList $vmrun, $args

if (-not (Wait-Job $job -Timeout $TimeoutSeconds)) {
    Stop-Job $job -ErrorAction SilentlyContinue
    Remove-Job $job -Force -ErrorAction SilentlyContinue
    Write-Output "(guest command timed out after ${TimeoutSeconds}s)"
} else {
    $out = Receive-Job $job
    Remove-Job $job -Force -ErrorAction SilentlyContinue
    if ($out) { $out | ForEach-Object { "  vmrun: $_" } }
}

# Copy the result out. CopyFileFromGuestToHost needs the guest path to be
# readable by the guest user, which C:\sr-test is.
& $vmrun -T ws -gu $user -gp $pass CopyFileFromGuestToHost $vmx $guestOut $hostOut 2>&1 | Out-Null
if (Test-Path $hostOut) {
    Get-Content $hostOut -ErrorAction SilentlyContinue | ForEach-Object { $_ }
    Remove-Item $hostOut -Force -ErrorAction SilentlyContinue
} else {
    Write-Output "(no output file was produced)"
}

& $vmrun -T ws -gu $user -gp $pass deleteFileInGuest $vmx $guestOut 2>&1 | Out-Null
