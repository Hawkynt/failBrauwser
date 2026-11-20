#!/bin/bash
# The archive helper for Windows, built on Linux: fb-archive.exe in target/helper, where the program and
# its tests look for it (the twin of the helper half of winpack/build.ps1). NativeAOT refuses another
# system's target by default; ilc emits Windows objects all the same, and lld-link links them against the
# CRT and SDK setup.sh fetched, which it finds through LIB as link.exe would.
#   winpack/cross/build-helper.sh [Debug|Release]      (default Release)
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
. "$repo/winpack/cross/env.sh"
config=${1:-Release}
out=$repo/target/helper
# EnableSourceLink: lld-link has no /SOURCELINK. NativeDebugSymbols: no .pdb is shipped (the Makefile
# drops them too), and the CRT's libraries name debug files xwin does not carry.
dotnet publish "$repo/helper/FbArchive/FbArchive.csproj" -c "$config" -r win-x64 --nologo -v quiet \
  "-p:CwbRoot=$repo/vendor/compressionworkbench" \
  -p:DisableUnsupportedError=true -p:IlcUseEnvironmentalTools=true \
  -p:CppLinker=lld-link -p:CppLibCreator=llvm-lib \
  -p:EnableSourceLink=false -p:NativeDebugSymbols=false \
  -o "$out/win-x64"
mv -f "$out/win-x64/fb-archive.exe" "$out/fb-archive.exe"
rm -rf "$out/win-x64"
echo "built $out/fb-archive.exe"
