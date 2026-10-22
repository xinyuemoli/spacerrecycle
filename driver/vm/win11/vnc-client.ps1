# Minimal RFB client, used to look at and type into a guest whose credentials are
# not available to vmrun.
#
# The situation this exists for: vmrun refuses blank passwords ("the guest OS
# does not support blank passwords") and refuses anonymous guest operations, so
# once an account's password is cleared there is no guest-operations route left.
# A VMware virtual machine can expose its console over VNC, and that channel is
# independent of the guest's accounts, so it still works.
#
# Written by hand rather than with a library because only two messages are
# needed: a framebuffer update to see the screen, and a key event to type into
# it. Both are a handful of bytes.

param(
  [int]$Port = 5901,
  [string]$Action = "dump",          # dump | key | shot
  [string]$Text = "",
  [string]$OutFile = "$PSScriptRoot\_vnc.png",
  [int]$TimeoutMs = 8000
)

$ErrorActionPreference = "Stop"

$client = New-Object System.Net.Sockets.TcpClient
$client.Connect("127.0.0.1", $Port)
$stream = $client.GetStream()
$stream.ReadTimeout = $TimeoutMs

function Read-N {
  param([int]$Count)
  $buf = New-Object byte[] $Count
  $got = 0
  while ($got -lt $Count) {
    $n = $stream.Read($buf, $got, $Count - $got)
    if ($n -le 0) { throw "closed after $got/$Count bytes" }
    $got += $n
  }
  return $buf
}
function Send-Bytes { param([byte[]]$B) $stream.Write($B, 0, $B.Length); $stream.Flush() }
function Hex { param([byte[]]$B) ($B | ForEach-Object { $_.ToString("X2") }) -join " " }

Write-Output "--- handshake ---"
$version = [Text.Encoding]::ASCII.GetString((Read-N 12))
Write-Output "  server: $($version.Trim())"
Send-Bytes ([Text.Encoding]::ASCII.GetBytes("RFB 003.008`n"))

$count = (Read-N 1)[0]
Write-Output "  security types: $count"
$types = if ($count -gt 0) { Read-N $count } else { @() }
if ($count -gt 0) { Write-Output "  offered: $((($types | ForEach-Object { $_ }) -join ','))" }
if ($count -eq 0) {
  $len = [BitConverter]::ToUInt32((Read-N 4), 0)
  Write-Output "  server refused: $([Text.Encoding]::ASCII.GetString((Read-N $len)))"
  exit 1
}
Send-Bytes ([byte[]]@(1))                       # security type None

# Try to read the 4-byte SecurityResult, but do not depend on it: VMware may or
# may not send one for the None type, and blocking on a message that never
# arrives is what stalled the earlier attempts.
$stream.ReadTimeout = 2000
$gotResult = $false
try {
  $probe = Read-N 4
  Write-Output "  4 bytes after choosing None: $(Hex $probe)"
  if ($probe[0] -eq 0 -and $probe[1] -eq 0 -and $probe[2] -eq 0 -and $probe[3] -eq 0) {
    Write-Output "  interpreted as SecurityResult OK"
    $gotResult = $true
  }
} catch {
  Write-Output "  no SecurityResult (timed out); continuing"
}
$stream.ReadTimeout = $TimeoutMs

Send-Bytes ([byte[]]@(1))                       # ClientInit, shared

if (-not $gotResult) {
  # The four bytes may not have arrived at all, in which case ServerInit is
  # still in flight; nothing to do but read it.
}

$header = Read-N 24
$width  = [BitConverter]::ToUInt16($header, 0)
$height = [BitConverter]::ToUInt16($header, 2)
$nameLen = [BitConverter]::ToUInt32($header, 20)
$name = if ($nameLen -gt 0) { [Text.Encoding]::ASCII.GetString((Read-N $nameLen)) } else { "" }
Write-Output "  desktop: ${width}x${height}  name='$name'"

if ($Action -eq "key") {
  Write-Output "--- typing '$Text' ---"
  foreach ($ch in $Text.ToCharArray()) {
    $sym = [int][char]$ch
    foreach ($down in 1, 0) {
      $msg = New-Object byte[] 8
      $msg[0] = 4; $msg[1] = [byte]$down
      [Array]::Copy([BitConverter]::GetBytes([uint32]$sym), 0, $msg, 4, 4)
      Send-Bytes $msg
    }
    Start-Sleep -Milliseconds 60
  }
  Write-Output "  typed $($Text.Length) characters"
  $client.Close()
  exit 0
}

# --- framebuffer update ---
# SetPixelFormat (32bpp little-endian), then SetEncodings with Raw only, then
# request the whole screen.
Write-Output "--- framebuffer ---"
$pf = New-Object byte[] 20
$pf[0] = 0                                   # message-type SetPixelFormat
$pf[4] = 32                                  # bits-per-pixel
$pf[5] = 24                                  # depth
$pf[6] = 0                                   # big-endian flag
$pf[7] = 1                                   # true-colour flag
[Array]::Copy([BitConverter]::GetBytes([uint16]255), 0, $pf, 8, 2)   # red max
[Array]::Copy([BitConverter]::GetBytes([uint16]255), 0, $pf, 10, 2)  # green max
[Array]::Copy([BitConverter]::GetBytes([uint16]255), 0, $pf, 12, 2)  # blue max
$pf[14] = 16; $pf[15] = 8                    # red shift
$pf[16] = 8;  $pf[17] = 8                    # green shift
$pf[18] = 0;  $pf[19] = 8                    # blue shift
Send-Bytes $pf

$enc = New-Object byte[] 8
$enc[0] = 2; $enc[1] = 0                     # message-type SetEncodings, 1 entry
[Array]::Copy([BitConverter]::GetBytes([int32]0), 0, $enc, 4, 4)     # Raw
Send-Bytes $enc

$req = New-Object byte[] 10
$req[0] = 3                                  # message-type FramebufferUpdateRequest
$req[1] = 0                                  # not incremental
Send-Bytes $req

$msgType = (Read-N 1)[0]
Write-Output "  server message type: $msgType"
if ($msgType -ne 0) { Write-Output "  (not a framebuffer update)"; $client.Close(); exit 0 }

Read-N 1 | Out-Null                          # padding
$rects = [BitConverter]::ToUInt16((Read-N 2), 0)
Write-Output "  rectangles: $rects"

Add-Type -AssemblyName System.Drawing
$bmp = New-Object System.Drawing.Bitmap($width, $height)
for ($i = 0; $i -lt $rects; $i++) {
  $r = Read-N 12
  $x = [BitConverter]::ToUInt16($r, 0); $y = [BitConverter]::ToUInt16($r, 2)
  $w = [BitConverter]::ToUInt16($r, 4); $h = [BitConverter]::ToUInt16($r, 6)
  $encoding = [BitConverter]::ToInt32($r, 8)
  Write-Output "  rect ${w}x${h} at $x,$y encoding=$encoding"
  if ($encoding -ne 0) { Write-Output "  (not raw; giving up)"; break }
  $pixels = Read-N ($w * $h * 4)
  for ($row = 0; $row -lt $h; $row++) {
    for ($col = 0; $col -lt $w; $col++) {
      $o = ($row * $w + $col) * 4
      $bmp.SetPixel($x + $col, $y + $row,
        [System.Drawing.Color]::FromArgb($pixels[$o + 2], $pixels[$o + 1], $pixels[$o]))
    }
  }
}
$bmp.Save($OutFile, [System.Drawing.Imaging.ImageFormat]::Png)
$bmp.Dispose()
$client.Close()
Write-Output "  saved $OutFile"
