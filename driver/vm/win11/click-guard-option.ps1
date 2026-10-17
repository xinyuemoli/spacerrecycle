# Selects one of the guard's dispositions through the accessibility tree.
#
# Two earlier attempts failed for reasons that are worth keeping, because both
# are easy to repeat:
#
#   The automation pattern was looked up as [AutomationElement]::InvokePattern.
#   No such static field exists - the pattern identifiers live on the pattern
#   classes ([InvokePattern]::Pattern, [SelectionItemPattern]::Pattern) - so the
#   lookup returned null, GetCurrentPattern was handed a null argument, and the
#   fallback was a click.
#
#   The click then went to the wrong place: the guest screen is 1024x768 and the
#   app window is 1200x780, so the bottom of the window sits behind the taskbar.
#   The tree still reports a rectangle for those controls and a click at those
#   coordinates lands on the taskbar.
#
# Selecting through the pattern needs neither a visible pixel nor a focus
# change.

param(
  [string]$Option = "Block it, without asking",
  [string]$PolicyPath = "C:\ProgramData\SpaceRecycle\policy.ini"
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement
Write-Output "session: $((Get-Process -Id $PID).SessionId)"

$radioCondition = New-Object System.Windows.Automation.PropertyCondition(
  [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
  [System.Windows.Automation.ControlType]::RadioButton)
$radios = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $radioCondition)
Write-Output "radio buttons: $($radios.Count)"

$target = $null
foreach ($r in $radios) {
  if ($r.Current.Name -like "*$Option*") { $target = $r }
}
if ($null -eq $target) { Write-Output "FAIL: no radio button matching '$Option'"; exit 2 }
Write-Output "target: '$($target.Current.Name)'"

$pattern = $target.GetCurrentPattern(
  [System.Windows.Automation.SelectionItemPattern]::Pattern)
$pattern.Select()
Write-Output "Select() called"

Start-Sleep -Seconds 4

Write-Output ""
Write-Output "=== the file the service watches ==="
if (Test-Path $PolicyPath) { Get-Content $PolicyPath | ForEach-Object { "  $_" } }
else { Write-Output "  MISSING: $PolicyPath" }

Write-Output ""
Write-Output "=== selection state ==="
$radios = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants, $radioCondition)
foreach ($r in $radios) {
  $sel = "?"
  try {
    $p = $r.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
    $sel = "selected=$($p.Current.IsSelected)"
  } catch { $sel = "no pattern" }
  Write-Output "  '$($r.Current.Name)'  $sel"
}

Write-Output ""
Write-Output "done"
