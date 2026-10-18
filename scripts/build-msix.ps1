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

.PARAMETER AddonStoreId
    Product ID of the subscription add-on, compiled into the binary as
    DEFAULT_SUBSCRIPTION_ADDON_STORE_ID. Defaults to $env:SPACERECYCLE_ADDON_ID.

    Leaving this empty is allowed - the package still builds and certifies -
    but read the warning the script prints, because the gate then never closes.

.PARAMETER GraphClientId
    Azure application (client) ID for the OneDrive device-code flow, compiled in
    as DEFAULT_GRAPH_CLIENT_ID. Defaults to $env:SPACERECYCLE_GRAPH_CLIENT_ID.

.EXAMPLE
    .\scripts\build-msix.ps1
    .\scripts\build-msix.ps1 -Version 0.2.0.0 -SkipBuild
    .\scripts\build-msix.ps1 -AddonStoreId SpaceRecyclePro -GraphClientId 00000000-1111-2222-3333-444444444444
#>
[CmdletBinding()]
param(
    [string]$Version          = "1.0.0.0",
    [string]$PackageName      = "SpaceRecycle",
    [string]$Publisher        = "CN=xinyuemoli",
    [string]$PublisherDisplay = "SpaceRecycle",
    [string]$AddonStoreId     = $env:SPACERECYCLE_ADDON_ID,
    [string]$GraphClientId    = $env:SPACERECYCLE_GRAPH_CLIENT_ID,
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

# The Store rejects a package whose version starts with 0: for Windows 10/11
# packages the Major section "cannot be 0" (and the fourth section is reserved
# for the Store and must be 0). Fail fast instead of building something unusable.
$verParts = $Version.Split('.')
if ($verParts.Count -ne 4) { throw "Version '$Version' must be in Major.Minor.Build.Revision form." }
if ([int]$verParts[0] -lt 1) { throw "Invalid package version '$Version': the Major section cannot be 0 for a Store submission (use e.g. 1.0.0.0)." }
if ([int]$verParts[3] -ne 0) { throw "Invalid package version '$Version': the fourth section is reserved for the Store and must be 0." }

if (-not $SkipBuild) {
    Write-Host "==> cargo build --release" -ForegroundColor Cyan
    Push-Location $tauriDir
    try {
        # The two IDs are compiled in with option_env!, so they have to be in the
        # environment of the cargo process - not written to a file afterwards.
        # They are not secrets: the device-code flow uses a public client, so the
        # client ID is visible in every authorization request anyway, and the
        # add-on ID is visible to anyone who reads the binary.
        $env:SPACERECYCLE_ADDON_ID = $AddonStoreId
        $env:SPACERECYCLE_GRAPH_CLIENT_ID = $GraphClientId

        # cmd does the redirection, and that is the whole point.
        #
        # cargo writes its progress lines ("Compiling ...", "Finished ...") to
        # stderr. In Windows PowerShell, a native command's stderr becomes an
        # ErrorRecord, and this script runs with $ErrorActionPreference = "Stop",
        # so the first progress line terminates the build. Adding `2>&1` to the
        # PowerShell pipeline does not help - the record is still an error record.
        # Letting cmd.exe merge the two streams means PowerShell never sees one:
        # all of it arrives as ordinary stdout.
        #
        # $LASTEXITCODE is the only thing that actually says whether it worked,
        # so it is checked explicitly rather than inferred from the absence of an
        # exception - which is precisely the inference that was wrong here.
        & cmd.exe /c "cargo build --release 2>&1" | ForEach-Object { Write-Host "    $_" }
        if ($LASTEXITCODE -ne 0) {
            throw "cargo build --release failed with exit code $LASTEXITCODE"
        }
    } finally {
        Pop-Location
        Remove-Item Env:\SPACERECYCLE_ADDON_ID -ErrorAction SilentlyContinue
        Remove-Item Env:\SPACERECYCLE_GRAPH_CLIENT_ID -ErrorAction SilentlyContinue
    }
}
if (-not (Test-Path $exePath)) { throw "Missing release binary: $exePath (build it first, or drop -SkipBuild)" }

# A Store build with no add-on ID is not a build with the paid tier switched
# off. It is a build with the paid tier switched ON: the entitlement resolves to
# NotConfigured, which means "nothing is on sale here", and the app correctly
# concludes there is nothing to gate. Every paid feature ships free, silently,
# to everyone - and the only sign is that nobody is ever asked to subscribe.
# That is a distribution decision, so it is stated plainly rather than left to
# be discovered after release.
if ([string]::IsNullOrWhiteSpace($AddonStoreId) -and -not $SkipBuild) {
    Write-Host ""
    Write-Host "WARNING: no add-on Store ID was supplied." -ForegroundColor Yellow
    Write-Host "  The package will be built, but the subscription gate will never" -ForegroundColor Yellow
    Write-Host "  close: every paid feature is open to every user, for free." -ForegroundColor Yellow
    Write-Host "  Pass -AddonStoreId <product id> once the add-on exists." -ForegroundColor Yellow
    Write-Host ""
}
if ([string]::IsNullOrWhiteSpace($GraphClientId) -and -not $SkipBuild) {
    Write-Host "NOTE: no Azure client ID was supplied - OneDrive account linking will" -ForegroundColor Yellow
    Write-Host "      report itself as unconfigured and the sign-in button stays inert." -ForegroundColor Yellow
}

Write-Host "==> staging package layout" -ForegroundColor Cyan
if (Test-Path $stageDir) { Remove-Item $stageDir -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stageDir | Out-Null
Copy-Item $exePath (Join-Path $stageDir "spacerrecycle.exe") -Force

Write-Host "==> generating image assets" -ForegroundColor Cyan
$generate = "winapp manifest generate `"$stageDir`" " +
    "--package-name `"$PackageName`" " +
    "--publisher-name `"$Publisher`" " +
    "--version $Version " +
    "--description `"Windows disk-space reclamation tool that never deletes anything on its own.`" " +
    "--executable `"$(Join-Path $stageDir 'spacerrecycle.exe')`" " +
    "--template Packaged " +
    "--logo-path `"$logoSrc`""
# cmd merges the streams for the same reason as the cargo call above: winapp is
# a native command, and its stderr would otherwise terminate this script.
& cmd.exe /c "$generate 2>&1" | ForEach-Object { Write-Host "    $_" }
if ($LASTEXITCODE -ne 0) { throw "winapp manifest generate failed with exit code $LASTEXITCODE" }

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
    & cmd.exe /c "winapp package `"$stageDir`" --generate-cert --publisher `"$Publisher`" --output `"$outFile`" 2>&1" |
        ForEach-Object { Write-Host "    $_" }
    if ($LASTEXITCODE -ne 0) { throw "winapp package failed with exit code $LASTEXITCODE" }
} finally { Pop-Location }

$size = [math]::Round((Get-Item $outFile).Length / 1MB, 2)
Write-Host ""
Write-Host "Built: $outFile ($size MB)" -ForegroundColor Green
Write-Host "Before Store submission all three identity values must match the Partner" -ForegroundColor Yellow
Write-Host "Center 'Product identity' panel: -PackageName, -Publisher, -PublisherDisplay." -ForegroundColor Yellow
