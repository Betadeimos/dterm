# Draws a dterm icon: green text on a dark rounded square, saved as a multi-size .ico.
#   powershell -File icon.ps1 -Text ">_" -Out dterm.ico
#   powershell -File icon.ps1 -Text "D_" -Out dterm-d.ico
# Then point dterm.rc at the .ico you want and rebuild the resource:  rc.exe /fo dterm.res dterm.rc
param([string]$Text = ">_", [string]$Out = "dterm.ico")
Add-Type -AssemblyName System.Drawing
$frames = foreach ($s in 256, 48, 32, 16) {
    $bm = New-Object System.Drawing.Bitmap $s, $s
    $g = [System.Drawing.Graphics]::FromImage($bm)
    $g.SmoothingMode = 'AntiAlias'; $g.TextRenderingHint = 'AntiAliasGridFit'
    $r = [math]::Max(2, $s * 0.18)
    $path = New-Object System.Drawing.Drawing2D.GraphicsPath
    $path.AddArc(0, 0, 2 * $r, 2 * $r, 180, 90)
    $path.AddArc($s - 2 * $r - 1, 0, 2 * $r, 2 * $r, 270, 90)
    $path.AddArc($s - 2 * $r - 1, $s - 2 * $r - 1, 2 * $r, 2 * $r, 0, 90)
    $path.AddArc(0, $s - 2 * $r - 1, 2 * $r, 2 * $r, 90, 90)
    $path.CloseFigure()
    $g.FillPath((New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x28, 0x2C, 0x34))), $path)  # One Half Dark background
    $font = New-Object System.Drawing.Font 'Cascadia Mono', ($s * 0.42), ([System.Drawing.FontStyle]::Bold), ([System.Drawing.GraphicsUnit]::Pixel)
    $fmt = New-Object System.Drawing.StringFormat; $fmt.Alignment = 'Center'; $fmt.LineAlignment = 'Center'
    $green = New-Object System.Drawing.SolidBrush ([System.Drawing.Color]::FromArgb(0x98, 0xC3, 0x79))  # One Half Dark green
    $g.DrawString($Text, $font, $green, (New-Object System.Drawing.RectangleF 0, 0, $s, $s), $fmt)
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
