# Can this guest session write to HKLM, and is UAC in the way?
#
# This decides how the icon-overlay handler gets installed. Overlay handlers are
# read from HKLM only - there is no per-user equivalent - so unlike the context
# menu, a per-user install is not an option. VMware's guest operations run the
# program with the caller's filtered token, so an administrator account with UAC
# on cannot write HKLM without going through an elevated task.
$ErrorActionPreference = 'Continue'

$o = New-Object System.Collections.Generic.List[string]
function L($t) { $o.Add([string]$t) | Out-Null }

$pol = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Policies\System'
$p = Get-ItemProperty $pol -ErrorAction SilentlyContinue
L "EnableLUA                  = $($p.EnableLUA)"
L "ConsentPromptBehaviorAdmin = $($p.ConsentPromptBehaviorAdmin)"
L "FilterAdministratorToken   = $($p.FilterAdministratorToken)"
L ""

try {
    New-Item -Path 'HKLM:\SOFTWARE\__srtest' -Force -ErrorAction Stop | Out-Null
    L "HKLM create key            = OK"
    Remove-Item 'HKLM:\SOFTWARE\__srtest' -Force -Recurse -ErrorAction SilentlyContinue
} catch {
    L "HKLM create key            = DENIED"
    L "    $($_.Exception.Message)"
}

# Who is this, really? The group list shows whether the token is filtered.
$id = [Security.Principal.WindowsIdentity]::GetCurrent()
L ""
L "user                       = $($id.Name)"
L "token type                 = $($id.ImpersonationLevel)"
$isAdmin = (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
L "in Administrators role     = $isAdmin"

$o | Set-Content -Path 'C:\sr-test\access.txt' -Encoding UTF8
$o | ForEach-Object { Write-Host $_ }