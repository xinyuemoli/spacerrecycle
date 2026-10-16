# Drives the running app through UI Automation and reports what it renders.
#
# Why not just a screenshot: a picture of the window proves the window exists,
# not that the settings card says what it is supposed to say in each state. The
# accessibility tree is the same content the screen reader sees, so finding the
# labels there is a real assertion about what the app rendered - and it can be
# produced and checked without anyone looking at an image.
#
# WebView2 exposes the DOM through this tree, so the tab is a Button carrying
# its visible label and the card's text appears as Text/Group elements.

param(
  [string]$Tab = "Settings",
  [string[]]$Expect = @()
)

$ErrorActionPreference = "Stop"
Add-Type -AssemblyName UIAutomationClient
Add-Type -AssemblyName UIAutomationTypes

# Which session this is running in decides whether it can see the app at all:
# an automation client only sees the desktop of its own session, so a process
# left in session 0 would find an empty tree and report the tab as missing.
Write-Output "automation client session: $((Get-Process -Id $PID).SessionId)"
$app = Get-Process spacerrecycle -ErrorAction SilentlyContinue | Select-Object -First 1
if ($app) {
  Write-Output "app session: $($app.SessionId)  pid $($app.Id)  title '$($app.MainWindowTitle)'"
} else {
  Write-Output "app process not found"
}

$root = [System.Windows.Automation.AutomationElement]::RootElement

function Find-ByName {
  param([string]$Name)
  $cond = New-Object System.Windows.Automation.PropertyCondition(
    [System.Windows.Automation.AutomationElement]::NameProperty, $Name)
  return $root.FindFirst([System.Windows.Automation.TreeScope]::Descendants, $cond)
}

Write-Output "=== switching to the '$Tab' tab ==="
$tabElement = Find-ByName $Tab
if ($null -eq $tabElement) {
  # Deliberately not fatal: an empty or unexpected tree is the interesting
  # result, and exiting here would hide the dump that shows what it actually
  # contains.
  Write-Output "  NOT FOUND: no element named '$Tab'"
} else {
  Write-Output "  found: $($tabElement.Current.ControlType.ProgrammaticName)"

  $invoked = $false
  foreach ($patternName in @("InvokePattern", "SelectionItemPattern", "LegacyIAccessiblePattern")) {
    try {
      $pattern = $tabElement.GetCurrentPattern(
        [System.Windows.Automation.AutomationElement]::$patternName)
      if ($patternName -eq "InvokePattern") { $pattern.Invoke() }
      elseif ($patternName -eq "SelectionItemPattern") { $pattern.Select() }
      else { $pattern.DoDefaultAction() }
      Write-Output "  activated via $patternName"
      $invoked = $true
      break
    } catch {
      Write-Output "  $patternName not usable: $($_.Exception.Message.Split([char]10)[0])"
    }
  }
  if (-not $invoked) {
    # The pattern objects come back null through PowerShell's static field
    # access, so the tab is clicked by coordinate instead. The rectangle comes
    # from the element itself, so this is still driven by what the accessibility
    # tree reports rather than by a hard-coded position.
    Write-Output "  falling back to a click at the element's own rectangle"
    Add-Type -AssemblyName System.Windows.Forms
    Add-Type -Namespace Sr -Name Mouse -MemberDefinition @'
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern bool SetCursorPos(int X, int Y);
[System.Runtime.InteropServices.DllImport("user32.dll")]
public static extern void mouse_event(uint f, uint dx, uint dy, uint d, System.UIntPtr e);
'@
    $r = $tabElement.Current.BoundingRectangle
    $x = [int]($r.Left + $r.Width / 2)
    $y = [int]($r.Top + $r.Height / 2)
    Write-Output "  rectangle $($r.Left),$($r.Top) $($r.Width)x$($r.Height) -> clicking $x,$y"
    [void][Sr.Mouse]::SetCursorPos($x, $y)
    Start-Sleep -Milliseconds 300
    [Sr.Mouse]::mouse_event(0x02, 0, 0, 0, [UIntPtr]::Zero)   # left down
    Start-Sleep -Milliseconds 80
    [Sr.Mouse]::mouse_event(0x04, 0, 0, 0, [UIntPtr]::Zero)   # left up
    $invoked = $true
  }

  Start-Sleep -Seconds 3
}

Write-Output ""
Write-Output "=== checking the expected content ==="
$failures = 0
foreach ($text in $Expect) {
  $found = Find-ByName $text
  if ($null -ne $found) {
    Write-Output "  PASS  $text"
  } else {
    Write-Output "  FAIL  $text"
    $failures++
  }
}

Write-Output ""
Write-Output "=== everything the app currently exposes (first 120) ==="
$all = $root.FindAll([System.Windows.Automation.TreeScope]::Descendants,
                     [System.Windows.Automation.Condition]::TrueCondition)
$names = @()
foreach ($e in $all) {
  $n = $e.Current.Name
  if ($n -and $n.Trim().Length -gt 0) { $names += $n }
}
$names | Select-Object -Unique | Select-Object -First 120 | ForEach-Object { "  $_" }

Write-Output ""
Write-Output "failures: $failures"
exit $failures
