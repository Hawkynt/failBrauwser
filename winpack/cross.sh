#!/bin/bash
# Builds, tests and packages the Windows program from Linux: the x86_64-pc-windows-gnu Rust
# target, the MinGW-w64 cross GCC as linker, and the same pinned MSYS2 UCRT64 GTK stack the
# PowerShell scripts use. The tests and the package run under Wine.
#   winpack/cross.sh bootstrap      # fetch bootstrap-lock.txt's packages into winpack/sysroot
#   winpack/cross.sh build [--release]
#   winpack/cross.sh helper         # the archive helper (fb-archive.exe) into target/helper
#   winpack/cross.sh test [args]    # cargo test under Wine (args go to cargo test)
#   winpack/cross.sh test-each [f]  # every test (name containing f) alone, with a time limit
#   winpack/cross.sh package        # winpack/out: the zip and the single-file exe
#   winpack/cross.sh run [args]     # start the debug build under Wine
# Needs: rustup target x86_64-pc-windows-gnu, x86_64-w64-mingw32-gcc (Arch: mingw-w64-gcc,
# Debian: gcc-mingw-w64-x86-64), pkgconf, zstd, zip, the .NET SDK and Wine.
# WINEPREFIX picks the Wine prefix (default: ~/.cache/failbrauwser-wine, so tests that use
# the recycle bin, the registry or the desktop never touch your own; never below the repo,
# where its z: drive would lead cargo's file scan through the whole filesystem).
set -euo pipefail
HERE=$(cd "$(dirname "$0")" && pwd)
REPO=$(dirname "$HERE")
TARGET=x86_64-pc-windows-gnu
SYSROOT=$HERE/sysroot
UCRT=$SYSROOT/ucrt64
export WINEPREFIX=${WINEPREFIX:-${XDG_CACHE_HOME:-$HOME/.cache}/failbrauwser-wine}
export WINEDEBUG=${WINEDEBUG:--all}
# A target directory of its own: the Linux build keeps its lock and its artifacts.
export CARGO_TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target/windows}

die() { echo "cross.sh: $*" >&2; exit 1; }

mirror() { grep -v '^#' "$HERE/mirror.txt" | grep -m1 . | tr -d '\r' | sed 's|/*$||'; }

bootstrap() {
  local m dl=$HERE/dl
  m=$(mirror)
  mkdir -p "$dl" "$SYSROOT"
  for f in $(grep -v '^#' "$HERE/bootstrap-lock.txt" | tr -d '\r' | grep '\.pkg\.tar'); do
    if [ ! -s "$dl/$f" ]; then
      echo "downloading $f"
      curl -fsSL -o "$dl/$f.part" "$m/mingw/ucrt64/$f" && mv "$dl/$f.part" "$dl/$f"
    fi
    tar --zstd -xf "$dl/$f" -C "$SYSROOT" --exclude='.BUILDINFO' --exclude='.MTREE' --exclude='.PKGINFO' --exclude='.INSTALL'
  done
  # pacman's install hooks, which a raw extraction skips: the settings schemas (the compiled
  # file is the same on every platform) and the pixbuf loader cache (written by the shipped
  # tool, so it names the loaders the way Windows sees them)
  glib-compile-schemas "$UCRT/share/glib-2.0/schemas"
  wine "$UCRT/bin/gdk-pixbuf-query-loaders.exe" --update-cache
  [ -f "$UCRT/bin/libgtk-3-0.dll" ] || die "bootstrap finished but libgtk-3-0.dll is missing"
  echo "sysroot ready: $SYSROOT"
}

env_setup() {
  [ -f "$UCRT/bin/libgtk-3-0.dll" ] || die "no sysroot yet: run winpack/cross.sh bootstrap"
  command -v x86_64-w64-mingw32-gcc >/dev/null || die "x86_64-w64-mingw32-gcc not on PATH"
  export PKG_CONFIG_ALLOW_CROSS=1
  export PKG_CONFIG_PATH=$UCRT/lib/pkgconfig
  export PKG_CONFIG_LIBDIR=$UCRT/lib/pkgconfig
  export PKG_CONFIG_SYSROOT_DIR=$SYSROOT
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
  export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUNNER=wine
  export CC_x86_64_pc_windows_gnu=x86_64-w64-mingw32-gcc
  export AR_x86_64_pc_windows_gnu=x86_64-w64-mingw32-ar
  export WINDRES=x86_64-w64-mingw32-windres
  # The programs find the GTK DLLs through PATH until they are packaged beside them; Wine
  # takes PATH from the registry, WINEPATH adds to it.
  export WINEPATH="$(winepath -w "$UCRT/bin" 2>/dev/null)"
}

