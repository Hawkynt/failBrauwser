#Requires -Version 5.1
# Day-to-day Windows build entry (the twin of `make`): builds the GTK-linked app
# and, when the .NET SDK is present, the NativeAOT archive helper next to it.
#   winpack\build.ps1            # debug
#   winpack\build.ps1 -Release   # release
param([switch]$Release)
$ErrorActionPreference = 'Stop'
. (Join-Path $PSScriptRoot 'env.ps1')
$repo = Split-Path -Parent $PSScriptRoot

$cargoArgs = @('build') + $(if ($Release) { @('--release') } else { @() })
& cargo @cargoArgs
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

$csproj = Join-Path $repo 'helper\FbArchive\FbArchive.csproj'
if (Get-Command dotnet -ErrorAction SilentlyContinue) {
    $vcvars = 'C:\Program Files\Microsoft Visual Studio\18\Professional\VC\Auxiliary\Build\vcvarsall.bat'
    if (-not (Test-Path $vcvars)) {
        $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
        if (Test-Path $vswhere) {
            $install = & $vswhere -latest -property installationPath
            $vcvars = Join-Path $install 'VC\Auxiliary\Build\vcvarsall.bat'
        }
    }
    if (Test-Path $vcvars) {
        # Same development location the Linux Makefile uses: target/helper, which
        # locate_helper checks for both the app and the test binaries.
        $out = Join-Path $repo 'target\helper'
        # VS18 renamed the VC component ids so ilcompiler's own vcvars probe finds
        # nothing; initializing the environment ourselves plus the escape hatch is
        # the settled way (see winpack/probe/publish.ps1).
        cmd /c "call `"$vcvars`" amd64 && cd /d `"$repo`" && dotnet publish helper\FbArchive\FbArchive.csproj -c $(if ($Release) { 'Release' } else { 'Debug' }) -r win-x64 -p:IlcUseEnvironmentalTools=true -o `"$out`""
        if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }
    } else {
        Write-Warning 'No vcvarsall.bat found: the app is built, the archive helper is skipped.'
    }
} else {
    Write-Warning 'No dotnet SDK: the app is built, the archive helper is skipped.'
}
Write-Output 'BUILD OK'
