# Sign the test driver with a locally created certificate.
#
# Test signing mode does not accept an *unsigned* driver - it accepts a driver
# signed by a certificate Windows does not trust. That distinction is why the
# first load attempt failed with 0x80070241 (STATUS_INVALID_IMAGE_HASH): the
# image had no signature at all.
#
# This certificate is for the throwaway VM only. A driver that ships needs an EV
# code-signing certificate and a Microsoft attestation signature; neither can be
# produced locally, and neither is cheap.
[CmdletBinding()]
param(
    [string]$Sys = "$PSScriptRoot\bin\srdelete.sys"
)

$ErrorActionPreference = 'Stop'
$subject = 'CN=SpaceRecycle Test Driver'
$password = '<redacted-password>'

$signtool = 'C:\Program Files (x86)\Windows Kits\10\bin\10.0.26100.0\x64\signtool.exe'
if (-not (Test-Path $signtool)) { throw "signtool not found: $signtool" }
if (-not (Test-Path $Sys)) { throw "driver not built: $Sys" }

# Reuse the certificate across runs so the file's signature stays stable.
$cert = Get-ChildItem Cert:\CurrentUser\My |
        Where-Object { $_.Subject -eq $subject } |
        Select-Object -First 1
if (-not $cert) {
    Write-Host "creating test certificate"
    $cert = New-SelfSignedCertificate -Type CodeSigningCert -Subject $subject `
        -CertStoreLocation Cert:\CurrentUser\My -KeyUsage DigitalSignature `
        -NotAfter (Get-Date).AddYears(2)
} else {
    Write-Host "reusing test certificate $($cert.Thumbprint)"
}

$pfx = Join-Path $PSScriptRoot 'bin\sr-test.pfx'
$cer = Join-Path $PSScriptRoot 'bin\sr-test.cer'
$secure = ConvertTo-SecureString -String $password -Force -AsPlainText
Export-PfxCertificate -Cert $cert -FilePath $pfx -Password $secure | Out-Null
Export-Certificate -Cert $cert -FilePath $cer | Out-Null
Write-Host "exported $pfx"
Write-Host "exported $cer"

Write-Host ""
Write-Host "=== signing ==="
& $signtool sign /v /fd sha256 /f $pfx /p $password $Sys
if ($LASTEXITCODE -ne 0) { throw "signtool sign failed: $LASTEXITCODE" }

Write-Host ""
Write-Host "=== verifying (self-signed, so chain errors are expected) ==="
& $signtool verify /v /pa $Sys 2>&1 | Select-Object -First 12