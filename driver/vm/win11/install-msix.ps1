# Installs the test MSIX in the guest and reports what the Store says about it.
#
# The point of this run is not that the app installs. It is what the app makes
# of its own entitlement once it has real package identity but no Store licence:
# StoreContext only answers for a package with identity, and a sideloaded
# package has identity without entitlements. That combination - packaged, not
# subscribed - is the case the gate exists for, and it cannot be reproduced by
# running the exe or by any test that injects a fake Store answer.
#
# Add-AppxPackage runs as the guest user rather than as SYSTEM: it installs for
# the calling user, so running it elevated would install the package for the
# machine account and the app would not appear for sruser at all.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\install-result.txt'
$lines = @()

$msix = 'C:\sr-test\sr.msix'
$lines += "msix exists: $(Test-Path $msix)"

# Remove a previous copy so this is a real install rather than a re-registration
# of whatever was there before.
$existing = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
if ($existing) {
    $lines += "removing existing: $($existing.PackageFullName)"
    Remove-AppxPackage -Package $existing.PackageFullName -ErrorAction SilentlyContinue
}

try {
    Add-AppxPackage -Path $msix -ErrorAction Stop
    $lines += 'Add-AppxPackage: OK'
} catch {
    $lines += "Add-AppxPackage: FAILED - $($_.Exception.Message)"
}

$pkg = Get-AppxPackage -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
if ($pkg) {
    $lines += "installed name:      $($pkg.Name)"
    $lines += "installed full:      $($pkg.PackageFullName)"
    $lines += "installed version:   $($pkg.Version)"
    $lines += "signature kind:      $($pkg.SignatureKind)"
    $lines += "install location:    $($pkg.InstallLocation)"
    $lines += "package family:      $($pkg.PackageFamilyName)"
} else {
    $lines += 'NOT INSTALLED'
}

$lines | Set-Content $log -Encoding UTF8
