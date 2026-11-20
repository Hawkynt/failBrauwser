#!/bin/bash
# Fills the folder winpack/cross/env.sh reads (FB_WINCROSS, default ~/.local/share/fb-wincross) so this
# checkout builds for x86_64-pc-windows-msvc on Linux and its tests run under Wine. Run again after the
# pinned GTK stack changes (winpack/bootstrap-lock.txt); what is there already is kept.
# Needs: rustup, clang, lld and llvm (clang-cl, lld-link, llvm-lib, llvm-dlltool, llvm-readobj, llvm-rc),
# pkgconf, zstd, bsdtar, curl and wine. xwin downloads Microsoft's CRT and Windows SDK and accepts their
# license in doing so (--accept-license), the same license Visual Studio's Build Tools come under.
set -euo pipefail
repo=$(cd "$(dirname "$0")/../.." && pwd)
. "$repo/winpack/cross/env.sh"
for tool in rustup clang-cl lld-link llvm-lib llvm-dlltool llvm-readobj llvm-rc pkgconf zstd bsdtar curl wine; do
  command -v "$tool" >/dev/null || { echo "setup.sh: $tool is missing" >&2; exit 1; }
done
case $FB_WINCROSS in *' '*) echo "setup.sh: FB_WINCROSS holds a space, which pkg-config cannot hand to cargo: $FB_WINCROSS" >&2; exit 1;; esac
mkdir -p "$FB_WINCROSS/dl" "$FB_WINCROSS/sysroot"

# 1. the Windows standard library for the toolchain cargo uses here
rustup target add x86_64-pc-windows-msvc

# 2. the MSVC CRT and the Windows SDK, headers and import libraries
if [ ! -d "$FB_WINCROSS/xwin/crt" ]; then
  command -v xwin >/dev/null || cargo install --locked xwin
  xwin --accept-license --arch x86_64 --cache-dir "$FB_WINCROSS/xwin-cache" splat --output "$FB_WINCROSS/xwin"
  rm -rf "$FB_WINCROSS/xwin-cache"
fi

# 3. the pinned UCRT64 GTK stack, the very files winpack/bootstrap.ps1 fetched (bootstrap-lock.txt),
#    from the MSYS2 repository itself, since winpack/mirror.txt names a company-internal mirror
grep -v '^#' "$repo/winpack/bootstrap-lock.txt" | grep '\.pkg\.tar' | while read -r pkg; do
  [ -s "$FB_WINCROSS/dl/$pkg" ] || curl -sSfL -o "$FB_WINCROSS/dl/$pkg" "https://repo.msys2.org/mingw/ucrt64/$pkg"
  tar --zstd -xf "$FB_WINCROSS/dl/$pkg" -C "$FB_WINCROSS/sysroot" --exclude=.PKGINFO --exclude=.BUILDINFO --exclude=.MTREE
done
# winpack/sysroot is where the PowerShell scripts look; here it is this one
[ -e "$repo/winpack/sysroot" ] || ln -s "$FB_WINCROSS/sysroot" "$repo/winpack/sysroot"

# 4. MSVC import libraries for the GTK DLLs (twin of winpack/genimports.ps1)
"$repo/winpack/cross/genimports.sh" "$FB_WINCROSS/sysroot"

# 5. the Wine prefix, and the pixbuf loader cache written for this sysroot (install.ps1 does the same
#    for an install): the one in the packages names the folders of the machine that built them
wineboot -i >/dev/null 2>&1
(cd "$FB_WINCROSS/sysroot/ucrt64/bin" && wine gdk-pixbuf-query-loaders.exe --update-cache >/dev/null 2>&1)

# 6. git.exe for the version-control code (MinGit; its sh.exe does not start programs under Wine)
if [ ! -x "$FB_WINCROSS/mingit/cmd/git.exe" ]; then
  url=$(curl -sSf https://api.github.com/repos/git-for-windows/git/releases/latest | grep -o 'https://[^"]*/MinGit-[0-9.]*-64-bit\.zip' | head -1)
  mkdir -p "$FB_WINCROSS/mingit"
  curl -sSfL -o "$FB_WINCROSS/mingit.zip" "$url"
  (cd "$FB_WINCROSS/mingit" && bsdtar -xf ../mingit.zip)
  rm "$FB_WINCROSS/mingit.zip"
fi
echo "ready: . winpack/cross/env.sh; cargo build --target x86_64-pc-windows-msvc"
