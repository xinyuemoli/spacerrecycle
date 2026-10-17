# Switches the running app to one of its tabs and reports the window rectangle.
#
# The rectangle goes back to the host so each screenshot can be cropped to the
# app window instead of being a picture of the whole desktop. It has to come
# from the guest because that is where the window lives.
#
# The tab is activated through InvokePattern rather than by clicking. Clicking
# was tried and failed for a reason that still applies: the app window is
# 1200x780, and on a 1024x768 guest the bottom of it sits under the taskbar, so
# a click at a control's reported coordinates lands on the taskbar. A pattern
# needs neither a visible pixel nor a focus change.

param(
  [Parameter(Mandatory = $true)][string]$Tab
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

$root = [System.Windows.Automation.AutomationElement]::RootElement

function Find-ByName([string]$Name) {
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
  return $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

# --- the app window ---
$windowCondition = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition(
     [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
     [System.Windows.Automation.ControlType]::Window)),
  (New-Object System.Windows.Automation.PropertyCondition(
     [System.Windows.Automation.AutomationElement]::NameProperty, "SpaceRecycle")))
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $windowCondition)
if ($null -eq $window) { Write-Output "FAIL: app window not found"; exit 2 }

$r = $window.Current.BoundingRectangle
Write-Output ("RECT {0} {1} {2} {3}" -f [int]$r.Left, [int]$r.Top, [int]$r.Width, [int]$r.Height)

# --- the tab ---
$tab = Find-ByName $Tab
if ($null -eq $tab) { Write-Output "FAIL: no element named '$Tab'"; exit 3 }

try {
  $invoke = $tab.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern)
  $invoke.Invoke()
  Write-Output "invoked '$Tab'"
} catch {
  # Older markup exposes the tabs as selectable items instead of buttons.
  $select = $tab.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern)
  $select.Select()
  Write-Output "selected '$Tab'"
}

Start-Sleep -Seconds 3
Write-Output "done"
