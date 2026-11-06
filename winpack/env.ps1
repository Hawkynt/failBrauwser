#Requires -Version 5.1
# Dot-source this to point the MSVC Rust build at the winpack sysroot:
#   . winpack\env.ps1 ; cargo build --release
# PKG_CONFIG_SYSROOT_DIR makes pkgconf emit real Windows paths for the sysroot's
# /ucrt64-prefixed .pc files; genimports.ps1 has created the matching .lib imports.
$winpackSysroot = Split-Path -Parent $MyInvocation.MyCommand.Path
$env:PKG_CONFIG = Join-Path $winpackSysroot 'sysroot\ucrt64\bin\pkgconf.exe'
$env:PKG_CONFIG_PATH = Join-Path $winpackSysroot 'sysroot\ucrt64\lib\pkgconfig'
$env:PKG_CONFIG_SYSROOT_DIR = $winpackSysroot + '\sysroot'
$env:PATH = (Join-Path $winpackSysroot 'sysroot\ucrt64\bin') + ';' + $env:PATH
# rustup keeps cargo in a folder of its own; machines that only ever ran it through a
# shell profile otherwise lose sight of it here.
if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = (Join-Path $env:USERPROFILE '.cargo\bin') + ';' + $env:PATH }
