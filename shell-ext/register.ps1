# Register / unregister the SpaceRecycle context menu, per user.
#
# HKCU only, so this needs no administrator and cannot affect another account.
# It calls the DLL's own DllRegisterServer rather than duplicating the registry
# layout here: one place defines what "installed" means.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('register', 'unregister', 'status')]
    [string]$Action,
    # Swallows the stray whitespace argument that the cmd/vmrun quoting chain
    # appends when this is run as `powershell -File register.ps1 -Action ...`.
    # Without it, PowerShell 2.0 tries to bind that argument positionally and
    # fails with "no positional parameter found".
    [Parameter(ValueFromRemainingArguments = $true)]
    $Ignored
)

$ErrorActionPreference = 'Stop'
# $PSScriptRoot needs PowerShell 3.0. The verification VM is Windows 7 SP1,
# which ships 2.0, so the directory is derived the way 2.0 can do it.
$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Definition }
$dll = Join-Path $scriptDir 'bin\SpaceRecycleShell.dll'
$clsid = '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}'

$overlayNames = @('SpaceRecycleImportant', 'SpaceRecycleDisposable')
$overlayClsids = @(
    '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0D}',
    '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0E}'
)

if ($Action -eq 'status') {
    Write-Host "DLL      : $dll  (exists: $(Test-Path $dll))"
    Write-Host ""
    Write-Host "context menu (per user, HKCU)"
    $comKey = "HKCU:\Software\Classes\CLSID\$clsid\InprocServer32"
    Write-Host "  COM key  : $(if (Test-Path $comKey) { (Get-ItemProperty $comKey).'(default)' } else { 'MISSING' })"
    foreach ($cls in @('*', 'Directory')) {
        # -LiteralPath: '*' is a wildcard to the registry provider, so a plain
        # path would match every class and report a meaningless merged value.
        $verb = "HKCU:\Software\Classes\$cls\shell\SpaceRecycle"
        $value = if (Test-Path -LiteralPath $verb) {
            (Get-ItemProperty -LiteralPath $verb -ErrorAction SilentlyContinue).ExplorerCommandHandler
        } else { $null }
        Write-Host "  verb [$cls] : $(if ($value) { $value } else { 'MISSING' })"
    }

    Write-Host ""
    Write-Host "icon overlays (machine wide, HKLM - needs elevation)"
    $slotRoot = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'
    for ($i = 0; $i -lt $overlayNames.Count; $i++) {
        $c = "HKLM:\SOFTWARE\Classes\CLSID\$($overlayClsids[$i])\InprocServer32"
        $comState = if (Test-Path $c) { (Get-ItemProperty $c).'(default)' } else { 'MISSING' }
        # The registry key name carries leading spaces that are significant to
        # Explorer's ordering, so it is matched by suffix rather than exactly.
        $slot = Get-ChildItem $slotRoot -ErrorAction SilentlyContinue |
                Where-Object { $_.PSChildName -like "*$($overlayNames[$i])" } |
                Select-Object -First 1
        Write-Host "  $($overlayNames[$i])"
        Write-Host "    COM    : $comState"
        Write-Host "    slot   : $(if ($slot) { "[$($slot.PSChildName)]" } else { 'MISSING' })"
    }

    # Explorer honours only the first 15 entries, ordered by name.
    if (Test-Path $slotRoot) {
        $all = @(Get-ChildItem $slotRoot | Select-Object -ExpandProperty PSChildName)
        $sorted = @($all | Sort-Object)
        $position = 0
        for ($i = 0; $i -lt $sorted.Count; $i++) {
            if ($sorted[$i] -like '*SpaceRecycle*') { $position = $i + 1; break }
        }
        Write-Host ""
        Write-Host "  total registered : $($all.Count)  (only the first 15 are honoured)"
        if ($position -gt 0) {
            Write-Host "  our first slot   : position $position of $($all.Count)$(if ($position -gt 15) { '  -> OUTSIDE the honoured range' })"
        }
    }
    exit 0
}

if (-not (Test-Path $dll)) { throw "not built yet: $dll  (run build.cmd)" }

# Load the library and call its own DllRegisterServer / DllUnregisterServer, so
# the registry layout lives in exactly one place - the DLL.
#
# regsvr32 would be the conventional route, but it is unusable here for two
# independent reasons: Start-Process -Wait is broken on PowerShell 2.0 and
# throws "the process is not currently running" the moment a short-lived program
# exits, and regsvr32 is a GUI-subsystem binary being started from a session
# with no interactive desktop. Calling the export in-process avoids both, and
# returns a real HRESULT instead of an exit code that has to be guessed at.
Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class SrRegister {
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr LoadLibrary(string path);
    [DllImport("kernel32", SetLastError=true)]
    public static extern IntPtr GetProcAddress(IntPtr module, string name);
    [DllImport("kernel32", SetLastError=true)]
    public static extern bool FreeLibrary(IntPtr module);
}
public delegate int DllRegisterServerFn();
'@

$module = [SrRegister]::LoadLibrary($dll)
if ($module -eq [IntPtr]::Zero) {
    throw "LoadLibrary failed for $dll (error $([Runtime.InteropServices.Marshal]::GetLastWin32Error()))"
}

$exportName = if ($Action -eq 'register') { 'DllRegisterServer' } else { 'DllUnregisterServer' }
$address = [SrRegister]::GetProcAddress($module, $exportName)
if ($address -eq [IntPtr]::Zero) {
    [SrRegister]::FreeLibrary($module) | Out-Null
    throw "export not found: $exportName"
}

$fn = [Runtime.InteropServices.Marshal]::GetDelegateForFunctionPointer($address, [DllRegisterServerFn])
$hr = $fn.Invoke()
[SrRegister]::FreeLibrary($module) | Out-Null

if ($hr -eq 0) {
    Write-Host "$exportName ok (hr=0)"
} elseif ($hr -eq 1) {
    # S_FALSE is the honest partial result: the per-user menu went in, the
    # machine-wide overlays did not, because HKLM needs elevation.
    Write-Host "$exportName partially applied (S_FALSE)"
    Write-Host "  the context menu is registered; the icon overlays are NOT."
    Write-Host "  overlays live only in HKLM, so this needs an elevated shell."
} else {
    throw ("$exportName returned 0x{0:X8}" -f $hr)
}
Write-Host ""
& $MyInvocation.MyCommand.Definition -Action status