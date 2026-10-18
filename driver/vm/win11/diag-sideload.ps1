# Why the sideload was refused.
#
# 0x80070005 on Add-AppxPackage has at least three unrelated causes - the signer
# is not trusted for the installing user, sideloading policy is off, or the
# package needs more privilege than the install - and they are fixed in
# different places. The deployment log names the one that applies, so it is read
# rather than inferred from the HRESULT.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\diag-result.txt'
$lines = @()

$lines += "user: $env:USERNAME"

# --- the signer, as this user sees it ---
foreach ($store in @('TrustedPeople', 'Root')) {
    foreach ($scope in @('LocalMachine', 'CurrentUser')) {
        $path = "Cert:\$scope\$store"
        try {
            $found = Get-ChildItem $path -ErrorAction Stop |
                Where-Object { $_.Subject -like '*226CE8C9*' }
            $lines += "cert $scope\$store : $(if ($found) { "found $($found.Count)" } else { 'NOT FOUND' })"
        } catch {
            $lines += "cert $scope\$store : error"
        }
    }
}

# --- sideloading policy ---
$unlock = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock'
try {
    $p = Get-ItemProperty $unlock -ErrorAction Stop
    $lines += "AllowAllTrustedApps:          $($p.AllowAllTrustedApps)"
    $lines += "AllowDevelopmentWithoutDevLicense: $($p.AllowDevelopmentWithoutDevLicense)"
} catch {
    $lines += "AppModelUnlock key: not present"
}

# --- what deployment actually said ---
$lines += '--- Get-AppPackageLog ---'
try {
    $entries = Get-AppPackageLog -ActivityID 'bc3ff8c5-5e5f-0003-8444-45bc5f5edd01' -ErrorAction Stop
    $entries | Select-Object -Last 25 | ForEach-Object {
        $lines += "  [$($_.Level)] $($_.Message)"
    }
} catch {
    $lines += "  log read failed: $($_.Exception.Message)"
}

# --- the package's own signature, verified independently ---
$lines += '--- signature ---'
try {
    $sig = Get-AuthenticodeSignature 'C:\sr-test\sr.msix'
    $lines += "  status: $($sig.Status)"
    $lines += "  signer: $($sig.SignerCertificate.Subject)"
    $lines += "  message: $($sig.StatusMessage)"
} catch {
    $lines += "  signature check failed: $($_.Exception.Message)"
}

# --- is the app execution alias / package already registered for someone else? ---
$lines += '--- existing registrations ---'
$all = Get-AppxPackage -AllUsers -Name 'xinyuemoli.SpaceRecycle' -ErrorAction SilentlyContinue
$lines += "  count: $(@($all).Count)"
@($all) | ForEach-Object { $lines += "  $_" }

$lines | Set-Content $log -Encoding UTF8
