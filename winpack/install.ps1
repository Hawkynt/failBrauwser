#Requires -Version 5.1
# Installs a packaged failBrauwser (the zip from package.ps1, or its unpacked folder)
# for the current user: the files into the prefix (default %LOCALAPPDATA%\Programs\
# failbrauwser, laid out bin/lib/share), a fresh gdk-pixbuf loader cache — the shipped
# one names the building machine's folders — and a Start Menu entry. This is also the
# manual update route the updater window points to for machine-wide installs.
#   winpack\install.ps1                                    # newest zip in winpack\out
#   winpack\install.ps1 -Package winpack\out\failbrauwser-0.6.0-x86_64-windows.zip
#   winpack\install.ps1 -Prefix D:\Apps\failbrauwser
# (Uninstall is plain: delete the prefix folder and the Start Menu entry.)
param(
    [string]$Package,
    [string]$Prefix = (Join-Path $env:LOCALAPPDATA 'Programs\failbrauwser')
)
$ErrorActionPreference = 'Stop'
$out = Join-Path $PSScriptRoot 'out'
if (-not $Package) {
    $latest = Get-ChildItem $out -Filter 'failbrauwser-*-windows.zip' -File -ErrorAction SilentlyContinue | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $latest) { throw "no package given and no zip in $out - run winpack\package.ps1 first" }
    $Package = $latest.FullName
}
$Package = (Resolve-Path $Package).Path

# Unpack a zip to work with; the package's own root folder carries the layout.
$src = $Package
if ($Package -match '\.zip$') {
    $tmp = Join-Path $env:TEMP ('fb-install-' + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $tmp | Out-Null
    Expand-Archive -LiteralPath $Package -DestinationPath $tmp
    $src = $tmp
}
$inner = Get-ChildItem $src -Directory -Filter 'failbrauwser-*' -ErrorAction SilentlyContinue | Select-Object -First 1
if ($inner) { $src = $inner.FullName }
if (-not (Test-Path -LiteralPath (Join-Path $src 'bin\failbrauwser.exe'))) { throw "$src holds no bin\failbrauwser.exe - is this the package from package.ps1?" }

New-Item -ItemType Directory -Force -Path $Prefix | Out-Null
Copy-Item (Join-Path $src '*') $Prefix -Recurse -Force
if ($tmp) { Remove-Item $tmp -Recurse -Force }

# The single-file launcher stamps the version of the tree it keeps current here; a
# manually installed one is no tree it may overwrite, so record this version too.
if ($src -match 'failbrauwser-(.+?)-[a-z0-9_]+-windows') { [IO.File]::WriteAllText((Join-Path $Prefix '.payload-version'), $Matches[1]) }

# The loader cache is a list of absolute paths; it has to be written where the loaders
# actually ended up. The tool ships in bin for exactly this step.
$bin = Join-Path $Prefix 'bin'
$cache = Join-Path $Prefix 'lib\gdk-pixbuf-2.0\2.10.0\loaders\loaders.cache'
$text = & (Join-Path $bin 'gdk-pixbuf-query-loaders.exe') | Out-String
if (-not $text -or $text -notmatch 'libpixbufloader') { throw 'writing the pixbuf loader cache failed' }
[IO.File]::WriteAllText($cache, $text.TrimEnd())

# Start Menu entry (per-user — no elevation; the same shortcut the Apps search shows).
$lnk = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\failBrauwser.lnk'
$shell = New-Object -ComObject WScript.Shell
$shortcut = $shell.CreateShortcut($lnk)
$shortcut.TargetPath = Join-Path $bin 'failbrauwser.exe'
$shortcut.WorkingDirectory = $bin
$shortcut.IconLocation = Join-Path $bin 'failbrauwser.exe,0'
$shortcut.Save()

Write-Output "INSTALLED: $Prefix"
Write-Output "Start: Start Menu > failBrauwser, or `"$bin\failbrauwser.exe`""
