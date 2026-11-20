# Source this (bash) to build failBrauwser for Windows on Linux and run what it builds under Wine —
# the twin of winpack/env.ps1. winpack/cross/setup.sh fills the folder it reads first.
#   . winpack/cross/env.sh
#   cargo build --target x86_64-pc-windows-msvc        # target/x86_64-pc-windows-msvc/debug
#   cargo test --target x86_64-pc-windows-msvc --lib   # each test binary runs in Wine
# FB_WINCROSS is that folder (default ~/.local/share/fb-wincross); rustup is found through
# CARGO_HOME (default ~/.cargo), and its cargo comes before a distribution's own on PATH.
FB_WINCROSS=${FB_WINCROSS:-${XDG_DATA_HOME:-$HOME/.local/share}/fb-wincross}
export FB_WINCROSS
_fb_x=$FB_WINCROSS/xwin _fb_r=$FB_WINCROSS/sysroot
export PATH="${CARGO_HOME:-$HOME/.cargo}/bin:$PATH"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER=lld-link
# lld-link finds the CRT and SDK libraries through LIB as link.exe does (the archive helper's link uses it).
export LIB="$_fb_x/crt/lib/x86_64;$_fb_x/sdk/lib/um/x86_64;$_fb_x/sdk/lib/ucrt/x86_64"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUSTFLAGS="-Lnative=$_fb_x/crt/lib/x86_64 -Lnative=$_fb_x/sdk/lib/um/x86_64 -Lnative=$_fb_x/sdk/lib/ucrt/x86_64 -Lnative=$_fb_r/ucrt64/lib"
export CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine
# C parts of dependencies: clang in MSVC mode against the same CRT and SDK headers.
export CC_x86_64_pc_windows_msvc=clang-cl CXX_x86_64_pc_windows_msvc=clang-cl AR_x86_64_pc_windows_msvc=llvm-lib
export CFLAGS_x86_64_pc_windows_msvc="--target=x86_64-pc-windows-msvc -Wno-unused-command-line-argument /imsvc$_fb_x/crt/include /imsvc$_fb_x/sdk/include/ucrt /imsvc$_fb_x/sdk/include/um /imsvc$_fb_x/sdk/include/shared"
export CXXFLAGS_x86_64_pc_windows_msvc="$CFLAGS_x86_64_pc_windows_msvc"
# The GTK stack: the sysroot's .pc files name /ucrt64, which the sysroot folder is put in front of.
export PKG_CONFIG_ALLOW_CROSS=1 PKG_CONFIG_PATH=$_fb_r/ucrt64/lib/pkgconfig PKG_CONFIG_LIBDIR=$_fb_r/ucrt64/lib/pkgconfig PKG_CONFIG_SYSROOT_DIR=$_fb_r
# The program icon: build.rs takes FB_RC before looking for the Windows SDK's rc.exe.
export FB_RC=${FB_RC:-$(command -v llvm-rc)}
# Wine: a prefix of its own, the GTK DLLs and MinGit's git.exe on its PATH, no Mono or Gecko asked for.
export WINEPREFIX=$FB_WINCROSS/wineprefix WINEARCH=win64 WINEDEBUG=${WINEDEBUG:--all}
export WINEDLLOVERRIDES=${WINEDLLOVERRIDES:-mscoree,mshtml=}
export WINEPATH="Z:${_fb_r//\//\\}\\ucrt64\\bin;Z:${FB_WINCROSS//\//\\}\\mingit\\cmd"
unset _fb_x _fb_r
