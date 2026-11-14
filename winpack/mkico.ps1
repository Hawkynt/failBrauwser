#Requires -Version 5.1
# Renders the application icon from its svg into data\icons\failbrauwser.ico — the sizes the
# Windows shell asks for, 16 through 256. Small sizes ship as 32-bit bitmaps so every shell
# reads them without a decoder; 256 ships as PNG, the only way that size stays small. The
# result is linked into both program files by build.rs (see docs\WINDOWS.md).
#   winpack\mkico.ps1                    # regenerate the icon
#   winpack\mkico.ps1 -Sizes 16,32,256   # a subset, while trying a change over
param([int[]]$Sizes = @(16, 24, 32, 48, 64, 128, 256))
$ErrorActionPreference = 'Stop'
$repo = Split-Path -Parent $PSScriptRoot
$svg = Join-Path $repo 'data\icons\org.failbrauwser.FailBrauwser.svg'
$ico = Join-Path $repo 'data\icons\failbrauwser.ico'
$rsvg = Join-Path $PSScriptRoot 'sysroot\ucrt64\bin\rsvg-convert.exe'
if (-not (Test-Path -LiteralPath $svg)) { throw "no icon painting at $svg" }
if (-not (Test-Path -LiteralPath $rsvg)) { throw 'rsvg-convert is missing - run winpack\bootstrap.ps1 first' }

# The icon container is small enough to write directly: ICONDIR, one record per picture, then
# the pictures. A 32-bit picture carries BGRA bottom-up under a header whose height counts the
# (fully transparent) AND mask twice over; a 256 picture is simply the PNG bytes.
Add-Type -AssemblyName System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Collections.Generic;
using System.Drawing;
using System.Drawing.Imaging;
using System.IO;
using System.Runtime.InteropServices;

public static class IconPack {
  public static byte[] Build(int[] sizes, string[] pngs) {
    var parts = new List<byte[]>();
    for (int i = 0; i < sizes.Length; i++) parts.Add(Picture(sizes[i], pngs[i]));
    int directory = 6 + 16 * sizes.Length, total = directory, at = directory;
    for (int i = 0; i < parts.Count; i++) total += parts[i].Length;
    var all = new byte[total];
    all[2] = 1; all[4] = (byte)sizes.Length; all[5] = 0;
    for (int i = 0; i < sizes.Length; i++) {
      int e = 6 + 16 * i, side = sizes[i];
      all[e] = (byte)(side & 0xff); all[e + 1] = (byte)(side & 0xff);   // 0 stands for 256
      all[e + 10] = 1; all[e + 12] = 32;
      Write(all, e + 8, parts[i].Length); Write(all, e + 12, at); at += parts[i].Length;
    }
    for (int i = 0, o = directory; i < parts.Count; i++) { Array.Copy(parts[i], 0, all, o, parts[i].Length); o += parts[i].Length; }
    return all;
  }

  private static byte[] Picture(int side, string png) {
    if (side >= 256) return File.ReadAllBytes(png);
    using (var bmp = new Bitmap(png)) {
      var area = new Rectangle(0, 0, bmp.Width, bmp.Height);
      var data = bmp.LockBits(area, ImageLockMode.ReadOnly, PixelFormat.Format32bppArgb);
      var rows = new byte[data.Stride * bmp.Height];
      Marshal.Copy(data.Scan0, rows, 0, rows.Length);
      bmp.UnlockBits(data);
      int maskRow = ((side + 31) / 32) * 4;
      var dib = new byte[40 + side * side * 4 + maskRow * side];
      Write(dib, 0, 40); Write(dib, 4, side); Write(dib, 8, side * 2);
      dib[12] = 1; dib[14] = 32;
      for (int y = 0; y < side; y++) Array.Copy(rows, (side - 1 - y) * data.Stride, dib, 40 + y * side * 4, side * 4);
      return dib;
    }
  }

  private static void Write(byte[] at, int offset, int value) {
    at[offset] = (byte)value; at[offset + 1] = (byte)(value >> 8); at[offset + 2] = (byte)(value >> 16); at[offset + 3] = (byte)(value >> 24);
  }
}
'@ -ReferencedAssemblies 'System.Drawing.dll'

$staged = Join-Path $env:TEMP ('fb-ico-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $staged | Out-Null
try {
    $pngs = foreach ($s in $Sizes) {
        $png = Join-Path $staged "$s.png"
        & $rsvg '-w' $s '-h' $s '-o' $png $svg
        if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $png)) { throw "rsvg-convert produced nothing at $s px" }
        $png
    }
    $body = [IconPack]::Build($Sizes, $pngs)
    [IO.File]::WriteAllBytes($ico, $body)
    Write-Output "ICON: $ico ($([math]::Round($body.Length / 1KB, 1)) KB, $($Sizes -join ', ') px)"
}
finally { Remove-Item $staged -Recurse -Force -ErrorAction SilentlyContinue }
