#Requires -Version 5.1
# Fills winpack/sysroot with the pinned MSYS2 UCRT64 GTK stack from the internal mirror
# (see mirror.txt, packages.txt). Run once; use -Clean after changing packages.txt.
# Produces sysroot/ucrt64/{bin,include,lib,share} plus bootstrap-lock.txt (what was fetched).
# MSVC import libraries (.lib) are generated separately by genimports.ps1.
[CmdletBinding()]
param([switch]$Clean)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$mirror = (Get-Content -LiteralPath (Join-Path $root 'mirror.txt') |
  Where-Object { $_ -and -not $_.StartsWith('#') } | Select-Object -First 1).TrimEnd('/')
$sysroot = Join-Path $root 'sysroot'
$dl = Join-Path $root 'dl'
$tar = Join-Path $env:SystemRoot 'System32\tar.exe' # bsdtar: reads the .zst packages

if ($Clean) {
  if (Test-Path $sysroot) { Remove-Item -LiteralPath $sysroot -Recurse -Force }
  if (Test-Path $dl) { Remove-Item -LiteralPath $dl -Recurse -Force }
}
New-Item -ItemType Directory -Force -Path $sysroot, $dl | Out-Null

# 1. fetch the repository database: a zstd tarball holding one directory per package
$dbDir = Join-Path $env:TEMP ('failbrauwser-msys2-db-' + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $dbDir | Out-Null
try {
  $db = Join-Path $dbDir 'ucrt64.db'
  Write-Host "reading repo database from $mirror/mingw/ucrt64/ucrt64.db"
  Invoke-WebRequest -UseBasicParsing -Uri "$mirror/mingw/ucrt64/ucrt64.db" -OutFile $db
  & $tar -xf $db -C $dbDir
  if ($LASTEXITCODE -ne 0) { throw 'failed to unpack ucrt64.db' }

  # 2. parse every package's desc (fields: %NAME% %VERSION% %FILENAME% %DEPENDS% %PROVIDES%;
  #    %VERSION% already includes the release, e.g. "3.24.52-1")
  $byBase = @{}
  $byProvide = @{}
  Get-ChildItem -LiteralPath $dbDir -Directory | ForEach-Object {
    $descPath = Join-Path $_.FullName 'desc'
    if (-not (Test-Path -LiteralPath $descPath)) { return }
    $fields = @{}
    $field = $null
    foreach ($line in (Get-Content -LiteralPath $descPath)) {
      if ($line -match '^%([A-Z]+)%$') { $field = $Matches[1]; $fields[$field] = @() }
      elseif ($field -and $line -ne '') { $fields[$field] += $line }
    }
    if (-not $fields['NAME'] -or -not $fields['FILENAME'] -or -not $fields['VERSION']) { return }
    $pkg = [pscustomobject]@{
      Base = $fields['NAME'][0]
      Version = $fields['VERSION'][0]
      FileName = $fields['FILENAME'][0]
      Depends = @(if ($fields['DEPENDS']) { $fields['DEPENDS'] })
      Provides = @(if ($fields['PROVIDES']) { $fields['PROVIDES'] })
    }
    $byBase[$pkg.Base] = $pkg
    foreach ($p in $pkg.Provides) { $name = (($p -split '[<>=]')[0]).Trim(); if ($name -and -not $byProvide.ContainsKey($name)) { $byProvide[$name] = $pkg } }
  }
  Write-Host "database knows $($byBase.Count) packages"

  # 3. pin check: every packages.txt entry must exist at the exact version
  $pinned = @()
  foreach ($line in (Get-Content -LiteralPath (Join-Path $root 'packages.txt') |
      Where-Object { $_ -and -not $_.StartsWith('#') })) {
    $parts = $line.Trim() -split '\s+'
    if ($parts.Count -ne 2) { throw "bad packages.txt line: $line" }
    $name, $version = $parts
    if (-not $byBase.ContainsKey($name)) { throw "package not in database: $name" }
    if ($byBase[$name].Version -ne $version) {
      throw "$name is version $($byBase[$name].Version) on the mirror, packages.txt pins $version"
    }
    $pinned += $byBase[$name]
  }

  # 4. transitive closure (msys-runtime deps are skipped: the sysroot is MinGW/UCRT only)
  $wanted = [ordered]@{}
  $queue = New-Object System.Collections.Queue
  $pinned | ForEach-Object { $queue.Enqueue($_) }
  $skippedMsys = @()
  while ($queue.Count) {
    $pkg = $queue.Dequeue()
    if ($wanted.Contains($pkg.Base)) { continue }
    $wanted[$pkg.Base] = $pkg
    foreach ($dep in $pkg.Depends) {
      $depName = (($dep -split '[<>=]')[0]).Trim()
      if (-not $depName) { continue }
      if ($depName.StartsWith('msys/') -or $depName -eq 'msys-runtime' -or $depName -eq 'windows-default-manifest') {
        $skippedMsys += "$($pkg.Base) -> $depName"; continue
      }
      $next = $null
      if ($byBase.ContainsKey($depName)) { $next = $byBase[$depName] }
      elseif ($byProvide.ContainsKey($depName)) { $next = $byProvide[$depName] }
      else { throw "unresolvable dependency: $depName (required by $($pkg.Base))" }
      if (-not $wanted.Contains($next.Base)) { $queue.Enqueue($next) }
    }
  }
  if ($skippedMsys.Count) { Write-Host ('skipped msys-only deps: ' + (($skippedMsys | Sort-Object -Unique) -join ', ')) }
  Write-Host "closure: $($wanted.Count) packages"

  # 5. download (cached in winpack/dl) and unpack into sysroot
  foreach ($pkg in $wanted.Values) {
    $local = Join-Path $dl $pkg.FileName
    if (-not (Test-Path -LiteralPath $local) -or (Get-Item -LiteralPath $local).Length -eq 0) {
      Write-Host "downloading $($pkg.FileName)"
      Invoke-WebRequest -UseBasicParsing -Uri "$mirror/mingw/ucrt64/$($pkg.FileName)" -OutFile $local
    }
    & $tar -xf $local -C $sysroot # archives carry their own ucrt64/ prefix
    if ($LASTEXITCODE -ne 0) { throw "failed to unpack $($pkg.FileName)" }
  }

  # 5b. pacman runs install hooks after unpacking (a raw tar extraction skips them):
  #     gdk-pixbuf needs loaders.cache or no image loader is ever found, and GSettings
  #     needs gschemas.compiled (FileChooser widgets read those schemas at runtime).
  #     The UCRT build is relocatable: both tools resolve paths relative to their own DLLs.
  & (Join-Path $sysroot 'ucrt64\bin\gdk-pixbuf-query-loaders.exe') --update-cache
  if ($LASTEXITCODE -ne 0) { throw 'gdk-pixbuf-query-loaders --update-cache failed' }
  & (Join-Path $sysroot 'ucrt64\bin\glib-compile-schemas.exe') (Join-Path $sysroot 'ucrt64\share\glib-2.0\schemas')
  if ($LASTEXITCODE -ne 0) { throw 'glib-compile-schemas failed' }

  # 6. audit trail
  $lockPath = Join-Path $root 'bootstrap-lock.txt'
  ($wanted.Values | ForEach-Object { $_.FileName } | Sort-Object) + @('', "# fetched $(Get-Date -Format 'u') from $mirror") |
    Set-Content -LiteralPath $lockPath

  $gtk = Join-Path $sysroot 'ucrt64\bin\libgtk-3-0.dll'
  if (-not (Test-Path -LiteralPath $gtk)) { throw 'bootstrap finished but libgtk-3-0.dll is missing' }
  Write-Host "sysroot ready: $sysroot"
}
finally {
  Remove-Item -LiteralPath $dbDir -Recurse -Force -ErrorAction SilentlyContinue
}
