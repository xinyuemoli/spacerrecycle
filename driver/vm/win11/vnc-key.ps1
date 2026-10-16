# Sends a keystroke to a VMware virtual machine over its built-in VNC server.
#
# Why this exists: the Windows install media waits a few seconds for a keypress
# ("Press any key to boot from CD or DVD...") and boots the next device if none
# arrives. In an unattended install nobody is there to press it, so the firmware
# gives up and reports that no operating system was found - which looks like a
# broken ISO rather than a missing keypress. vmrun has no keystroke injection at
# all, and the Vix API only reaches the guest through VMware Tools, which by
# definition is not running on a machine with no operating system on it.
#
# VMware exposes the console over VNC, and the RFB protocol has a KeyEvent
# message, so this does the handshake by hand and presses a key. It is used only
# to get past the CD boot prompt; everything after that goes through vmrun.
#
# Requires in the .vmx, before the machine is powered on:
#     RemoteDisplay.vnc.enabled = "TRUE"
#     RemoteDisplay.vnc.port = "5901"
#
# Usage:  vnc-key.ps1 [-Port 5901] [-Seconds 60] [-Key space|enter]

param(
  [int]$Port = 5901,
  [int]$Seconds = 60,
  [ValidateSet("space", "enter", "f6")]
  [string]$Key = "space"
)

$ErrorActionPreference = "Stop"

# X11 keysyms. These are what RFB carries, not scan codes.
$keysym = switch ($Key) {
  "space" { 0x20 }
  "enter" { 0xFF0D }
  "f6"    { 0xFFC3 }
}

function Read-Exactly {
  param([System.Net.Sockets.NetworkStream]$Stream, [int]$Count)
  $buffer = New-Object byte[] $Count
  $read = 0
  while ($read -lt $Count) {
    $n = $Stream.Read($buffer, $read, $Count - $read)
    if ($n -le 0) { throw "connection closed after $read of $Count bytes" }
    $read += $n
  }
  return $buffer
}

function Send-Bytes {
  param([System.Net.Sockets.NetworkStream]$Stream, [byte[]]$Bytes)
  $Stream.Write($Bytes, 0, $Bytes.Length)
  $Stream.Flush()
}

function Send-Key {
  param([System.Net.Sockets.NetworkStream]$Stream, [uint32]$Sym)
  # KeyEvent: message-type(1) down-flag(1) padding(2) keysym(4)
  foreach ($down in 1, 0) {
    $msg = New-Object byte[] 8
    $msg[0] = 4
    $msg[1] = [byte]$down
    $bytes = [BitConverter]::GetBytes($Sym)
    [Array]::Copy($bytes, 0, $msg, 4, 4)
    Send-Bytes -Stream $Stream -Bytes $msg
  }
}

Write-Output "connecting to 127.0.0.1:$Port"

$client = New-Object System.Net.Sockets.TcpClient
$client.Connect("127.0.0.1", $Port)
$stream = $client.GetStream()

# --- handshake ---
$serverVersion = [Text.Encoding]::ASCII.GetString((Read-Exactly -Stream $stream -Count 12))
Write-Output "  server version: $($serverVersion.Trim())"
Send-Bytes -Stream $stream -Bytes ([Text.Encoding]::ASCII.GetBytes("RFB 003.008`n"))

$count = (Read-Exactly -Stream $stream -Count 1)[0]
if ($count -eq 0) {
  $reasonLength = [BitConverter]::ToUInt32((Read-Exactly -Stream $stream -Count 4), 0)
  $reason = [Text.Encoding]::ASCII.GetString((Read-Exactly -Stream $stream -Count $reasonLength))
  throw "server refused the connection: $reason"
}
$types = Read-Exactly -Stream $stream -Count $count
Write-Output "  security types offered: $(($types | ForEach-Object { $_ }) -join ',')"

if ($types -notcontains 1) {
  throw "only security type 1 (none) is supported; set RemoteDisplay.vnc.password to empty"
}
Send-Bytes -Stream $stream -Bytes ([byte[]]@(1))

# ClientInit goes out immediately, before anything is read back. RFB 3.8 says
# the server answers a None security type with a SecurityResult word, but VMware
# does not send one, and waiting for it blocks forever - which is exactly how
# the first attempt failed, silently sending no keys at all and letting the boot
# prompt time out again. Sending ClientInit early is valid either way, and the
# ambiguity is then resolved by inspecting what comes back: SecurityResult OK is
# four zero bytes, while ServerInit begins with a screen width and height, which
# are never zero on a real console.
Send-Bytes -Stream $stream -Bytes ([byte[]]@(1))   # shared desktop

$first = Read-Exactly -Stream $stream -Count 4
if ($first[0] -eq 0 -and $first[1] -eq 0 -and $first[2] -eq 0 -and $first[3] -eq 0) {
  Write-Output "  security result: OK"
  $header = Read-Exactly -Stream $stream -Count 24
  $width = [BitConverter]::ToUInt16($header, 0)
  $height = [BitConverter]::ToUInt16($header, 2)
  $nameLength = [BitConverter]::ToUInt32($header, 20)
} else {
  Write-Output "  no security result (VMware skips it for type None)"
  $width = [BitConverter]::ToUInt16($first, 0)
  $height = [BitConverter]::ToUInt16($first, 2)
  $rest = Read-Exactly -Stream $stream -Count 20
  $nameLength = [BitConverter]::ToUInt32($rest, 16)
}
$name = [Text.Encoding]::ASCII.GetString((Read-Exactly -Stream $stream -Count $nameLength))
Write-Output "  desktop ${width}x${height}: $name"

if ($width -eq 0 -or $height -eq 0) {
  throw "handshake went wrong: the server reported a ${width}x${height} desktop"
}

# --- press the key repeatedly ---
# Repeated on purpose: the exact moment the boot loader starts waiting is not
# knowable from outside, and a single press that arrives a second early is lost.
# The message is harmless once setup is already running.
Write-Output "  pressing '$Key' every 200ms for $Seconds seconds"
$deadline = (Get-Date).AddSeconds($Seconds)
$presses = 0
while ((Get-Date) -lt $deadline) {
  try {
    Send-Key -Stream $stream -Sym $keysym
    $presses++
  } catch {
    Write-Output "  send failed after $presses presses: $($_.Exception.Message)"
    break
  }
  Start-Sleep -Milliseconds 200
}

$client.Close()
Write-Output "  sent $presses presses"
