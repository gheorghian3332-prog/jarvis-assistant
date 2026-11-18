# Draws the installer artwork with System.Drawing (no external assets, no third-party logos):
#   header.bmp   150x57   - shown in the header of the installer pages
#   sidebar.bmp  164x314  - shown on the welcome and finish pages
# HUD style of the app: background #080c10, teal ring #19d3da, blue accent #3aa0ff, a small neural network.
# Output: app\src-tauri\windows\ (BMP, 24 bit, as NSIS requires). The result is committed; run this script to regenerate.
[CmdletBinding()]
param(
    [string]$OutDir = (Join-Path (Split-Path -Parent $PSScriptRoot) 'app\src-tauri\windows')
)
$ErrorActionPreference = 'Stop'
Add-Type -AssemblyName System.Drawing
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$teal = [System.Drawing.Color]::FromArgb(255, 25, 211, 218)
$blue = [System.Drawing.Color]::FromArgb(255, 58, 160, 255)
$dark = [System.Drawing.Color]::FromArgb(255, 8, 12, 16)
$text = [System.Drawing.Color]::FromArgb(255, 220, 240, 244)
$dim = [System.Drawing.Color]::FromArgb(255, 110, 140, 150)

function New-Canvas([int]$W, [int]$H) {
    $bmp = New-Object System.Drawing.Bitmap $W, $H, ([System.Drawing.Imaging.PixelFormat]::Format24bppRgb)
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.TextRenderingHint = [System.Drawing.Text.TextRenderingHint]::AntiAliasGridFit
    $g.Clear($dark)
    return @{ Bitmap = $bmp; Graphics = $g }
}

function Add-Glow($g, [single]$cx, [single]$cy, [single]$radius, $color, [int]$alpha) {
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddEllipse($cx - $radius, $cy - $radius, 2 * $radius, 2 * $radius)
    $brush = New-Object System.Drawing.Drawing2D.PathGradientBrush $path
    $brush.CenterColor = [System.Drawing.Color]::FromArgb($alpha, $color)
    $brush.SurroundColors = @([System.Drawing.Color]::FromArgb(0, $color))
    $g.FillPath($brush, $path)
    $brush.Dispose(); $path.Dispose()
}

function Add-Ring($g, [single]$cx, [single]$cy, [single]$r, [single]$stroke) {
    Add-Glow $g $cx $cy ($r * 1.9) $teal 60
    $pen = New-Object System.Drawing.Pen $teal, $stroke
    $g.DrawEllipse($pen, $cx - $r, $cy - $r, 2 * $r, 2 * $r)
    $pen.Dispose()
    $thin = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(110, $teal)), 1
    $r2 = $r * 1.28
    $g.DrawArc($thin, $cx - $r2, $cy - $r2, 2 * $r2, 2 * $r2, 200, 120)
    $g.DrawArc($thin, $cx - $r2, $cy - $r2, 2 * $r2, 2 * $r2, 20, 120)
    $thin.Dispose()
    $dot = $r * 0.34
    $brush = New-Object System.Drawing.SolidBrush $blue
    $g.FillEllipse($brush, $cx - $dot / 2, $cy - $dot / 2, $dot, $dot)
    $brush.Dispose()
}

function Add-Network($g, [int]$W, [int]$H, [int]$Count, [int]$Seed, [int]$Link) {
    $rng = New-Object System.Random $Seed
    $pts = @()
    for ($i = 0; $i -lt $Count; $i++) { $pts += , @(($rng.NextDouble() * $W), ($rng.NextDouble() * $H)) }
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::FromArgb(55, $teal)), 1
    for ($i = 0; $i -lt $Count; $i++) {
        for ($j = $i + 1; $j -lt $Count; $j++) {
            $dx = $pts[$i][0] - $pts[$j][0]; $dy = $pts[$i][1] - $pts[$j][1]
            if (($dx * $dx + $dy * $dy) -lt ($Link * $Link)) { $g.DrawLine($pen, [single]$pts[$i][0], [single]$pts[$i][1], [single]$pts[$j][0], [single]$pts[$j][1]) }
        }
    }
    $pen.Dispose()
    $brush = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(150, $teal))
    foreach ($p in $pts) { $g.FillEllipse($brush, [single]($p[0] - 1.5), [single]($p[1] - 1.5), 3, 3) }
    $brush.Dispose()
}

function Add-Brackets($g, [int]$W, [int]$H, [int]$Inset, [int]$Len) {
    $pen = New-Object System.Drawing.Pen $teal, 1.5
    $x0 = $Inset; $y0 = $Inset; $x1 = $W - $Inset - 1; $y1 = $H - $Inset - 1
    $g.DrawLines($pen, @((New-Object System.Drawing.Point $x0, ($y0 + $Len)), (New-Object System.Drawing.Point $x0, $y0), (New-Object System.Drawing.Point ($x0 + $Len), $y0)))
    $g.DrawLines($pen, @((New-Object System.Drawing.Point ($x1 - $Len), $y1), (New-Object System.Drawing.Point $x1, $y1), (New-Object System.Drawing.Point $x1, ($y1 - $Len))))
    $pen.Dispose()
}

function Save-Bmp($canvas, [string]$Name) {
    $canvas.Graphics.Dispose()
    $path = Join-Path $OutDir $Name
    $canvas.Bitmap.Save($path, [System.Drawing.Imaging.ImageFormat]::Bmp)
    $canvas.Bitmap.Dispose()
    Write-Host "wrote $path"
}

# --- header 150x57: ring on the left, the name on the right ---
$c = New-Canvas 150 57
$g = $c.Graphics
Add-Network $g 150 57 9 7 55
Add-Ring $g 26 28.5 14 3
Add-Brackets $g 150 57 3 8
$font = New-Object System.Drawing.Font 'Segoe UI Semibold', 11.5, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$sub = New-Object System.Drawing.Font 'Consolas', 8, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$g.DrawString('J.A.R.V.I.S.', $font, (New-Object System.Drawing.SolidBrush $text), 49, 15)
$g.DrawString('VOICE ASSISTANT', $sub, (New-Object System.Drawing.SolidBrush $teal), 50, 33)
Save-Bmp $c 'installer-header.bmp'

# --- sidebar 164x314: network, big ring, name at the bottom ---
$c = New-Canvas 164 314
$g = $c.Graphics
Add-Network $g 164 314 34 11 60
Add-Ring $g 82 112 36 5
Add-Brackets $g 164 314 6 14
$title = New-Object System.Drawing.Font 'Segoe UI Semibold', 22, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$small = New-Object System.Drawing.Font 'Consolas', 10, ([System.Drawing.FontStyle]::Regular), ([System.Drawing.GraphicsUnit]::Pixel)
$center = New-Object System.Drawing.StringFormat
$center.Alignment = [System.Drawing.StringAlignment]::Center
$g.DrawString('J.A.R.V.I.S.', $title, (New-Object System.Drawing.SolidBrush $text), (New-Object System.Drawing.RectangleF 0, 196, 164, 30), $center)
$g.DrawString('LOCAL VOICE ASSISTANT', $small, (New-Object System.Drawing.SolidBrush $teal), (New-Object System.Drawing.RectangleF 0, 228, 164, 16), $center)
$g.DrawString('private  |  offline  |  Windows', $small, (New-Object System.Drawing.SolidBrush $dim), (New-Object System.Drawing.RectangleF 0, 280, 164, 16), $center)
Save-Bmp $c 'installer-sidebar.bmp'