# The test fixtures write archives with Python ("python" on Windows): the embeddable
# distribution, unpacked into the Wine prefix once.
PYTHON_EMBED=https://www.python.org/ftp/python/3.12.10/python-3.12.10-embed-amd64.zip
prefix_setup() {
  [ -d "$WINEPREFIX/drive_c" ] || wineboot -i >/dev/null 2>&1
  local py=$WINEPREFIX/drive_c/fb-python
  if [ ! -f "$py/python.exe" ]; then
    mkdir -p "$py"
    curl -fsSL -o "$py/embed.zip" "$PYTHON_EMBED"
    (cd "$py" && unzip -qo embed.zip && rm embed.zip)
  fi
  WINEPATH="$WINEPATH;C:\\fb-python"
}

helper() {
  # NativeAOT does not cross-compile between operating systems, so the cross build ships the
  # helper as a self-contained single file instead; the Windows-native build.ps1 keeps AOT.
  dotnet publish "$REPO/helper/FbArchive/FbArchive.csproj" -c Release -r win-x64 \
    -p:PublishAot=false -p:SelfContained=true -p:PublishSingleFile=true \
    -p:IncludeNativeLibrariesForSelfExtract=true "-p:CwbRoot=$REPO/vendor/compressionworkbench" \
    -o "$REPO/target/helper" --nologo -v quiet
  rm -f "$REPO/target/helper/"*.pdb
}

# The DLLs a PE file imports, by name.
imports() { x86_64-w64-mingw32-objdump -p "$1" | sed -n 's/^\s*DLL Name: //p'; }

