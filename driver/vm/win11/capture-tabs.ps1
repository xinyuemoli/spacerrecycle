# Captures one screenshot per tab of the running app, from inside the guest.
#
# Done in a single guest-side run rather than by switching a tab from the host
# and capturing after each one. The host-side version needed a scheduled task
# per tab and a file round trip between each step, which is both slow and easy
# to get wrong: two of the four runs produced no output at all and two produced
# the same picture, because the switch and the capture were racing each other.
# Capturing here removes the race - the screen is grabbed by the same process
# that just changed it, after it has waited for the change to land.
#
# The image is cropped to the app window using the rectangle the accessibility
# tree reports for it, so the result is the window rather than a picture of the
# whole desktop with the window somewhere in it.

param(
  [string]$OutDir = "C:\sr-test\vm\shots",
  [string[]]$Tabs = @("Overview", "Scan Results", "Quarantine", "Settings")
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes
Add-Type -AssemblyName System.Drawing

New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

$root = [System.Windows.Automation.AutomationElement]::RootElement

# The window is looked up once; its rectangle is reused for every crop.
$windowCondition = New-Object System.Windows.Automation.AndCondition(
  (New-Object System.Windows.Automation.PropertyCondition(
     [System.Windows.Automation.AutomationElement]::ControlTypeProperty,
     [System.Windows.Automation.ControlType]::Window)),
  (New-Object System.Windows.Automation.PropertyCondition(
     [System.Windows.Automation.AutomationElement]::NameProperty, "SpaceRecycle")))
$window = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $windowCondition)
if ($null -eq $window) { Write-Output "FAIL: app window not found"; exit 2 }
$wr = $window.Current.BoundingRectangle
Write-Output ("window rect {0},{1} {2}x{3}" -f [int]$wr.Left, [int]$wr.Top, [int]$wr.Width, [int]$wr.Height)

$index = 0
foreach ($tabName in $Tabs) {
  $index++
  Write-Output ""
  Write-Output "--- $tabName ---"

  $condition = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $tabName)
  $tab = $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $condition)
  if ($null -eq $tab) { Write-Output "  not found"; continue }

  # Reported explicitly: a lookup that silently returns a string instead of an
  # element makes the pattern call below fail with a message about String not
  # having the method, which says nothing about where the value came from.
  Write-Output "  element type: $($tab.GetType().FullName)"

  $switched = $false
  try {
    $tab.GetCurrentPattern([System.Windows.Automation.InvokePattern]::Pattern).Invoke()
    Write-Output "  invoked"
    $switched = $true
  } catch {
    Write-Output "  invoke failed: $($_.Exception.Message.Split([char]10)[0])"
  }
  if (-not $switched) {
    try {
      $tab.GetCurrentPattern([System.Windows.Automation.SelectionItemPattern]::Pattern).Select()
      Write-Output "  selected"
      $switched = $true
    } catch {
      Write-Output "  select failed: $($_.Exception.Message.Split([char]10)[0])"
    }
  }
  if (-not $switched) { continue }

  # Long enough for the view swap and its data to render; the previous
  # host-driven attempt captured before this had settled.
  Start-Sleep -Seconds 4

  $full = New-Object System.Drawing.Bitmap(1920, 1200)
  $g = [System.Drawing.Graphics]::FromImage($full)
  $g.CopyFromScreen(0, 0, 0, 0, $full.Size)
  $g.Dispose()

  $x = [Math]::Max(0, [int]$wr.Left); $y = [Math]::Max(0, [int]$wr.Top)
  $w = [Math]::Min([int]$wr.Width, $full.Width - $x)
  $h = [Math]::Min([int]$wr.Height, $full.Height - $y)
  $crop = New-Object System.Drawing.Bitmap($w, $h)
  $cg = [System.Drawing.Graphics]::FromImage($crop)
  $cg.DrawImage($full, (New-Object System.Drawing.Rectangle(0, 0, $w, $h)),
                (New-Object System.Drawing.Rectangle($x, $y, $w, $h)),
                [System.Drawing.GraphicsUnit]::Pixel)
  $cg.Dispose()
  $full.Dispose()

  $safe = ($tabName -replace "[^A-Za-z0-9]", "-").ToLower()
  $file = Join-Path $OutDir ("{0}-{1}.png" -f $index, $safe)
  $crop.Save($file, [System.Drawing.Imaging.ImageFormat]::Png)
  $crop.Dispose()
  Write-Output "  saved $file (${w}x${h})"
}

Write-Output ""
Write-Output "done"
