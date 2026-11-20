#!/bin/bash
# Linux twin of winpack/genimports.ps1: MSVC import libraries (.lib) for the sysroot's MinGW-built
# GTK DLLs, made from each DLL's export table (llvm-readobj) by llvm-dlltool. setup.sh runs it.
set -euo pipefail
R="${1:?the sysroot folder (winpack/cross/setup.sh passes it)}"
bin=$R/ucrt64/bin lib=$R/ucrt64/lib
export PKG_CONFIG_LIBDIR=$lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$R
tokens=$(for pc in "$lib"/pkgconfig/*.pc; do pkgconf --libs "$(basename "$pc" .pc)" 2>/dev/null || true; done | tr ' ' '\n' | sed -n 's/^-l\(.\+\)$/\1/p' | sort -u)
made=0 missing=()
tmp=$(mktemp -d); trap 'rm -rf "$tmp"' EXIT
for t in $tokens; do
  dll=""
  for c in "$bin/lib$t.dll" "$bin"/lib"$t"-[0-9].dll "$bin"/lib"$t"-[0-9]*.dll; do [ -f "$c" ] && { dll=$c; break; }; done
  [ -z "$dll" ] && [ "$t" = z ] && [ -f "$bin/zlib1.dll" ] && dll=$bin/zlib1.dll
  [ -z "$dll" ] && [ "$t" = intl ] && dll=$(ls "$bin"/libintl-*.dll 2>/dev/null | head -1)
  [ -z "$dll" ] && [ -f "$bin/$t.dll" ] && dll=$bin/$t.dll
  [ -z "$dll" ] && { missing+=("$t"); continue; }
  { echo "LIBRARY $(basename "$dll")"; echo EXPORTS
    llvm-readobj --coff-exports "$dll" | sed -n 's/^ *Name: \(.\+\)$/\1/p' | grep -v '^__imp_'; } > "$tmp/$t.def"
  llvm-dlltool -m i386:x86-64 -d "$tmp/$t.def" -l "$lib/$t.lib"
  made=$((made+1))
done
echo "import libs: $made generated"
[ ${#missing[@]} -gt 0 ] && echo "no import lib for: ${missing[*]} (system libraries come from the SDK)"
exit 0
