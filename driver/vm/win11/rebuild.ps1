# Wipes the Windows 11 test machine and installs it again from the checked-in
# answer file.
#
# Rebuilding rather than repairing is deliberate here. The previous machine was
# left with a blank account password, and vmrun refuses blank passwords and
# anonymous guest operations, so there was no way back in - no guest file copy,
# no guest program, and therefore no way to restore a password. Rebuilding is
# unattended and takes about forty minutes; working out an offline SAM edit
# would have taken longer and left the machine in a state nobody could rebuild
# from.
#
# It also picks up the fix for why the first machine kept stopping at the
# sign-in screen: Windows deletes the automatic logon values after the first
# successful logon, so they are now re-written on every boot.

param(
  [string]$Dir = "",
  [string]$Vmx = $env:SR_VMX,
  [string]$Vdisk = "C:\Program Files (x86)\VMware\VMware Workstation\vmware-vdiskmanager.exe",
  [string]$Vmrun = "C:\Program Files (x86)\VMware\VMware Workstation\vmrun.exe",
  [switch]$SkipIso
)

if ([string]::IsNullOrEmpty($Vmx)) {
    throw 'SR_VMX is not set - path to the test VM''s .vmx file'
}
if ([string]::IsNullOrEmpty($Dir)) { $Dir = Split-Path -Parent $Vmx }

# Not Stop: vmware-vdiskmanager prints an SSL config warning on stderr that is
# entirely harmless, and treating stderr as fatal aborts the rebuild halfway
# through with the disk already created.
$ErrorActionPreference = "Continue"

# --- 1. stop and wipe ---
Write-Output "=== stopping the machine ==="
& $Vmrun -T ws stop $Vmx hard 2>&1 | Out-Null
Start-Sleep -Seconds 10

Write-Output "=== removing the disk and the firmware state ==="
Get-ChildItem "$Dir\Windows 11 x64.vmdk*" -ErrorAction SilentlyContinue | ForEach-Object {
  Write-Output "  removing $($_.Name)"
  Remove-Item $_.FullName -Force
}
Get-ChildItem "$Dir\*.nvram" -ErrorAction SilentlyContinue | ForEach-Object {
  Write-Output "  removing $($_.Name)"
  Remove-Item $_.FullName -Force
}

# --- 2. fresh disk ---
Write-Output "=== creating a new 70 GB disk ==="
& $Vdisk -c -s 70GB -a lsilogic -t 0 "$Dir\Windows 11 x64.vmdk" 2>&1 | Select-Object -Last 2

# --- 3. refresh the install media ---
if (-not $SkipIso) {
  Write-Output "=== rebuilding the no-prompt ISO with the current answer files ==="
  & "$PSScriptRoot\build-noprompt-iso.ps1" 2>&1 |
    Where-Object { $_ -match "placed|building|done|\.iso" }
}

# --- 4. boot it ---
Write-Output "=== starting the install ==="
& $Vmrun -T ws start $Vmx nogui
Write-Output "start exit=$LASTEXITCODE"
