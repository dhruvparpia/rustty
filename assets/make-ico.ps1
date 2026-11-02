# Build a multi-size ICO (PNG-compressed entries, Vista+) from the crab PNG.
Add-Type -AssemblyName System.Drawing
$src = "C:\Users\dhruv\OneDrive\Projects\winttyghost\assets\rustty-icon.png"
$dst = "C:\Users\dhruv\OneDrive\Projects\winttyghost\assets\rustty-icon.ico"
$bmp = [System.Drawing.Bitmap]::FromFile($src)
# The Mac icon bakes the art into 80% of the canvas (Apple grid). Windows
# icons fill the canvas, so crop to the art's alpha bounds.
$minX = $bmp.Width; $maxX = -1; $minY = $bmp.Height; $maxY = -1
for ($y = 0; $y -lt $bmp.Height; $y += 2) {
    for ($x = 0; $x -lt $bmp.Width; $x += 2) {
        if ($bmp.GetPixel($x, $y).A -gt 8) {
            if ($x -lt $minX) { $minX = $x }; if ($x -gt $maxX) { $maxX = $x }
            if ($y -lt $minY) { $minY = $y }; if ($y -gt $maxY) { $maxY = $y }
        }
    }
}
$aw = $maxX - $minX + 1; $ah = $maxY - $minY + 1
"art bounds: x=$minX..$maxX y=$minY..$maxY ($aw x $ah)"
$sizes = 16, 24, 32, 48, 64, 128, 256
$pngs = @()
foreach ($n in $sizes) {
    $b = New-Object System.Drawing.Bitmap $n, $n
    $g = [System.Drawing.Graphics]::FromImage($b)
    $g.InterpolationMode = [System.Drawing.Drawing2D.InterpolationMode]::HighQualityBicubic
    $g.PixelOffsetMode = [System.Drawing.Drawing2D.PixelOffsetMode]::HighQuality
    # draw the art centred, aspect preserved, long edge full-bleed
    $side = [Math]::Max($aw, $ah)
    $pad = $side
    $dw = [int]($n * $aw / $pad); $dh = [int]($n * $ah / $pad)
    $dest = New-Object System.Drawing.Rectangle ([int](($n - $dw) / 2)), ([int](($n - $dh) / 2)), $dw, $dh
    $g.DrawImage($bmp, $dest, $minX, $minY, $aw, $ah, [System.Drawing.GraphicsUnit]::Pixel)
    $g.Dispose()
    $ms = New-Object System.IO.MemoryStream
    $b.Save($ms, [System.Drawing.Imaging.ImageFormat]::Png)
    $b.Dispose()
    $pngs += , $ms.ToArray()
    $ms.Dispose()
}
$bmp.Dispose()
$fs = [System.IO.File]::Create($dst)
$w = New-Object System.IO.BinaryWriter $fs
$w.Write([uint16]0); $w.Write([uint16]1); $w.Write([uint16]$sizes.Count)
$off = 6 + 16 * $sizes.Count
for ($i = 0; $i -lt $sizes.Count; $i++) {
    $n = $sizes[$i]
    if ($n -ge 256) { $b1 = 0 } else { $b1 = $n }
    if ($n -ge 256) { $b2 = 0 } else { $b2 = $n }
    $w.Write([byte]$b1); $w.Write([byte]$b2); $w.Write([byte]0); $w.Write([byte]0)
    $w.Write([uint16]1); $w.Write([uint16]32)
    $w.Write([uint32]$pngs[$i].Length); $w.Write([uint32]$off)
    $off += $pngs[$i].Length
}
foreach ($p in $pngs) { $w.Write($p) }
$w.Dispose(); $fs.Dispose()
"ICO written: $dst $((Get-Item $dst).Length) bytes"
