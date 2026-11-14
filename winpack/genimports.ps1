#Requires -Version 5.1
# Generates MSVC import libraries (.lib) from the sysroot's MinGW-built GTK DLLs so the
# MSVC Rust toolchain can link against them (the packages only ship GNU .dll.a imports).
# Collects every -l token that pkgconf reports for the .pc files in the sysroot, maps it
# to its DLL (lib<token>[-N].dll), dumps exports with dumpbin and runs lib.exe /def.
# Re-run after bootstrap.ps1. Idempotent: skips .lib newer than its DLL unless -Force.
[CmdletBinding()]
param([switch]$Force)

$ErrorActionPreference = 'Stop'
$root = $PSScriptRoot
$sysroot = Join-Path $root 'sysroot'
$bin = Join-Path $sysroot 'ucrt64\bin'
$lib = Join-Path $sysroot 'ucrt64\lib'
$pkgconfig = Join-Path $sysroot 'ucrt64\lib\pkgconfig'
$pkgconf = Join-Path $bin 'pkgconf.exe'
if (-not (Test-Path -LiteralPath $pkgconf)) { throw 'run bootstrap.ps1 first (pkgconf missing)' }

$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
if (-not (Test-Path -LiteralPath $vswhere)) { throw 'Visual Studio Installer (vswhere) not found' }
$vcRoots = & $vswhere -latest -prerelease -find 'VC\Tools\MSVC\*\bin\Hostx64\x64\lib.exe'
$libExe = $vcRoots | Sort-Object -Descending | Select-Object -First 1
if (-not $libExe) { throw 'lib.exe not found in any installed VC toolset (C++ workload missing?)' }
$dumpbin = Join-Path (Split-Path $libExe) 'dumpbin.exe'

# every link name pkgconf may emit for the modules the Rust sys crates probe
$tokens = New-Object System.Collections.Generic.SortedSet[string]
foreach ($pc in (Get-ChildItem -LiteralPath $pkgconfig -Filter '*.pc')) {
  $out = & $pkgconf --libs $pc.BaseName 2>$null
  foreach ($arg in (@($out) -join ' ' -split '\s+')) {
    if ($arg.StartsWith('-l') -and $arg.Length -gt 2) { [void]$tokens.Add($arg.Substring(2)) }
  }
}
Write-Host "link names referenced by the sysroot: $($tokens.Count)"

$made = 0; $skipped = 0; $missing = @()
foreach ($token in $tokens) {
  $dllCandidates = @(
    (Join-Path $bin "lib$token.dll"),
    (Get-ChildItem -LiteralPath $bin -Filter "lib$token-?.dll" -ErrorAction SilentlyContinue | Select-Object -First 1 -ExpandProperty FullName),
    (Get-ChildItem -LiteralPath $bin -Filter "lib$token-*.dll" -ErrorAction SilentlyContinue | Where-Object { $_.Name -match [regex]::Escape("lib$token-") + '\d+\.dll$' } | Select-Object -First 1 -ExpandProperty FullName)
  ) | Where-Object { $_ -and (Test-Path -LiteralPath $_) } | Select-Object -First 1
  if (-not $dllCandidates) {
    # tokens whose DLL does not follow lib<token>-*.dll: the GTK chain emits -lz (zlib1.dll)
    # and -lintl (libintl-N.dll) even though the pc modules are named zlib/intl-8
    $alias = $null
    if ($token -eq 'z') { $alias = Join-Path $bin 'zlib1.dll' }
    if ($token -eq 'intl') { $alias = Get-ChildItem -LiteralPath $bin -Filter 'libintl-*.dll' | Select-Object -First 1 -ExpandProperty FullName }
    if ($alias -and (Test-Path -LiteralPath $alias)) { $dllCandidates = $alias }
    else {
      $direct = Join-Path $bin "$token.dll"
      if (Test-Path -LiteralPath $direct) { $dllCandidates = $direct } else { $missing += $token; continue }
    }
  }
  $outLib = Join-Path $lib "$token.lib"
  if (-not $Force -and (Test-Path -LiteralPath $outLib) -and
      (Get-Item -LiteralPath $outLib).LastWriteTime -gt (Get-Item -LiteralPath $dllCandidates).LastWriteTime) {
    $skipped++; continue
  }

  $exports = & $dumpbin /exports $dllCandidates 2>$null
  $def = Join-Path $env:TEMP ("def-" + $token + '.def')
  $lines = New-Object System.Collections.Generic.List[string]
  # the loader matches by the DLL name baked into the import lib: keep the real file name
  $lines.Add('LIBRARY ' + (Split-Path -Leaf $dllCandidates))
  $lines.Add('EXPORTS')
  $count = 0; $forwarded = 0; $anon = 0; $impSkipped = 0
  foreach ($line in $exports) {
    # rows: "        1    0 0001B100 name" — ordinal, hint, RVA, name; forwarded and
    # ordinal-only rows are skipped (the Rust bindings only call named functions;
    # a genuinely needed one would show up as an unresolved external at link)
    if ($line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+\S+\s+\(forwarded') { $forwarded++; continue }
    if ($line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+\[NAME: #\d+\]\s*$') { $anon++; continue }
    if ($line -match '^\s+\d+\s+[0-9A-Fa-f]+\s+[0-9A-Fa-f]+\s+(\S+)\s*$') {
      # GNU DLLs export the data-import thunk `__imp_X` literally; a def entry for X
      # already makes lib.exe synthesize `__imp_X` — listing both collides in the
      # import index and link.exe then reports the lib as corrupt (LNK1127)
      if ($Matches[1].StartsWith('__imp_')) { $impSkipped++; continue }
      $lines.Add($Matches[1])
      $count++
    }
  }
  if ($count -eq 0) { $missing += "$token (no exports parsed from $dllCandidates)"; Remove-Item -LiteralPath $def -ErrorAction SilentlyContinue; continue }
  if ($forwarded -or $anon -or $impSkipped) {
    Write-Host "$token`: skipped $forwarded forwarded, $anon ordinal-only, $impSkipped __imp_ data-thunk exports"
  }
  Set-Content -LiteralPath $def -Value $lines -Encoding ASCII
  # lib.exe prints progress to stdout; show it so corruption/empty-list warnings stay visible
  & $libExe /nologo "/def:$def" /machine:x64 "/out:$outLib"
  if ($LASTEXITCODE -ne 0) { throw "lib.exe failed for $token (dll: $dllCandidates)" }
  Remove-Item -LiteralPath $def, "$outLib.exp" -ErrorAction SilentlyContinue
  $made++
}
Write-Host "import libs: $made generated, $skipped current"
if ($missing.Count) { Write-Host "no import lib for: $($missing -join ', ') (only a problem if the build actually links them)" }
