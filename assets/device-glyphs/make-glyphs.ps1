# The nine device shapes the format window and the device-icon verbs draw with (README: *Device icons*).
#
# This is the authorship record of the tracked PNG masters next to it: every shape is drawn here out of
# plain GDI+ primitives, on a transparent 256x256 canvas, in the flat three-tone scheme the shapes are
# recognised by. Nothing is copied from anywhere: a device icon of another program is its own work, and the
# shapes below are drawn from what the device looks like. The program never runs this file — it reads the
# PNGs it produces (`include_bytes!` in src\fsformat\art.rs), and the manufacturer and the size are
# composited when an icon is written, not here.
#
#   powershell -NoProfile -ExecutionPolicy Bypass -File assets\device-glyphs\make-glyphs.ps1
#
# Holes (a platter spindle, a disc's centre, a floppy's hub) are cut by painting transparent pixels in
# SourceCopy mode, so the icon stays transparent where the device has a hole.

Add-Type -AssemblyName System.Drawing

$S = 256
$Body = [System.Drawing.Color]::FromArgb(255, 47, 58, 69)
$Panel = [System.Drawing.Color]::FromArgb(255, 92, 107, 122)
$Edge = [System.Drawing.Color]::FromArgb(255, 215, 222, 229)
$Gold = [System.Drawing.Color]::FromArgb(255, 201, 162, 39)

function New-Bitmap {
    $bmp = New-Object System.Drawing.Bitmap $S, $S
    $g = [System.Drawing.Graphics]::FromImage($bmp)
    $g.SmoothingMode = [System.Drawing.Drawing2D.SmoothingMode]::AntiAlias
    $g.Clear([System.Drawing.Color]::Transparent)
    return @($bmp, $g)
}

# Paints fully transparent pixels, which is how a shape's hole becomes a hole.
function Cut-Hole($g, $brush) {
    $mode = $g.CompositingMode
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillEllipse([System.Drawing.Brushes]::Transparent, $brush.X, $brush.Y, $brush.Width, $brush.Height)
    $g.CompositingMode = $mode
}

function Save-Glyph($name, $draw) {
    $bmp, $g = New-Bitmap
    & $draw $g
    $path = Join-Path $PSScriptRoot "$name.png"
    $bmp.Save($path, [System.Drawing.Imaging.ImageFormat]::Png)
    $g.Dispose()
    $bmp.Dispose()
    Write-Output "wrote $path"
}

$rect = { param($x, $y, $w, $h) New-Object System.Drawing.RectangleF($x, $y, $w, $h) }
$round = {
    param($g, $x, $y, $w, $h, $r, $colour)
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddArc($x, $y, $r * 2, $r * 2, 180, 90)
    $p.AddArc($x + $w - $r * 2, $y, $r * 2, $r * 2, 270, 90)
    $p.AddArc($x + $w - $r * 2, $y + $h - $r * 2, $r * 2, $r * 2, 0, 90)
    $p.AddArc($x, $y + $h - $r * 2, $r * 2, $r * 2, 90, 90)
    $p.CloseFigure()
    $b = New-Object System.Drawing.SolidBrush($colour)
    $g.FillPath($b, $p)
    $b.Dispose()
    $p.Dispose()
}
$fill = { param($g, $colour, $x, $y, $w, $h) $b = New-Object System.Drawing.SolidBrush($colour); $g.FillRectangle($b, (& $rect $x $y $w $h)); $b.Dispose() }
$pen = { param($g, $colour, $w) New-Object System.Drawing.Pen($colour, $w) }

Save-Glyph 'nvme' {
    param($g)
    # An M.2 stick: a long body, a connector edge with its key notch, chips on the face.
    & $round $g 28 96 200 64 10 $Body
    & $fill $g $Edge 18 104 14 48
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillRectangle([System.Drawing.Brushes]::Transparent, (& $rect 24 118 8 20))
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
    & $fill $g $Panel 60 108 56 40
    & $fill $g $Panel 128 108 56 40
    & $fill $g $Gold 202 104 14 48
}

Save-Glyph 'ssd' {
    param($g)
    # A 2.5" case: a flat box, four corner screws, a label band across it.
    & $round $g 40 56 176 144 14 $Body
    & $fill $g $Panel 60 76 136 60
    foreach ($x in 56, 192) { foreach ($y in 72, 176) {
        $b = New-Object System.Drawing.SolidBrush($Edge); $g.FillEllipse($b, $x, $y, 12, 12); $b.Dispose()
    } }
    & $fill $g $Gold 152 156 48 12
    & $fill $g $Panel 64 156 64 12
}

