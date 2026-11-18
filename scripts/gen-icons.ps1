[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'app\src-tauri\icons')
)
# Draws the app icon (teal ring on a dark background) with System.Drawing: no extra dependencies.
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

function New-IconPng([int]$Size) {
    $bitmap = New-Object System.Drawing.Bitmap $Size, $Size, ([System.Drawing.Imaging.PixelFormat]::Format32bppArgb)
    $graphics = [System.Drawing.Graphics]::FromImage($bitmap)
    $graphics.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $graphics.Clear([System.Drawing.Color]::Transparent)
    $background = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 8, 12, 16))
    $radius = [int]($Size * 0.22)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $d = $radius * 2
    $path.AddArc(0, 0, $d, $d, 180, 90)
    $path.AddArc($Size - $d - 1, 0, $d, $d, 270, 90)
    $path.AddArc($Size - $d - 1, $Size - $d - 1, $d, $d, 0, 90)
    $path.AddArc(0, $Size - $d - 1, $d, $d, 90, 90)
    $path.CloseFigure()
    $graphics.FillPath($background, $path)
    $stroke = [Math]::Max(2.0, $Size * 0.09)
    $inset = $Size * 0.22
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(255, 25, 211, 218)), $stroke
    $graphics.DrawEllipse($pen, $inset, $inset, $Size - 2 * $inset, $Size - 2 * $inset)
    $dot = $Size * 0.12
    $accent = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(255, 58, 160, 255))
    $graphics.FillEllipse($accent, ($Size - $dot) / 2, ($Size - $dot) / 2, $dot, $dot)
    $graphics.Dispose()
    $stream = New-Object System.IO.MemoryStream
    $bitmap.Save($stream, [System.Drawing.Imaging.ImageFormat]::Png)
    $bitmap.Dispose()
    return , $stream.ToArray()
}

$sizes = @{ '32x32.png' = 32; '128x128.png' = 128; '128x128@2x.png' = 256; 'icon.png' = 512 }
foreach ($name in $sizes.Keys) {
    [System.IO.File]::WriteAllBytes((Join-Path $OutDir $name), (New-IconPng $sizes[$name]))
}

# icon.ico: PNG-compressed entries (supported since Windows Vista).
$icoSizes = @(16, 32, 48, 64, 128, 256)
$images = foreach ($size in $icoSizes) { , (New-IconPng $size) }
$stream = New-Object System.IO.MemoryStream
$writer = New-Object System.IO.BinaryWriter $stream
$writer.Write([uint16]0); $writer.Write([uint16]1); $writer.Write([uint16]$icoSizes.Count)
$offset = 6 + 16 * $icoSizes.Count
for ($i = 0; $i -lt $icoSizes.Count; $i++) {
    $size = $icoSizes[$i]
    $dimension = if ($size -ge 256) { 0 } else { $size }
    $writer.Write([byte]$dimension); $writer.Write([byte]$dimension)
    $writer.Write([byte]0); $writer.Write([byte]0)
    $writer.Write([uint16]1); $writer.Write([uint16]32)
    $writer.Write([uint32]$images[$i].Length); $writer.Write([uint32]$offset)
    $offset += $images[$i].Length
}
foreach ($image in $images) { $writer.Write([byte[]]$image) }
$writer.Flush()
[System.IO.File]::WriteAllBytes((Join-Path $OutDir 'icon.ico'), $stream.ToArray())
Write-Host "Icons written to $OutDir"