package() {
  env_setup
  cargo build --release --target $TARGET --manifest-path "$REPO/Cargo.toml"
  helper
  local version arch=x86_64 root stage out=$HERE/out
  version=$(cargo metadata --no-deps --format-version 1 --manifest-path "$REPO/Cargo.toml" | sed -n 's/.*"name":"failbrauwser","version":"\([^"]*\)".*/\1/p')
  [ -n "$version" ] || die "no version in Cargo.toml"
  root=failbrauwser-$version-$arch-windows
  stage=$out/$root
  rm -rf "$stage" "$out/$root.zip"
  mkdir -p "$stage/bin" "$stage/lib/gdk-pixbuf-2.0/2.10.0/loaders"
  cp "$CARGO_TARGET_DIR/$TARGET/release/failbrauwser.exe" "$stage/bin/"
  cp "$REPO/target/helper/fb-archive.exe" "$stage/bin/"
  cp "$UCRT/bin/gdk-pixbuf-query-loaders.exe" "$stage/bin/"
  cp "$UCRT/lib/gdk-pixbuf-2.0/2.10.0/loaders/"*.dll "$stage/lib/gdk-pixbuf-2.0/2.10.0/loaders/"
  # every sysroot DLL reachable from the program, the loader tool and the loaders (deps.ps1)
  local -A seen=()
  local queue=("$stage/bin/failbrauwser.exe" "$stage/bin/gdk-pixbuf-query-loaders.exe" "$stage/lib/gdk-pixbuf-2.0/2.10.0/loaders/"*.dll)
  while [ ${#queue[@]} -gt 0 ]; do
    local f=${queue[0]} dll lower src
    queue=("${queue[@]:1}")
    while read -r dll; do
      lower=${dll,,}
      [ -n "${seen[$lower]:-}" ] && continue
      seen[$lower]=1
      src=$(find "$UCRT/bin" -maxdepth 1 -iname "$dll" -print -quit)
      if [ -n "$src" ]; then
        cp "$src" "$stage/bin/"
        queue+=("$src")
      fi
    done < <(imports "$f")
  done
  for tree in share/glib-2.0/schemas/gschemas.compiled share/icons/Adwaita share/icons/AdwaitaLegacy share/icons/hicolor share/fontconfig share/licenses; do
    [ -e "$UCRT/$tree" ] || die "runtime asset missing: $tree"
    mkdir -p "$stage/$(dirname "$tree")"
    cp -r "$UCRT/$tree" "$stage/$tree"
  done
  cp "$REPO/LICENSE" "$REPO/README.md" "$stage/"
  (cd "$out" && zip -qr "$root.zip" "$root")
  echo "PACKAGED: $out/$root.zip"
  # the single-file release: the zip embedded in the launcher crate (see package.ps1)
  FB_PAYLOAD=$out/$root.zip FB_VERSION=$version \
    CARGO_TARGET_X86_64_PC_WINDOWS_GNU_RUSTFLAGS="-C target-feature=+crt-static" \
    cargo build --release --target $TARGET --manifest-path "$HERE/onefile/Cargo.toml"
  cp "$CARGO_TARGET_DIR/$TARGET/release/failbrauwser-onefile.exe" "$out/$root.exe"
  echo "ONEFILE: $out/$root.exe"
}

cmd=${1:-}
[ $# -gt 0 ] && shift
case $cmd in
  bootstrap) bootstrap ;;
  build) env_setup; cargo build --target $TARGET --manifest-path "$REPO/Cargo.toml" "$@" ;;
  helper) helper ;;
  test|test-each)
        env_setup; prefix_setup
        [ -f "$REPO/target/helper/fb-archive.exe" ] || helper
        # the UI scenarios run in-process (PowerShell does not run under Wine), and on a
        # private X server when there is one, so no window lands on the desktop
        export FB_UI_HARNESS=builtin
        xvfb=(); command -v xvfb-run >/dev/null && xvfb=(xvfb-run -a -s "-screen 0 1280x800x24")
        if [ "$cmd" = test ]; then
          "${xvfb[@]}" cargo test --target $TARGET --manifest-path "$REPO/Cargo.toml" "$@"
        else
          # every test in a process of its own with a time limit: where Wine lacks a service
          # a test can hang, and one hang must not hide the rest
          "${xvfb[@]}" "$0" each-inner "$@"
        fi ;;
  each-inner)
        env_setup
        mapfile -t bins < <(cargo test --no-run --target $TARGET --manifest-path "$REPO/Cargo.toml" --message-format=json 2>/dev/null |
          sed -n 's/.*"executable":"\([^"]*\/deps\/[^"]*\)".*/\1/p')
        pass=0; skip=0; fail=(); hang=()
        # Output goes to files, never pipes: Wine's own background processes (explorer,
        # a session bus a test started) inherit the handles and would keep a pipe open.
        log=$(mktemp -d)
        for bin in "${bins[@]}"; do
          name=$(basename "$bin" .exe); name=${name%-*}
          (cd "$REPO" && wine "$bin" --list --format terse </dev/null >"$log/list" 2>/dev/null) || true
          while read -r t; do
            [ -n "$t" ] || continue
            [ $# -eq 0 ] || [[ "$t" == *"$1"* ]] || continue
            rc=0
            (cd "$REPO" && timeout -k 5 "${FB_TEST_TIMEOUT:-180}" wine "$bin" --exact "$t" --test-threads=1 </dev/null >"$log/out" 2>&1) || rc=$?
            if [ $rc -eq 0 ] && grep -q '^test result: ok. 1 passed' "$log/out"; then
              pass=$((pass+1)); echo "ok    $name::$t"
            elif [ $rc -eq 0 ] && grep -q '^test result: ok. 0 passed; 0 failed; 1 ignored' "$log/out"; then
              skip=$((skip+1)); echo "skip  $name::$t (#[ignore])"
            elif [ $rc -ge 124 ]; then
              hang+=("$name::$t"); echo "HANG  $name::$t"; wineserver -k 2>/dev/null || true
            else
              fail+=("$name::$t"); echo "FAIL  $name::$t"
              grep -vE 'pci id|MESA|^$' "$log/out" | sed -n '/^---- /,/^failures:/p' | head -40 | sed 's/^/      /'
            fi
          done < <(tr -d '\r' <"$log/list" | sed -n 's/: test$//p')
        done
        rm -rf "$log"
        echo "passed ${pass}, ignored ${skip}, failed ${#fail[@]}, timed out ${#hang[@]}"
        [ ${#fail[@]} -eq 0 ] && [ ${#hang[@]} -eq 0 ] ;;
  package) package ;;
  run) env_setup; cargo build --target $TARGET --manifest-path "$REPO/Cargo.toml"
       wine "$CARGO_TARGET_DIR/$TARGET/debug/failbrauwser.exe" "$@" ;;
  *) sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'; exit 2 ;;
esac
