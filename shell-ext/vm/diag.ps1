# Architecture / registration diagnostics for the guest.
#
# The symptom is contradictory: the extension's registry keys read back fine
# from the guest's PowerShell, yet an x64 test host reports REGDB_E_CLASSNOTREG
# when it asks COM for the class. Those two facts can only both be true if the
# writer and the reader are looking at different registry views - which is what
# WOW64 redirection does - so this dumps both views plus the process bitness of
# everything involved.
#
# No param block: a stray whitespace argument from the cmd/vmrun quoting chain
# silently overrides a parameter default under PowerShell 2.0.
$ErrorActionPreference = 'Continue'

$o = New-Object System.Collections.Generic.List[string]
function L($t) { $o.Add([string]$t) | Out-Null }

L "PROCESSOR_ARCHITECTURE   = $env:PROCESSOR_ARCHITECTURE"
L "PROCESSOR_ARCHITEW6432   = $env:PROCESSOR_ARCHITEW6432"
L "SysWOW64 present         = $(Test-Path 'C:\Windows\SysWOW64')"
L "SysWOW64\regsvr32.exe    = $(Test-Path 'C:\Windows\SysWOW64\regsvr32.exe')"
try { L "Is64BitProcess           = $([Environment]::Is64BitProcess)" } catch { L "Is64BitProcess           = (unavailable)" }
try { L "Is64BitOperatingSystem   = $([Environment]::Is64BitOperatingSystem)" } catch { L "Is64BitOperatingSystem   = (unavailable)" }
L "powershell.exe           = $([Diagnostics.Process]::GetCurrentProcess().MainModule.FileName)"
L "clr version              = $([Environment]::Version)"
L ""

# The DLL must be 64-bit for 64-bit Explorer and menutest.exe to use it.
$dll = 'C:\sr-test\bin\SpaceRecycleShell.dll'
L "--- DLL ---"
L "exists                   = $(Test-Path $dll)"
if (Test-Path $dll) {
    $fs = New-Object IO.FileStream($dll, [IO.FileMode]::Open, [IO.FileAccess]::Read)
    $br = New-Object IO.BinaryReader($fs)
    $fs.Position = 0x3C
    $peOffset = $br.ReadInt32()
    $fs.Position = $peOffset + 4
    $machine = $br.ReadUInt16()
    $br.Close(); $fs.Close()
    $arch = switch ($machine) { 0x014c { 'x86 (32-bit)' } 0x8664 { 'x64 (64-bit)' } default { "unknown 0x{0:X4}" -f $machine } }
    L "PE machine               = $arch"
}

L ""
L "--- CLSID registry views ---"
$clsid = '{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}'
$paths = @(
    "HKCU:\Software\Classes\CLSID\$clsid\InprocServer32",
    "HKCU:\Software\Classes\Wow6432Node\CLSID\$clsid\InprocServer32",
    "HKLM:\Software\Classes\CLSID\$clsid\InprocServer32",
    "HKLM:\Software\Classes\Wow6432Node\CLSID\$clsid\InprocServer32"
)
foreach ($p in $paths) {
    $v = if (Test-Path $p) { (Get-ItemProperty $p -ErrorAction SilentlyContinue).'(default)' } else { 'MISSING' }
    L "$p"
    L "    -> $v"
}

L ""
L "--- verbs ---"
foreach ($cls in @('*', 'Directory')) {
    foreach ($node in @('', 'Wow6432Node\')) {
        $v = "HKCU:\Software\Classes\$node$cls\shell\SpaceRecycle"
        $h = if (Test-Path -LiteralPath $v) { (Get-ItemProperty -LiteralPath $v -ErrorAction SilentlyContinue).ExplorerCommandHandler } else { 'MISSING' }
        L "  $v -> $h"
    }
}

L ""
L "--- can the DLL be loaded at all? ---"
try {
    Add-Type -Namespace Dg -Name N -MemberDefinition @'
[DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)]
public static extern IntPtr LoadLibrary(string path);
[DllImport("kernel32", SetLastError=true)]
public static extern int GetLastError();
'@ -ErrorAction Stop
    $h = [Dg.N]::LoadLibrary($dll)
    if ($h -eq [IntPtr]::Zero) {
        L "LoadLibrary              = FAILED, GetLastError=$([Dg.N]::GetLastError())"
    } else {
        L "LoadLibrary              = OK"
    }
} catch {
    L "LoadLibrary probe        = $($_.Exception.Message)"
}

$o | Set-Content -Path 'C:\sr-test\diag.txt' -Encoding UTF8
$o | ForEach-Object { Write-Host $_ }