# Registers / unregisters the SpaceRecycle overlay probe.
#
# Overlay handlers live under HKLM, so this must run elevated. It only ever
# touches the two keys it created; unregister removes exactly those.
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('register', 'unregister')]
    [string]$Action
)

$ErrorActionPreference = 'Stop'

$clsid     = '{6F1B2C3D-4E5A-4B7C-9D8E-1A2B3C4D5E6F}'
$dll       = Join-Path $PSScriptRoot 'SpaceRecycleProbe.dll'

# Explorer honours only the first 15 overlay handlers, ordered by a
# character-by-character comparison of the key name. Because a space sorts
# below every letter, MORE leading spaces wins: at the first position where two
# names differ, the one with a space there comes first. OneDrive uses 13 spaces,
# which is why it owns seven slots on a machine that has 35 registered.
#
# Thirteen spaces plus a name starting with "S" therefore lands immediately
# after OneDrive7 and immediately before Tortoise1Normal - inside the 15, and
# displacing only Tortoise8Ignored.
$prefix     = ' ' * 13
$overlayKey = "HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers\$prefix`SpaceRecycleProbe"
$clsidKey   = "HKLM:\SOFTWARE\Classes\CLSID\$clsid"

# Any key this project may have written previously, whatever its spacing: a
# stale entry outside the top 15 is invisible but still clutters the list.
function Remove-StaleOverlayKeys($root) {
    Get-ChildItem $root -ErrorAction SilentlyContinue |
        Where-Object { $_.PSChildName -like '*SpaceRecycle*' } |
        ForEach-Object {
            Remove-Item $_.PSPath -Recurse -Force
            Write-Host "removed stale overlay key [$($_.PSChildName)]"
        }
}

function Remove-KeyIfExists($path) {
    if (Test-Path $path) {
        Remove-Item $path -Recurse -Force
        Write-Host "removed $path"
    }
}

switch ($Action) {
    'register' {
        if (-not (Test-Path $dll)) { throw "DLL not found: $dll" }

        $root = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'
        Remove-StaleOverlayKeys $root

        New-Item -Path $clsidKey -Force | Out-Null
        Set-ItemProperty -Path $clsidKey -Name '(default)' -Value 'SpaceRecycle overlay probe'
        New-Item -Path "$clsidKey\InprocServer32" -Force | Out-Null
        Set-ItemProperty -Path "$clsidKey\InprocServer32" -Name '(default)' -Value $dll
        # Explorer loads shell extensions on a single-threaded apartment thread.
        Set-ItemProperty -Path "$clsidKey\InprocServer32" -Name 'ThreadingModel' -Value 'Apartment'

        New-Item -Path $overlayKey -Force | Out-Null
        Set-ItemProperty -Path $overlayKey -Name '(default)' -Value $clsid

        Write-Host "registered overlay handler -> $dll"
        Write-Host "key name: [$prefix`SpaceRecycleProbe]"
    }
    'unregister' {
        $root = 'HKLM:\SOFTWARE\Microsoft\Windows\CurrentVersion\Explorer\ShellIconOverlayIdentifiers'
        Remove-StaleOverlayKeys $root
        Remove-KeyIfExists $clsidKey
        Write-Host "unregistered"
    }
}