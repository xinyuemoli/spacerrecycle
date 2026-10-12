# Isolates the COM failure.
#
# An x64 test host reports REGDB_E_CLASSNOTREG for a CLSID that the registry
# undeniably contains. Two very different causes produce that same code:
#   1. COM never found the registration (registry view / hive problem), or
#   2. COM loaded the DLL, the DLL was asked for the class, and the DLL said
#      CLASS_E_CLASSNOTAVAILABLE - which CoCreateInstance surfaces as
#      REGDB_E_CLASSNOTREG rather than as the DLL's own error.
#
# Calling the export by hand separates them: if this returns S_OK, the DLL's
# class object is fine and the problem is registration lookup. If it returns
# CLASS_E_CLASSNOTAVAILABLE, the DLL itself is rejecting the CLSID.
$ErrorActionPreference = 'Continue'

$o = New-Object System.Collections.Generic.List[string]
function L($t) { $o.Add([string]$t) | Out-Null }

Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class NativeProbe {
    [DllImport("kernel32", CharSet=CharSet.Unicode, SetLastError=true)]
    public static extern IntPtr LoadLibrary(string path);
    [DllImport("kernel32", SetLastError=true)]
    public static extern IntPtr GetProcAddress(IntPtr module, string name);
}
public delegate int DllGetClassObjectFn(ref Guid rclsid, ref Guid riid, out IntPtr ppv);
'@

$dll = 'C:\sr-test\bin\SpaceRecycleShell.dll'
$module = [NativeProbe]::LoadLibrary($dll)
L "LoadLibrary            = $(if ($module -eq [IntPtr]::Zero) { 'FAILED err=' + [Runtime.InteropServices.Marshal]::GetLastWin32Error() } else { 'OK' })"

$proc = [NativeProbe]::GetProcAddress($module, 'DllGetClassObject')
L "GetProcAddress(DllGetClassObject) = $(if ($proc -eq [IntPtr]::Zero) { 'MISSING' } else { 'found' })"

if ($proc -ne [IntPtr]::Zero) {
    $fn = [Runtime.InteropServices.Marshal]::GetDelegateForFunctionPointer($proc, [DllGetClassObjectFn])
    $iidUnknown = [Guid]'00000000-0000-0000-C000-000000000046'

    foreach ($candidate in @('7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C',   # ours
                             '00000000-0000-0000-0000-000000000000')) { # deliberately wrong
        $clsid = [Guid]$candidate
        $iid = $iidUnknown
        $ppv = [IntPtr]::Zero
        try {
            $hr = $fn.Invoke([ref]$clsid, [ref]$iid, [ref]$ppv)
            L ("DllGetClassObject({0}) -> hr=0x{1:X8} ppv={2}" -f $candidate, $hr, $ppv)
        } catch {
            L "DllGetClassObject($candidate) -> threw: $($_.Exception.Message)"
        }
    }
}

L ""
L "--- what does the registry say to a shell-level reader? ---"
foreach ($p in @("HKCU:\Software\Classes\CLSID\$('{7A5E3C21-9B4D-4E8F-A6C2-1D3E5F7A9B0C}')\InprocServer32")) {
    if (Test-Path $p) {
        $item = Get-Item $p
        L "$p"
        L "    (default)     = $((Get-ItemProperty $p).'(default)')"
        L "    ThreadingModel= $((Get-ItemProperty $p).ThreadingModel)"
    } else {
        L "$p -> MISSING"
    }
}

# The per-user hive COM actually reads depends on the profile being loaded.
L ""
L "user hives loaded:"
L ((Get-ChildItem 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion\ProfileList' |
    ForEach-Object { "  " + $_.PSChildName + " -> " + (Get-ItemProperty $_.PSPath).ProfileImagePath }) -join "`n")

$o | Set-Content -Path 'C:\sr-test\diag2.txt' -Encoding UTF8
$o | ForEach-Object { Write-Host $_ }