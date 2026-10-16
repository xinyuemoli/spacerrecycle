# Reads the text off a screenshot with the OCR engine built into Windows.
#
# The point is to find out what a guest is showing without a person looking at
# it. A screenshot can be captured from the host at any time, but its contents
# are only useful to someone who can see it, which an automated check cannot.
# Colour histograms narrow it down - a screen that is 37% uniform blue is either
# the lock screen or a recovery screen and those need different responses - but
# they cannot tell the two apart. The text can.
#
# WinRT is called through the AsTask shim because PowerShell 5.1 has no await.

param(
  [Parameter(Mandatory = $true)][string]$Image
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.Runtime.WindowsRuntime | Out-Null

$asTaskGeneric = ([System.WindowsRuntimeSystemExtensions].GetMethods() |
  Where-Object {
    $_.Name -eq 'AsTask' -and
    $_.GetParameters().Count -eq 1 -and
    $_.GetParameters()[0].ParameterType.Name -eq 'IAsyncOperation`1'
  })[0]

function Await {
  param($Operation, [Type]$ResultType)
  $task = $asTaskGeneric.MakeGenericMethod($ResultType).Invoke($null, @($Operation))
  $task.Wait(-1) | Out-Null
  $task.Result
}

[Windows.Storage.StorageFile, Windows.Storage, ContentType = WindowsRuntime] | Out-Null
[Windows.Graphics.Imaging.BitmapDecoder, Windows.Graphics.Imaging, ContentType = WindowsRuntime] | Out-Null
[Windows.Media.Ocr.OcrEngine, Windows.Foundation, ContentType = WindowsRuntime] | Out-Null

$engine = [Windows.Media.Ocr.OcrEngine]::TryCreateFromUserProfileLanguages()
if ($null -eq $engine) {
  Write-Output "no OCR engine for the current languages; available:"
  [Windows.Media.Ocr.OcrEngine]::AvailableRecognizerLanguages | ForEach-Object { "  $($_.LanguageTag)" }
  exit 1
}

$full = (Resolve-Path $Image).Path
$file = Await ([Windows.Storage.StorageFile]::GetFileFromPathAsync($full)) ([Windows.Storage.StorageFile])
$stream = Await ($file.OpenAsync([Windows.Storage.FileAccessMode]::Read)) ([Windows.Storage.Streams.IRandomAccessStream])
$decoder = Await ([Windows.Graphics.Imaging.BitmapDecoder]::CreateAsync($stream)) ([Windows.Graphics.Imaging.BitmapDecoder])
$bitmap = Await ($decoder.GetSoftwareBitmapAsync()) ([Windows.Graphics.Imaging.SoftwareBitmap])

$result = Await ($engine.RecognizeAsync($bitmap)) ([Windows.Media.Ocr.OcrResult])

Write-Output "=== OCR: $Image ==="
Write-Output "engine language: $($engine.RecognizerLanguage.LanguageTag)"
Write-Output "lines: $($result.Lines.Count)"
Write-Output "---"
foreach ($line in $result.Lines) { Write-Output "  $($line.Text)" }
