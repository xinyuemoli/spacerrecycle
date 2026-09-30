<#
.SYNOPSIS
    Builds the Microsoft Store (MSIX) package for SpaceRecycle.

.DESCRIPTION
    Produces src-tauri\target\msix\SpaceRecycle_<version>_x64.msix, signed with a
    locally generated development certificate.

    Why MSIX: submitting an MSIX to the Microsoft Store gets the package
    code-signed by Microsoft at no cost. The unpackaged EXE/MSI Store route
    instead requires buying a CA code-signing certificate.

.PREREQUISITES
    winget install microsoft.winappcli --source winget

    For local install/testing the certificate must be trusted, which needs an
    elevated shell once:  winapp cert install <path>\SpaceRecycle_cert.pfx
    Testing without installing is possible on a machine with Developer Mode on:
        winapp run src-tauri\target\msix\stage --detach

.PARAMETER PackageName
    Manifest Identity Name. Must match Package/Identity/Name from the Partner
    Center "Product identity" panel before the Store will accept the upload.

.PARAMETER Publisher
    Publisher distinguished name written into the manifest Identity. It MUST
    match the Publisher DN assigned to your Partner Center account before the
    Store will accept the package. The default is a local-testing placeholder.

.PARAMETER PublisherDisplay
    Human-readable publisher name shown to users. Must match
    Package/Identity/PublisherDisplayName from the same panel.

.EXAMPLE
    .\scripts\build-msix.ps1
    .\scripts\build-msix.ps1 -Version 0.2.0.0 -SkipBuild
#>
[CmdletBinding()]
param(
    [string]$Version          = "0.1.0.0",
    [string]$PackageName      = "SpaceRecycle",
    [string]$Publisher        = "CN=xinyuemoli",
    [string]$PublisherDisplay = "SpaceRecycle",
    [switch]$SkipBuild
)

$ErrorActionPreference = "Stop"

$root     = Split-Path -Parent $PSScriptRoot
$tauriDir = Join-Path $root "src-tauri"
$msixDir  = Join-Path $tauriDir "target\msix"
$stageDir = Join-Path $msixDir "stage"
$exePath  = Join-Path $tauriDir "target\release\spacerrecycle.exe"
$logoSrc  = Join-Path $tauriDir "icons\icon-1024.png"
$template = Join-Path $tauriDir "msix\Package.appxmanifest.template"
$outFile  = Join-Path $msixDir "SpaceRecycle_${Version}_x64.msix"

if (-not (Get-Command winapp -ErrorAction SilentlyContinue)) {
    throw "winapp CLI not found. Install it with: winget install microsoft.winappcli --source winget"
}
if (-not (Test-Path $template)) { throw "Missing manifest template: $template" }
if (-not (Test-Path $logoSrc))  { throw "Missing icon source (needs >= 400x400): $logoSrc" }

if (-not $SkipBuild) {
    Write-Host "==> cargo build --release" -ForegroundColor Cyan
    Push-Location $tauriDir
    try { cargo build --release } finally { Pop-Location }
}
if (-not (Test-Path $exePath)) { throw "Missing release binary: $exePath (build it first, or drop -SkipBuild)" }

Write-Host "==> staging package layout" -ForegroundColor Cyan
if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
Copy-Item $exePath (Join-Path $stageDir "spacerrecycle.exe") -Force

Write-Host "==> generating image assets" -ForegroundColor Cyan
winapp manifest generate $stageDir `
    --package-name $PackageName `
    --publisher-name $Publisher `
    --version $Version `
    --description "Windows disk-space reclamation tool that never deletes anything on its own." `
    --executable (Join-Path $stageDir "spacerrecycle.exe") `
    --template Packaged `
    --logo-path $logoSrc | Out-Null

Write-Host "==> applying manifest template" -ForegroundColor Cyan
$xml = Get-Content $template -Raw
$xml = $xml.Replace("{{VERSION}}", $Version)
$xml = $xml.Replace("{{PACKAGE_NAME}}", $PackageName)
$xml = $xml.Replace("{{PUBLISHER}}", $Publisher)
$xml = $xml.Replace("{{PUBLISHER_DISPLAY}}", $PublisherDisplay)
Set-Content (Join-Path $stageDir "Package.appxmanifest") $xml -Encoding UTF8

Write-Host "==> packaging and signing MSIX" -ForegroundColor Cyan
Push-Location $msixDir
try {
    winapp package $stageDir --generate-cert --publisher $Publisher --output $outFile
} finally { Pop-Location }

$size = [math]::Round((Get-Item $outFile).Length / 1MB, 2)
Write-Host ""
Write-Host "Built: $outFile ($size MB)" -ForegroundColor Green
Write-Host "Before Store submission all three identity values must match the Partner" -ForegroundColor Yellow
Write-Host "Center 'Product identity' panel: -PackageName, -Publisher, -PublisherDisplay." -ForegroundColor Yellow