Save-Glyph 'hdd' {
    param($g)
    # A hard disk: the case, one platter as a ring, its spindle and the actuator arm across it.
    & $round $g 40 48 176 160 14 $Body
    $ring = New-Object System.Drawing.SolidBrush($Panel)
    $g.FillEllipse($ring, 60, 64, 128, 128)
    $ring.Dispose()
    Cut-Hole $g (& $rect 110 114 28 28)
    $b = New-Object System.Drawing.SolidBrush($Edge); $g.FillEllipse($b, 116, 120, 16, 16); $b.Dispose()
    $p = & $pen $g $Edge 14
    $g.DrawLine($p, 188, 76, 132, 152)
    $p.Dispose()
    & $fill $g $Gold 196 176 24 12
}

Save-Glyph 'usbstick' {
    param($g)
    # A stick: the body, its cap seam, and the metal shell with the plastic tongue inside it.
    & $round $g 88 88 80 132 16 $Body
    & $fill $g $Panel 100 156 56 48
    & $round $g 96 28 64 60 8 $Edge
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillRectangle([System.Drawing.Brushes]::Transparent, (& $rect 108 40 40 32))
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
    & $fill $g $Panel 112 44 32 16
}

Save-Glyph 'sdcard' {
    param($g)
    # An SD card: a tall card with the write-protect chamfer at one corner and contact teeth at the far end.
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddLines([System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF(64, 40)), (New-Object System.Drawing.PointF(192, 40)),
        (New-Object System.Drawing.PointF(192, 216)), (New-Object System.Drawing.PointF(64, 216)),
        (New-Object System.Drawing.PointF(64, 96)), (New-Object System.Drawing.PointF(120, 40))))
    $p.CloseFigure()
    $b = New-Object System.Drawing.SolidBrush($Body); $g.FillPath($b, $p); $b.Dispose()
    & $fill $g $Panel 76 52 24 28
    for ($i = 0; $i -lt 6; $i++) { & $fill $g $Gold (76 + $i * 20) 180 12 28 }
    & $fill $g $Edge 76 96 100 68
}

Save-Glyph 'msd' {
    param($g)
    # A µSD card: the same silhouette with the chamfer on the other side and smaller contacts, so the two
    # are told apart by eye while the machine cannot tell them at all.
    $p = New-Object System.Drawing.Drawing2D.GraphicsPath
    $p.AddLines([System.Drawing.PointF[]]@(
        (New-Object System.Drawing.PointF(64, 56)), (New-Object System.Drawing.PointF(192, 56)),
        (New-Object System.Drawing.PointF(192, 200)), (New-Object System.Drawing.PointF(64, 200)),
        (New-Object System.Drawing.PointF(64, 120)), (New-Object System.Drawing.PointF(128, 56))))
    $p.CloseFigure()
    $b = New-Object System.Drawing.SolidBrush($Body); $g.FillPath($b, $p); $b.Dispose()
    for ($i = 0; $i -lt 7; $i++) { & $fill $g $Gold (72 + $i * 17) 172 10 24 }
    & $fill $g $Panel 84 100 92 56
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillRectangle([System.Drawing.Brushes]::Transparent, (& $rect 186 84 6 16))
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
}

Save-Glyph 'floppy35' {
    param($g)
    # A 3.5" floppy: the hard shell, the metal slider across the top and the label below it.
    & $round $g 48 40 160 176 10 $Body
    & $fill $g $Edge 72 52 96 40
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillRectangle([System.Drawing.Brushes]::Transparent, (& $rect 120 60 28 24))
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
    & $fill $g $Panel 68 108 120 88
    & $fill $g $Gold 176 52 16 40
}

Save-Glyph 'floppy525' {
    param($g)
    # A 5.25" floppy: the flexible sleeve, its label ring and the big hub hole in the middle.
    & $round $g 40 32 176 192 8 $Body
    & $fill $g $Panel 64 56 128 76
    Cut-Hole $g (& $rect 104 136 48 48)
    $p = & $pen $g $Edge 8
    $g.DrawEllipse($p, 100, 132, 56, 56)
    $p.Dispose()
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceCopy
    $g.FillRectangle([System.Drawing.Brushes]::Transparent, (& $rect 120 32 16 24))
    $g.CompositingMode = [System.Drawing.Drawing2D.CompositingMode]::SourceOver
}

Save-Glyph 'optical' {
    param($g)
    # An optical disc: the ring, the inner clear band and the centre hole.
    $b = New-Object System.Drawing.SolidBrush($Panel)
    $g.FillEllipse($b, 32, 32, 192, 192)
    $b.Dispose()
    Cut-Hole $g (& $rect 104 104 48 48)
    $p = & $pen $g $Edge 6
    $g.DrawEllipse($p, 88, 88, 80, 80)
    $p.Dispose()
    $b = New-Object System.Drawing.SolidBrush($Gold); $g.FillRectangle($b, 118, 44, 8, 24); $b.Dispose()
}
