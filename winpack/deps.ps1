#Requires -Version 5.1
# Assembles the runtime file set one built program needs: every DLL reachable from it
# through the import table (dumpbin), plus the plugin trees GTK loads while running —
# the gdk-pixbuf loaders, which live beside the binaries at ../lib — and the data
# trees (icons, fonts, schemas, licenses) those libraries read. System DLLs stay with
# Windows; every sysroot DLL joins the staging bin folder.
#   winpack\deps.ps1 -Exe target\release\failbrauwser.exe -Out winpack\out\staging
param(
    [Parameter(Mandatory = $true)][string]$Exe,
    [Parameter(Mandatory = $true)][string]$Out
)
$ErrorActionPreference = 'Stop'
$sysroot = Join-Path $PSScriptRoot 'sysroot\ucrt64'
$Exe = (Resolve-Path $Exe).Path

# dumpbin comes through vswhere exactly like genimports.ps1 finds lib.exe.
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Visual Studio Installer (vswhere) not found' }
$found = & $vswhere -latest -prerelease -find 'VC\Tools\MSVC\*\bin\Hostx64\x64\dumpbin.exe'
if (-not $found) { throw 'dumpbin.exe not found through vswhere' }
$dumpbin = ($found | Sort-Object -Descending | Select-Object -First 1)

function Dependencies([string]$file) {
    & $dumpbin /dependents $file | ForEach-Object { if ($_ -match '^\s+(\S+\.dll)$') { $Matches[1] } }
}

$sysBins = @{}
foreach ($f in (Get-ChildItem (Join-Path $sysroot 'bin') -Filter '*.dll' -File)) { $sysBins[$f.Name.ToLowerInvariant()] = $f.FullName }
$sys32 = "$env:SystemRoot\System32"

# Plugins (the gdk-pixbuf loaders) keep their home relative to the binaries — GTK looks
# for <DLL folder>\..\lib\gdk-pixbuf-2.0\2.10.0\loaders — so they stage into lib and only
# their own imports join bin. They are seeded into the queue like the program itself.
$binOut = Join-Path $Out 'bin'
$loadersOut = Join-Path $Out 'lib\gdk-pixbuf-2.0\2.10.0\loaders'
New-Item -ItemType Directory -Force -Path $binOut, $loadersOut | Out-Null
$copied = @{}
$queue = [System.Collections.Generic.Queue[string]]::new()
$queue.Enqueue($Exe)
foreach ($ld in (Get-ChildItem (Join-Path $sysroot 'lib\gdk-pixbuf-2.0\2.10.0\loaders') -Filter '*.dll' -File)) {
    Copy-Item $ld.FullName (Join-Path $loadersOut $ld.Name) -Force
    $copied[$ld.Name.ToLowerInvariant()] = 'plugin'
    $queue.Enqueue($ld.FullName)
}
# The cache carries absolute paths of the machine that built it; install.ps1 writes a
# fresh one on the target machine with the shipped tool, which belongs in the package.
if (Test-Path -LiteralPath (Join-Path $sysroot 'bin\gdk-pixbuf-query-loaders.exe')) {
    Copy-Item (Join-Path $sysroot 'bin\gdk-pixbuf-query-loaders.exe') (Join-Path $binOut 'gdk-pixbuf-query-loaders.exe') -Force
}

# Breadth-first over the import graph of the program and every plugin.
$unknown = @()
while ($queue.Count) {
    foreach ($dep in (Dependencies $queue.Dequeue())) {
        $key = $dep.ToLowerInvariant()
        if ($copied.ContainsKey($key)) { continue }
        if ($sysBins.ContainsKey($key)) {
            $copied[$key] = $sysBins[$key]
            Copy-Item $sysBins[$key] (Join-Path $binOut $dep) -Force
            $queue.Enqueue($sysBins[$key])
        }
        elseif ($key.StartsWith('api-ms-win-') -or $key.StartsWith('ext-ms-')) {
            $copied[$key] = 'apiset'   # virtual API sets the loader resolves itself (the CRT among them)
        }
        elseif (Test-Path -LiteralPath (Join-Path $sys32 $dep)) {
            $copied[$key] = 'system'   # Windows itself provides it
        }
        else { $unknown += $dep }
    }
}
if ($unknown) { throw "dependencies neither in the sysroot nor in Windows: $((($unknown | Sort-Object -Unique) -join ', '))" }

# The data trees GTK reads at runtime: the compiled settings schemas (path-portable,
# copied as built), the icon themes the menus use, the font configuration, and every
# dependency's license text, which the GPL obligations of the shipped libraries need.
foreach ($tree in 'share\glib-2.0\schemas\gschemas.compiled', 'share\icons\Adwaita', 'share\icons\AdwaitaLegacy', 'share\icons\hicolor', 'share\fontconfig', 'share\licenses') {
    $src = Join-Path $sysroot $tree
    if (-not (Test-Path -LiteralPath $src)) { throw "runtime asset missing: $tree" }
    $dst = Join-Path $Out $tree
    New-Item -ItemType Directory -Force -Path (Split-Path $dst) | Out-Null
    if ((Get-Item -LiteralPath $src).PSIsContainer) { Copy-Item $src $dst -Recurse -Force } else { Copy-Item $src $dst -Force }
}

$binCount = (Get-ChildItem $binOut -Filter '*.dll' -File).Count
Write-Output "DEPS OK: $binCount DLLs in bin, loaders and data trees staged"
