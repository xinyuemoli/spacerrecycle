# Installs the MSIX signing certificate into the guest's machine stores.
#
# Run inside the guest, not on the host. Written as a file because every attempt
# to pass this inline through vmrun lost the quoting somewhere between
# PowerShell, vmrun and the guest's cmd - and the failures were silent, which is
# how a certificate check turns into "the package will not install and nobody
# knows why".
#
# TrustedPeople is where a sideloaded MSIX looks for its signer. Root is also
# written because this certificate is self-signed: without it in the chain, the
# package is refused with a trust error rather than a signature error, which
# points at the wrong thing.

$ErrorActionPreference = 'Continue'
$log = 'C:\sr-test\cert-result.txt'
$lines = @()
$lines += "user: $env:USERNAME"
$lines += "elevated: $((New-Object Security.Principal.WindowsPrincipal([Security.Principal.WindowsIdentity]::GetCurrent())).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator))"

$cer = 'C:\sr-test\sr-signing.cer'
$lines += "cert file exists: $(Test-Path $cer)"
if (Test-Path $cer) {
    try {
        $c = New-Object Security.Cryptography.X509Certificates.X509Certificate2($cer)
        $lines += "subject:  $($c.Subject)"
        $lines += "thumb:    $($c.Thumbprint)"
    } catch {
        $lines += "load failed: $($_.Exception.Message)"
    }
}

foreach ($store in @('TrustedPeople', 'Root')) {
    $loc = "Cert:\LocalMachine\$store"
    try {
        Import-Certificate -FilePath $cer -CertStoreLocation $loc -ErrorAction Stop | Out-Null
        $lines += "import $store : OK"
    } catch {
        $lines += "import $store : FAILED - $($_.Exception.Message)"
    }
    # Read back rather than trusting the import's return value.
    try {
        $found = Get-ChildItem $loc | Where-Object { $_.Subject -like '*226CE8C9*' }
        $lines += "verify $store : $(if ($found) { "found $($found.Count)" } else { 'NOT FOUND' })"
    } catch {
        $lines += "verify $store : error - $($_.Exception.Message)"
    }
}

$lines | Set-Content $log -Encoding UTF8
