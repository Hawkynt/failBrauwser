#Requires -Version 5.1
# Builds the release pair and packs failbrauwser-<version>-<arch>-windows.zip: the
# release asset whose name the updater matches on — src/update.rs `package_name` is the
# one formula, mirrored here by reading the version from Cargo.toml. The zip holds a
# folder named after itself, containing `bin` (program, archive helper, all DLLs),
# `lib` (the gdk-pixbuf loaders, the home GTK derives from the bin folder) and
# `share` (icons, fonts, schemas, licenses). install.ps1 unpacks exactly this layout,
# as does Help > Check for Updates.
#   winpack\package.ps1            # build, then pack
#   winpack\package.ps1 -SkipBuild # pack what is already built
param([switch]$SkipBuild)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
$repo = Split-Path -Parent $PSScriptRoot

if (-not $SkipBuild) {
    & (Join-Path $PSScriptRoot 'build.ps1') -Release
    if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
}

$version = ((& cargo metadata --no-deps --format-version 1) | ConvertFrom-Json).packages[0].version
$arch = 'x86_64'   # std::env::consts::ARCH on this target
$rootName = "failbrauwser-$version-$arch-windows"
$stage = Join-Path (Join-Path $PSScriptRoot 'out') $rootName
if (Test-Path -LiteralPath $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path $stage | Out-Null

& (Join-Path $PSScriptRoot 'deps.ps1') -Exe (Join-Path $repo "target\release\failbrauwser.exe") -Out $stage
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

# The two programs share the bin folder: locate_helper finds the archive helper right
# next to the app there.
Copy-Item (Join-Path $repo 'target\release\failbrauwser.exe') (Join-Path $stage 'bin\failbrauwser.exe')
Copy-Item (Join-Path $repo 'target\helper\fb-archive.exe') (Join-Path $stage 'bin\fb-archive.exe')
foreach ($doc in 'LICENSE', 'README.md') { Copy-Item (Join-Path $repo $doc) (Join-Path $stage $doc) }

$zip = Join-Path (Join-Path $PSScriptRoot 'out') "$rootName.zip"
if (Test-Path -LiteralPath $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $stage -DestinationPath $zip
Write-Output "PACKAGED: $zip ($([math]::Round((Get-Item $zip).Length / 1MB, 1)) MB)"

# The single-file release: one program holding that whole zip inside, so a folder with
# nothing else is a complete install (see src/portable.rs). It builds from its own crate
# (winpack\onefile), which must run with that folder as the working directory so its
# .cargo\config.toml (static CRT - no VC++ redistributable) applies.
$onefileDir = Join-Path $PSScriptRoot 'onefile'
$env:FB_PAYLOAD = $zip
$env:FB_VERSION = $version
Push-Location $onefileDir
& cargo build --release
$onefileRc = $LASTEXITCODE
Pop-Location
if ($onefileRc -ne 0) { exit $onefileRc }
$one = Join-Path (Join-Path $PSScriptRoot 'out') "$rootName.exe"
Copy-Item (Join-Path $onefileDir 'target\release\failbrauwser-onefile.exe') $one -Force
Write-Output "ONEFILE: $one ($([math]::Round((Get-Item $one).Length / 1MB, 1)) MB)"
