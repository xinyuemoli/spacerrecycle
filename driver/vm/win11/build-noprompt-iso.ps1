# Rebuilds the Windows 11 install ISO without the CD boot prompt.
#
# The stock media waits a few seconds at "Press any key to boot from CD or DVD"
# and boots the next device if nothing arrives. An unattended install has nobody
# to press it, so the firmware falls through, finds no other device, and reports
# that no operating system was found - which reads like a corrupt download
# rather than a missing keystroke. The vmware.log makes it look like a boot
# failure:
#
#     Guest: About to do EFI boot: EFI VMware Virtual SATA CDROM Drive (1.0)
#     Guest: Status upon boot failure: Time out
#     [msg.Backdoor.OsNotFound] No operating system was found.
#
# Microsoft ships the fix inside the media itself: cdboot_noprompt.efi and
# efisys_noprompt.bin are the same boot images with the wait removed. The stock
# El Torito entry points at efisys.bin, so pointing it at the noprompt variant
# and rebuilding is all that is needed.
#
# oscdimg is used rather than a third-party ISO tool because it is the tool
# Microsoft documents for exactly this, and it preserves the UDF layout the
# media needs: install.wim is larger than 4 GB, so plain ISO9660 cannot hold it.
#
# The unattended answer file is copied into the media root, so one disc carries
# both Windows and the answers and no second CD is required - setup searches
# removable media for autounattend.xml, and the install media is one.

param(
  [string]$Iso = "<iso-dir>\Win11_24H2_Enterprise_Eval_x64.iso",
  [string]$Stage = "<iso-dir>\win11-media",
  [string]$Output = "<iso-dir>\Win11_noprompt.iso",
  [string]$Unattend = "<repo>\driver\vm\unattend"
)

# Not Stop: oscdimg reports its progress on stderr, and treating that as fatal
# aborts the build while the image is being written.
$ErrorActionPreference = "Continue"
$oscdimg = "C:\Program Files (x86)\Windows Kits\10\Assessment and Deployment Kit\Deployment Tools\amd64\Oscdimg\oscdimg.exe"
if (-not (Test-Path $oscdimg)) { throw "oscdimg not found at $oscdimg" }

# --- 1. stage the media ---
if (-not (Test-Path (Join-Path $Stage "sources\install.wim"))) {
  Write-Output "staging the media to $Stage (about 5 GB)"
  $img = Mount-DiskImage -ImagePath $Iso -PassThru
  $drive = "$(($img | Get-Volume).DriveLetter):\"
  New-Item -ItemType Directory -Path $Stage -Force | Out-Null
  # /MT:16 for throughput, /NFL /NDL /NJH /NJS to keep the log short, /NP for a
  # summary line instead of a percentage that would fill the transcript.
  $rc = robocopy $drive $Stage /E /MT:16 /NFL /NDL /NJH /NJS /NP /R:2 /W:2
  Dismount-DiskImage -ImagePath $Iso | Out-Null
  # robocopy returns 0-7 for success; 8 and above are real failures.
  if ($LASTEXITCODE -ge 8) { throw "robocopy failed with $LASTEXITCODE" }
  Write-Output "  staged"
} else {
  Write-Output "media already staged at $Stage"
}

# --- 2. put the answer file in the media root ---
# srarm.cmd comes along because setup copies it into the installed system and
# registers it to run on every boot; without it in the media root that copy has
# nothing to copy from, and the guest stops logging itself in after the first
# reboot.
#
# The answer file carries a placeholder where the local account password goes,
# because this repository is public and a password in it is a password everybody
# has. The real value comes from the environment when the media is built and is
# never written into the tracked file.
$vmPassword = $env:SR_VM_PASSWORD
if ([string]::IsNullOrEmpty($vmPassword)) {
  throw "SR_VM_PASSWORD is not set. It is the password the unattended install gives the local account."
}

foreach ($f in @("autounattend.xml", "srsetup.cmd", "srarm.cmd")) {
  $src = Join-Path $Unattend $f
  $dst = Join-Path $Stage $f
  $body = [IO.File]::ReadAllText($src, (New-Object Text.UTF8Encoding($false)))
  if ($body.Contains('__VM_PASSWORD__')) {
    # Plain string replace, not a regex: a password containing $ or a backtick
    # would be a pattern to Regex.Replace and literal text to the person who
    # chose it.
    $body = $body.Replace('__VM_PASSWORD__', $vmPassword)
    [IO.File]::WriteAllText($dst, $body, (New-Object Text.UTF8Encoding($false)))
    Write-Output "  placed $f in the media root (password substituted)"
  } else {
    Copy-Item $src $dst -Force
    Write-Output "  placed $f in the media root"
  }
}

# Refuse to build media that still carries the placeholder: the install would
# then either fail or set a literal __VM_PASSWORD__ as the account password.
foreach ($f in @("autounattend.xml", "srsetup.cmd", "srarm.cmd")) {
  $dst = Join-Path $Stage $f
  if (-not (Test-Path $dst)) { continue }
  if ([IO.File]::ReadAllText($dst).Contains('__VM_PASSWORD__')) {
    throw "$f still contains __VM_PASSWORD__ after staging"
  }
}

# --- 3. rebuild ---
# Two El Torito entries: BIOS (etfsboot.com) and UEFI (efisys_noprompt.bin).
$biosBoot = Join-Path $Stage "boot\etfsboot.com"
$uefiBoot = Join-Path $Stage "efi\microsoft\boot\efisys_noprompt.bin"
foreach ($b in @($biosBoot, $uefiBoot)) {
  if (-not (Test-Path $b)) { throw "boot image missing: $b" }
}

if (Test-Path $Output) { Remove-Item $Output -Force }
$bootData = "-bootdata:2#p0,e,b$biosBoot#pEF,e,b$uefiBoot"
Write-Output "building $Output"
Write-Output "  $bootData"

& $oscdimg -m -o -u2 -udfver102 $bootData $Stage $Output
if ($LASTEXITCODE -ne 0) { throw "oscdimg failed with $LASTEXITCODE" }

Get-Item $Output | Select-Object Name, @{n='GB';e={[math]::Round($_.Length/1GB,2)}}
Write-Output "done"
