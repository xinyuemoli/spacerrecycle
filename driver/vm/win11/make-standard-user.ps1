# Creates a standard (non-administrator) local user for the ACL experiment.
#
# sruser is an Administrator, and whoami reports "High Mandatory Level" for that
# token - which means a deny ACE is not the thing deciding the outcome. Testing
# the technique as sruser answers a question nobody asked. A standard user is
# the only caller whose result is informative.
#
# Must run elevated, so it is started as SYSTEM by a scheduled task: vmrun does
# not provide an elevated token of its own.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\stduser-result.txt'
$lines = @()

$name = 'srstd'
$pass = 'SrStd2026!'

$existing = Get-LocalUser -Name $name -ErrorAction SilentlyContinue
if ($existing) {
    $lines += "user already exists: $name"
} else {
    try {
        $secure = ConvertTo-SecureString $pass -AsPlainText -Force
        New-LocalUser -Name $name -Password $secure -PasswordNeverExpires `
            -Description 'Standard user for the ACL deny-delete experiment' -ErrorAction Stop | Out-Null
        $lines += "created: $name"
    } catch {
        $lines += "create failed: $($_.Exception.Message)"
    }
}

# Explicitly not a member of Administrators. New-LocalUser does not add it, but
# the check is stated rather than assumed because the whole experiment rests on
# this token being unprivileged.
try {
    $inAdmins = Get-LocalGroupMember -Group 'Administrators' -ErrorAction Stop |
        Where-Object { $_.Name -like "*$name" }
    if ($inAdmins) {
        Remove-LocalGroupMember -Group 'Administrators' -Member $name -ErrorAction SilentlyContinue
        $lines += 'removed from Administrators'
    } else {
        $lines += 'not in Administrators (as required)'
    }
} catch {
    $lines += "group check failed: $($_.Exception.Message)"
}

$u = Get-LocalUser -Name $name -ErrorAction SilentlyContinue
if ($u) {
    $lines += "enabled: $($u.Enabled)"
    $lines += "sid:     $($u.SID.Value)"
    $lines += "groups:"
    try {
        Get-LocalGroup -ErrorAction SilentlyContinue | ForEach-Object {
            $g = $_
            $m = Get-LocalGroupMember -Group $g.Name -ErrorAction SilentlyContinue |
                Where-Object { $_.Name -like "*$name" }
            if ($m) { $lines += "  $($g.Name)" }
        }
    } catch { }
}

$lines | Set-Content $log -Encoding UTF8
