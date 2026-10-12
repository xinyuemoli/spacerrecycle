# Register / unregister the SpaceRecycle context menu, per user.
#
# HKCU only, so this needs no administrator and cannot affect another account.
# It calls the DLL's own DllRegisterServer rather than duplicating the registry
# layout here: one place defines what "installed" means.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('register', 'unregister', 'status')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'
# $PSScriptRoot needs PowerShell 3.0. The verification VM is Windows 7 SP1,
# which ships 2.0, so the directory is derived the way 2.0 can do it.
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Definition }
$dll = Join-Path $scriptDir 'bin\SpaceRecycleShell.dll'
$clsid = '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}'

if ($Action -eq 'status') {
    Write-Host "DLL      : $dll  (exists: $(Test-Path $dll))"
    $comKey = "HKCU:\Software\Classes\CLSID\$clsid\InprocServer32"
    Write-Host "COM key  : $(if (Test-Path $comKey) { (Get-ItemProperty $comKey).'(default)' } else { 'MISSING' })"
    foreach ($cls in @('*', 'Directory')) {
        # -LiteralPath: '*' is a wildcard to the registry provider, so a plain
        # path would match every class and report a meaningless merged value.
        $verb = "HKCU:\Software\Classes\$cls\shell\SpaceRecycle"
        $value = if (Test-Path -LiteralPath $verb) {
            (Get-ItemProperty -LiteralPath $verb -ErrorAction SilentlyContinue).ExplorerCommandHandler
        } else { $null }
        Write-Host "verb [$cls] : $(if ($value) { $value } else { 'MISSING' })"
    }
    exit 0
}

if (-not (Test-Path $dll)) { throw "not built yet: $dll  (run build.cmd)" }

# regsvr32 calls the DLL's own DllRegisterServer / DllUnregisterServer, so the
# registry layout lives in exactly one place - the DLL. It is launched with
# -Wait because regsvr32 is a GUI-subsystem binary: PowerShell's call operator
# returns immediately for those, and an exit code read straight afterwards is
# empty. It does not self-elevate, which is what proves the install really is
# per-user.
$args = if ($Action -eq 'register') { @('/s', $dll) } else { @('/s', '/u', $dll) }
$proc = Start-Process -FilePath 'regsvr32.exe' -ArgumentList $args -PassThru -Wait
if ($proc.ExitCode -ne 0) {
    throw "regsvr32 failed with exit code $($proc.ExitCode)"
}
Write-Host "$Action ok"
Write-Host ""
& $MyInvocation.MyCommand.Definition -Action status