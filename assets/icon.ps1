# Draws a dterm icon: dark text on a green rounded square, saved as a multi-size .ico (and the
# 256 px frame as icon.png for the README). The green tile stands out on dark and light taskbars.
#   powershell -File assets\icon.ps1 -Text "D_" -Out icon.ico          (the current icon)
#   powershell -File assets\icon.ps1 -Text ">_" -Out icon-prompt.ico   (the alternative)
# Files are written next to this script. Then point dterm.rc at the .ico you want and rebuild
# the resource from the project root:  rc.exe /fo dterm.res dterm.rc
param([string]$Text = "D_", [string]$Out = "icon.ico")
Add-Type -AssemblyName System.Drawing
# Every size Windows asks for at 100-200% scaling (taskbar 24 / 32 / 48, title bar 16 / 20 / 32,
# Explorer up to 256), so none has to be resized and blurred.
$frames = foreach ($s in 256, 64, 48, 40, 32, 24, 20, 16) {
    $bm = New-Object System.Drawing.Bitmap $s, $s
    $g = [System.Drawing.Graphics]::FromImage($bm)
    $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'
    $r = [math]::Max(2, $s * 0.2)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, 2 * $r, 2 * $r, 180, 90)
    $path.AddArc($s - 2 * $r - 1, 0, 2 * $r, 2 * $r, 270, 90)
    $path.AddArc($s - 2 * $r - 1, $s - 2 * $r - 1, 2 * $r, 2 * $r, 0, 90)
    $path.AddArc(0, $s - 2 * $r - 1, 2 * $r, 2 * $r, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x98, 0xC3, 0x79))), $path)  # One Half Dark green
    # The text is an outline path, thickened by a stroke and fitted by its ink bounds into a square
    # with the same margin on every side (so it is centered by what you see, not by the font box).
    $glyphs = New-Object System.Drawing.Drawing2D.GraphicsPath
    $glyphs.AddString($Text, (New-Object System.Drawing.FontFamily 'Cascadia Mono'), [int][System.Drawing.FontStyle]::Bold, 100, (New-Object System.Drawing.PointF 0, 0), [System.Drawing.StringFormat]::GenericTypographic)
    $pen = New-Object System.Drawing.Pen ([System.Drawing.Color]::Black), 7
    $pen.LineJoin = 'Round'
    $b = $glyphs.GetBounds($null, $pen)
    $m = $s * 0.22; $box = $s - 2 * $m
    $fit = New-Object System.Drawing.Drawing2D.Matrix
    $fit.Translate($m, $m)
    $fit.Scale($box / $b.Width, $box / $b.Height)
    $fit.Translate(-$b.X, -$b.Y)
    $g.Transform = $fit
    $dark = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x1B, 0x1D, 0x23))  # One Half Dark gap color
    $pen.Color = $dark.Color
    $g.FillPath($dark, $glyphs); $g.DrawPath($pen, $glyphs)
    $g.ResetTransform()
    if ($s -eq 256 -and $Out -eq 'icon.ico') { $bm.Save((Join-Path $PSScriptRoot 'icon.png'), [System.Drawing.Imaging.ImageFormat]::Png) }
    $ms = New-Object System.IO.MemoryStream
    $bm.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    , @($s, $ms.ToArray())
}
# ICO = header + one directory entry per size + the PNG images.
$ico = New-Object System.IO.MemoryStream
$w = New-Object System.IO.BinaryWriter $ico
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$frames.Count)
$offset = 6 + 16 * $frames.Count
foreach ($f in $frames) {
    $s = $f[0]; $png = $f[1]
    $w.Write([byte]($s % 256)); $w.Write([byte]($s % 256)); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32); $w.Write([uint32]$png.Length); $w.Write([uint32]$offset)
    $offset += $png.Length
}
foreach ($f in $frames) { $w.Write($f[1]) }
[System.IO.File]::WriteAllBytes((Join-Path $PSScriptRoot $Out), $ico.ToArray())
