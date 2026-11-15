#!/usr/bin/env bash
# Builds and installs failBrauwser.
#
#   ./install.sh                 install for the current user into ~/.local
#   ./install.sh --system        install for everyone into /usr/local (asks for sudo)
#   ./install.sh --prefix DIR    install into DIR
#   ./install.sh --no-autostart  do not start the background instance at login
#   ./install.sh --default       make failBrauwser the default file manager
#   ./install.sh --no-network    do not install the GVfs SMB backend (smb:// addresses)
#   ./install.sh --no-archives   do not build archive support (needs no .NET SDK then)
#   ./install.sh --no-file-chooser  keep the desktop's own open and save dialogs for
#                                applications that ask the desktop portal for one, and
#                                for GTK 3 applications
#   ./install.sh --uninstall     remove what an install with the same options put in place
#   ./install.sh --prebuilt      install the program from a release package (bin/, lib/) as is
#   ./install.sh --reopen        open a window when done (Help > Check for Updates uses it)
#   ./install.sh --deps-only     only set up what building needs (packages, Rust, .NET SDK)
#
# Building needs Rust (as new as rust-version in Cargo.toml), GTK 3's development files and a
# C compiler. On Debian, Ubuntu and their kin the missing packages are installed with apt-get
# (asks for sudo). Where the system's Rust is older (Debian 13 has 1.85), a newer one is set up
# with rustup in ~/.local/share/failbrauwser/rust (no root needed, only used for building).
#
# Archive support (opening, extracting, creating archives) is a helper built with the
# .NET 10 SDK from CompressionWorkbench (by Hawkynt, vendored in vendor/compressionworkbench).
# Without an SDK on the PATH the script downloads one into ~/.local/share/failbrauwser/dotnet
# (no root needed, only used for building).
#
# Typed smb:// addresses are mounted through GVfs; its SMB backend is installed with the
# system's package manager (asks for sudo) unless it is already there.
set -euo pipefail

APP_ID=org.failbrauwser.FailBrauwser
HERE=$(cd "$(dirname "$0")" && pwd)
PREFIX="$HOME/.local"
SUDO=""
AUTOSTART=1
MAKE_DEFAULT=0
NETWORK=1
ARCHIVES=1
FILE_CHOOSER=1
UNINSTALL=0
PREBUILT=0
REOPEN=0
DEPS_ONLY=0
# The options to remember for updates (Help > Check for Updates installs the same way).
KEPT=()
CWB_ROOT=${CWB_ROOT:-"$HERE/vendor/compressionworkbench"}

while [ $# -gt 0 ]; do
    case "$1" in
        --system|--no-autostart|--default|--no-network|--no-archives|--no-file-chooser) KEPT+=("$1") ;;
    esac
    case "$1" in
        --system) PREFIX=/usr/local; SUDO=sudo ;;
        --prefix) PREFIX=$2; shift ;;
        --no-autostart) AUTOSTART=0 ;;
        --default) MAKE_DEFAULT=1 ;;
        --no-network) NETWORK=0 ;;
        --no-archives) ARCHIVES=0 ;;
        --uninstall) UNINSTALL=1 ;;
        --no-file-chooser) FILE_CHOOSER=0 ;;
        --prebuilt) PREBUILT=1 ;;
        --reopen) REOPEN=1 ;;
        --deps-only) DEPS_ONLY=1 ;;
        -h|--help) sed -n '2,31p' "$0" | sed 's/^# \{0,1\}//'; exit 0 ;;
        *) echo "unknown option: $1" >&2; exit 2 ;;
    esac
    shift
done

BIN_DIR="$PREFIX/bin"
LIB_DIR="$PREFIX/lib/failbrauwser"
SHARE="$PREFIX/share"
AUTOSTART_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/autostart/failbrauwser.desktop"
PORTAL_NAME=org.freedesktop.impl.portal.desktop.failbrauwser
PORTAL_CONF="${XDG_CONFIG_HOME:-$HOME/.config}/xdg-desktop-portal/portals.conf"
PORTAL_LINE="org.freedesktop.impl.portal.FileChooser=failbrauwser"
PORTAL_MARK="# Written by failBrauwser's install.sh"
# GTK 3 applications that make GTK's own dialog load this module, which asks the portal.
MODULE_NAME=libfailbrauwser-chooser.so
MODULE_ENV="${XDG_CONFIG_HOME:-$HOME/.config}/environment.d/60-failbrauwser.conf"

# xdg-desktop-portal reads one portals.conf: the user's takes the place of the system's, so a
# new one starts from the system's default and only the file chooser goes to failBrauwser.
portal_conf_add() {
    mkdir -p "$(dirname "$PORTAL_CONF")"
    if [ -f "$PORTAL_CONF" ]; then
        sed -i '/^org\.freedesktop\.impl\.portal\.FileChooser=/d' "$PORTAL_CONF"
        grep -q '^\[preferred\]' "$PORTAL_CONF" || printf '[preferred]\n' >> "$PORTAL_CONF"
        sed -i "/^\[preferred\]/a $PORTAL_LINE" "$PORTAL_CONF"
    else
        local default=""
        for f in /usr/share/xdg-desktop-portal/"$(echo "${XDG_CURRENT_DESKTOP%%:*}" | tr '[:upper:]' '[:lower:]')"-portals.conf \
                 /usr/share/xdg-desktop-portal/portals.conf /usr/share/xdg-desktop-portal/*-portals.conf; do
            [ -f "$f" ] && default=$(sed -n 's/^default=//p' "$f" | head -1) && [ -n "$default" ] && break
        done
        printf '%s\n[preferred]\ndefault=%s\n%s\n' "$PORTAL_MARK" "${default:-gtk}" "$PORTAL_LINE" > "$PORTAL_CONF"
    fi
}

portal_conf_remove() {
    [ -f "$PORTAL_CONF" ] || return 0
    sed -i "/^$PORTAL_LINE\$/d" "$PORTAL_CONF"
    # Ours alone: gone again, the system's settings apply as before.
    if head -1 "$PORTAL_CONF" | grep -qF "$PORTAL_MARK" && [ "$(grep -cv '^\(#.*\|\[preferred\]\|default=.*\|\)$' "$PORTAL_CONF")" = 0 ]; then
        rm -f "$PORTAL_CONF"
    fi
}

# GTK3_MODULES with the module at $1 added (or, without $1, failBrauwser's taken out) of
# the module list $2.
gtk3_modules() {
    local out=() m
    local IFS=:
    for m in $2; do
        [ -n "$m" ] && [ "${m##*/}" != "$MODULE_NAME" ] && out+=("$m")
    done
    [ -n "$1" ] && out=("$1" "${out[@]}")
    echo "${out[*]}"
}

# The session's GTK3_MODULES, for applications started through systemd or D-Bus from now
# on (the ones started by the desktop itself get it at the next login, from environment.d).
session_gtk3_modules() {
    command -v systemctl >/dev/null || return 0
    local now new
    now=$(systemctl --user show-environment 2>/dev/null | sed -n 's/^GTK3_MODULES=//p')
    new=$(gtk3_modules "${1:-}" "$now")
    [ "$new" = "$now" ] && return 0
    if [ -n "$new" ]; then
        systemctl --user set-environment "GTK3_MODULES=$new" 2>/dev/null || true
    else
        systemctl --user unset-environment GTK3_MODULES 2>/dev/null || true
    fi
    command -v dbus-update-activation-environment >/dev/null &&
        dbus-update-activation-environment "GTK3_MODULES=$new" 2>/dev/null || true
}

gtk_module_env_add() {
    mkdir -p "$(dirname "$MODULE_ENV")"
    {
        echo "$PORTAL_MARK: GTK 3 applications get failBrauwser's open and save dialogs."
        echo "GTK3_MODULES=$1\${GTK3_MODULES:+:\$GTK3_MODULES}"
    } > "$MODULE_ENV"
    session_gtk3_modules "$1"
}

gtk_module_env_remove() {
    rm -f "$MODULE_ENV"
    session_gtk3_modules ""
}

# The portal reads its backends when it starts.
restart_portal() {
    systemctl --user try-restart xdg-desktop-portal.service 2>/dev/null || true
}

say() { printf '\033[1m==>\033[0m %s\n' "$*"; }

# Downloads $1 into the file $2 (curl, else wget).
download() {
    if command -v curl >/dev/null; then curl -fsSL "$1" -o "$2"
    elif command -v wget >/dev/null; then wget -qO "$2" "$1"
    else echo "curl or wget is needed to download $1" >&2; return 1
    fi
}

# The packages building needs that are missing, on systems with apt (Debian, Ubuntu, …):
# a C compiler and linker, pkg-config, GTK 3's development files, a downloader, and for the
# archive helper zlib's development files (its native code links them).
missing_apt_packages() {
    local want=(build-essential pkg-config libgtk-3-dev)
    command -v curl >/dev/null || command -v wget >/dev/null || want+=(curl)
    [ "$ARCHIVES" = 1 ] && want+=(zlib1g-dev)
    local p
    for p in "${want[@]}"; do
        dpkg-query -W -f='${Status}' "$p" 2>/dev/null | grep -q 'install ok installed' || echo "$p"
    done
}

install_build_packages() {
    command -v apt-get >/dev/null && command -v dpkg-query >/dev/null || return 0
    local missing
    mapfile -t missing < <(missing_apt_packages)
    [ ${#missing[@]} -gt 0 ] || return 0
    say "Installing what building needs: sudo apt-get install -y ${missing[*]}"
    sudo apt-get install -y "${missing[@]}" ||
        echo "warning: the packages were not installed — install them with: sudo apt-get install ${missing[*]}" >&2
}

# The Rust the build needs (rust-version in Cargo.toml).
RUST_NEEDED=$(sed -n 's/^rust-version = "\(.*\)"/\1/p' "$HERE/Cargo.toml" 2>/dev/null | head -1)
RUST_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/failbrauwser/rust"

# Whether the rustc $1 is at least $RUST_NEEDED.
rust_new_enough() {
    local v
    v=$("$1" --version 2>/dev/null | awk '{print $2}' | cut -d- -f1)
    [ -n "$v" ] || return 1
    [ -z "$RUST_NEEDED" ] && return 0
    [ "$(printf '%s\n%s\n' "$RUST_NEEDED" "$v" | sort -V | head -1)" = "$RUST_NEEDED" ]
}

# cargo and rustc on the PATH, new enough: the system's, one set up earlier, or a new one
# from rustup in $RUST_HOME (the system's packages are left alone).
setup_rust() {
    if command -v cargo >/dev/null && command -v rustc >/dev/null && rust_new_enough rustc; then return 0; fi
    export CARGO_HOME="$RUST_HOME/cargo" RUSTUP_HOME="$RUST_HOME/rustup"
    export PATH="$CARGO_HOME/bin:$PATH"
    if [ -x "$CARGO_HOME/bin/rustc" ] && rust_new_enough "$CARGO_HOME/bin/rustc"; then return 0; fi
    if [ -x "$CARGO_HOME/bin/rustup" ]; then
        say "Updating the Rust in $RUST_HOME to the newest stable (Rust $RUST_NEEDED is needed)"
        "$CARGO_HOME/bin/rustup" update stable --no-self-update
    else
        local old
        old=$(command -v rustc >/dev/null && rustc --version 2>/dev/null | awk '{print $2}' || true)
        say "Setting up Rust in $RUST_HOME with rustup (Rust $RUST_NEEDED is needed${old:+, the system has $old})"
        local script
        script=$(mktemp)
        download https://sh.rustup.rs "$script" || { rm -f "$script"; echo "Rust could not be downloaded: see https://rustup.rs" >&2; exit 1; }
        sh "$script" -y --no-modify-path --profile minimal --default-toolchain stable || { rm -f "$script"; echo "Rust could not be set up: see https://rustup.rs" >&2; exit 1; }
        rm -f "$script"
    fi
    rust_new_enough "$CARGO_HOME/bin/rustc" || { echo "Rust $RUST_NEEDED or newer is required: https://rustup.rs" >&2; exit 1; }
}

refresh_caches() {
    command -v update-desktop-database >/dev/null && $SUDO update-desktop-database -q "$SHARE/applications" 2>/dev/null || true
    command -v gtk-update-icon-cache >/dev/null && $SUDO gtk-update-icon-cache -q -t -f "$SHARE/icons/hicolor" 2>/dev/null || true
}

if [ "$UNINSTALL" = 1 ]; then
    say "Stopping the background instance"
    "$BIN_DIR/failbrauwser" --quit 2>/dev/null || true
    say "Removing failBrauwser from $PREFIX"
    $SUDO rm -f "$BIN_DIR/failbrauwser" \
        "$SHARE/applications/$APP_ID.desktop" \
        "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
    $SUDO rm -f "$LIB_DIR/fb-archive" "$LIB_DIR/$MODULE_NAME"
    gtk_module_env_remove
    $SUDO rmdir "$LIB_DIR" 2>/dev/null || true
    rm -f "$AUTOSTART_FILE"
    $SUDO rm -f "$SHARE/xdg-desktop-portal/portals/failbrauwser.portal" "$SHARE/dbus-1/services/$PORTAL_NAME.service"
    portal_conf_remove
    restart_portal
    refresh_caches
    say "Done. Settings in ~/.config/failbrauwser were kept."
    exit 0
fi

APP_BIN="$HERE/target/release/failbrauwser"
if [ "$PREBUILT" = 1 ]; then
    # A release package: the program and the archive helper are built already.
    APP_BIN="$HERE/bin/failbrauwser"
    [ -x "$APP_BIN" ] || { echo "$APP_BIN is missing: not a release package" >&2; exit 1; }
    HELPER=""
    [ "$ARCHIVES" = 1 ] && [ -x "$HERE/lib/fb-archive" ] && HELPER="$HERE/lib/fb-archive"
    MODULE=""
    [ -f "$HERE/lib/$MODULE_NAME" ] && MODULE="$HERE/lib/$MODULE_NAME"
    # A package built on a newer system needs a newer C library than this one may have.
    if ! why=$("$APP_BIN" --version 2>&1); then
        echo "the ready-built program does not run on this system: $(echo "$why" | head -1)" >&2
        echo "build it from the source instead (./install.sh in the source folder)" >&2
        exit 1
    fi
else
    install_build_packages
    setup_rust
    pkg-config --exists gtk+-3.0 || { echo "GTK 3 development files are required (gtk3; libgtk-3-dev on Debian and Ubuntu)" >&2; exit 1; }

    HELPER=""
    HELPER_PID=""
    HELPER_LOG="$HERE/target/helper-build.log"
    DOTNET_HOME="${XDG_DATA_HOME:-$HOME/.local/share}/failbrauwser/dotnet"
    # The .NET SDK does not start without ICU, which a small installation may lack; building
    # the helper needs none of what ICU does.
    has_icu=0
    for f in /usr/lib/libicuuc.so.* /usr/lib/*/libicuuc.so.* /usr/lib64/libicuuc.so.*; do
        [ -e "$f" ] && has_icu=1 && break
    done
    [ "$has_icu" = 1 ] || export DOTNET_SYSTEM_GLOBALIZATION_INVARIANT=1

    # A .NET SDK that can build net10.0: on the PATH, from an earlier run, or downloaded now.
    has_sdk10() { "$1" --list-sdks 2>/dev/null | grep -Eq '^(1[0-9]|[2-9][0-9])\.'; }
    find_dotnet() {
        if command -v dotnet >/dev/null && has_sdk10 dotnet; then command -v dotnet; return; fi
        if [ -x "$DOTNET_HOME/dotnet" ] && has_sdk10 "$DOTNET_HOME/dotnet"; then echo "$DOTNET_HOME/dotnet"; return; fi
        say "Downloading the .NET 10 SDK into $DOTNET_HOME (for building the archive helper)" >&2
        local script
        script=$(mktemp)
        download https://dot.net/v1/dotnet-install.sh "$script" || { rm -f "$script"; return 1; }
        bash "$script" --channel 10.0 --install-dir "$DOTNET_HOME" --no-path >&2 || { rm -f "$script"; return 1; }
        rm -f "$script"
        has_sdk10 "$DOTNET_HOME/dotnet" && echo "$DOTNET_HOME/dotnet"
    }

    if [ "$DEPS_ONLY" = 1 ]; then
        if [ "$ARCHIVES" = 1 ] && [ -f "$CWB_ROOT/Compression.Lib/Compression.Lib.csproj" ]; then
            DOTNET=$(find_dotnet) || { echo "warning: no .NET 10 SDK could be set up — archive support will not build" >&2; DOTNET=""; }
            [ -n "$DOTNET" ] && say ".NET SDK: $DOTNET"
        fi
        say "Ready to build: $(cargo --version)"
        exit 0
    fi

    if [ "$ARCHIVES" = 1 ] && [ -f "$CWB_ROOT/Compression.Lib/Compression.Lib.csproj" ]; then
        if DOTNET=$(find_dotnet); then
            # The helper is compiled to native code, which needs a C toolchain to link.
            AOT_LINKER=()
            if ! command -v clang >/dev/null; then
                if command -v gcc >/dev/null; then AOT_LINKER=(-p:CppCompilerAndLinker=gcc)
                else echo "warning: neither clang nor gcc found — the archive helper cannot be linked" >&2
                fi
            fi
            # Built alongside the app: most of the helper's native compile runs on one core,
            # so the Rust build uses the others meanwhile.
            say "Building the archive helper in the background (CompressionWorkbench at $CWB_ROOT; log: $HELPER_LOG)"
            mkdir -p "$HERE/target"
            (cd "$HERE" && PATH="$(dirname "$DOTNET"):$PATH" DOTNET_ROOT="$(dirname "$(readlink -f "$DOTNET")")" \
                DOTNET_CLI_TELEMETRY_OPTOUT=1 DOTNET_NOLOGO=1 \
                make helper CWB_ROOT="$(cd "$CWB_ROOT" && pwd)" HELPER_FLAGS="${AOT_LINKER[*]}") >"$HELPER_LOG" 2>&1 &
            HELPER_PID=$!
            # Do not leave it running when the app build fails.
            trap '[ -n "$HELPER_PID" ] && kill "$HELPER_PID" 2>/dev/null' EXIT
        else
            echo "warning: no .NET 10 SDK could be set up — installing without archive support" >&2
        fi
    elif [ "$ARCHIVES" = 1 ]; then
        echo "warning: $CWB_ROOT has no CompressionWorkbench — installing without archive support" >&2
    fi

    # The crates the build compiles, one "Compiling" line each: the update window's progress
    # bar counts them (README, Updates). Unknown when cargo cannot answer.
    UNITS=$(cd "$HERE" && cargo tree -e normal,build --prefix none --locked 2>/dev/null | sed 's/ (\*)$//; s/ (proc-macro)//' | sort -u | wc -l)
    if [ "${UNITS:-0}" -gt 0 ]; then say "Building failBrauwser: $UNITS crates"; else say "Building failBrauwser"; fi
    (cd "$HERE" && cargo build --release --locked)

    MODULE=""
    if [ "$FILE_CHOOSER" = 1 ]; then
        if (cd "$HERE" && make -s module CC="${CC:-cc}") >/dev/null; then
            MODULE="$HERE/target/module/$MODULE_NAME"
        else
            echo "warning: the GTK 3 module did not build — GTK 3 applications keep GTK's own dialogs" >&2
        fi
    fi

    if [ -n "$HELPER_PID" ]; then
        say "Waiting for the archive helper"
        if wait "$HELPER_PID"; then
            HELPER="$HERE/target/helper/fb-archive"
        else
            grep -v 'warning CS' "$HELPER_LOG" | tail -20 >&2
            echo "warning: the archive helper did not build (log: $HELPER_LOG) — installing without archive support" >&2
        fi
        HELPER_PID=""
    fi

fi

say "Installing into $PREFIX"
$SUDO install -Dm755 "$APP_BIN" "$BIN_DIR/failbrauwser"
if [ -n "$HELPER" ]; then
    $SUDO install -Dm755 "$HELPER" "$LIB_DIR/fb-archive"
fi
if [ "$FILE_CHOOSER" = 1 ] && [ -n "$MODULE" ]; then
    $SUDO install -Dm755 "$MODULE" "$LIB_DIR/$MODULE_NAME"
fi
$SUDO install -Dm644 "$HERE/data/$APP_ID.desktop" "$SHARE/applications/$APP_ID.desktop"
$SUDO install -Dm644 "$HERE/data/icons/$APP_ID.svg" "$SHARE/icons/hicolor/scalable/apps/$APP_ID.svg"
if [ "$PREFIX" != /usr ] && [ "$PREFIX" != /usr/local ]; then
    # Launchers need the absolute path when the prefix is not on everyone's PATH.
    $SUDO sed -i "s|^Exec=failbrauwser|Exec=$BIN_DIR/failbrauwser|; s|^TryExec=failbrauwser|TryExec=$BIN_DIR/failbrauwser|" "$SHARE/applications/$APP_ID.desktop"
fi
refresh_caches

# The GVfs SMB backend, for typed smb:// addresses.
if [ "$NETWORK" = 1 ] && [ ! -e /usr/share/gvfs/mounts/smb.mount ]; then
    if command -v pacman >/dev/null; then smb_install=(sudo pacman -S --needed gvfs-smb)
    elif command -v apt-get >/dev/null; then smb_install=(sudo apt-get install -y gvfs-backends)
    elif command -v dnf >/dev/null; then smb_install=(sudo dnf install -y gvfs-smb)
    elif command -v zypper >/dev/null; then smb_install=(sudo zypper install gvfs-backend-samba)
    else smb_install=()
    fi
    if [ ${#smb_install[@]} -gt 0 ]; then
        say "Installing the GVfs SMB backend (smb:// addresses): ${smb_install[*]}"
        if "${smb_install[@]}"; then
            # The running GVfs daemon only knows the backends it started with.
            systemctl --user restart gvfs-daemon 2>/dev/null || true
        else
            echo "warning: the GVfs SMB backend was not installed — smb:// addresses will not open" >&2
        fi
    else
        echo "warning: install your distribution's GVfs SMB backend for smb:// addresses" >&2
    fi
fi

if [ "$AUTOSTART" = 1 ]; then
    say "Starting a background instance at login (windows then open instantly)"
    install -Dm644 "$HERE/data/failbrauwser-autostart.desktop" "$AUTOSTART_FILE"
    sed -i "s|^Exec=failbrauwser|Exec=$BIN_DIR/failbrauwser|" "$AUTOSTART_FILE"
else
    rm -f "$AUTOSTART_FILE"
fi

if [ "$FILE_CHOOSER" = 1 ]; then
    say "Answering applications' open and save dialogs (through the desktop portal)"
    $SUDO install -Dm644 "$HERE/data/failbrauwser.portal" "$SHARE/xdg-desktop-portal/portals/failbrauwser.portal"
    $SUDO install -Dm644 "$HERE/data/$PORTAL_NAME.service" "$SHARE/dbus-1/services/$PORTAL_NAME.service"
    $SUDO sed -i "s|^Exec=failbrauwser|Exec=$BIN_DIR/failbrauwser|" "$SHARE/dbus-1/services/$PORTAL_NAME.service"
    portal_conf_add
    if [ -f "$LIB_DIR/$MODULE_NAME" ]; then
        say "GTK 3 applications ask failBrauwser too (GTK3_MODULES in $MODULE_ENV)"
        gtk_module_env_add "$LIB_DIR/$MODULE_NAME"
    fi
else
    $SUDO rm -f "$SHARE/xdg-desktop-portal/portals/failbrauwser.portal" "$SHARE/dbus-1/services/$PORTAL_NAME.service"
    portal_conf_remove
    $SUDO rm -f "$LIB_DIR/$MODULE_NAME"
    gtk_module_env_remove
fi

if [ "$MAKE_DEFAULT" = 1 ]; then
    say "Making failBrauwser the default file manager"
    xdg-mime default "$APP_ID.desktop" inode/directory
fi

# Replace a running (older) instance and keep the new one ready right away.
"$BIN_DIR/failbrauwser" --quit 2>/dev/null || true
if [ "$AUTOSTART" = 1 ] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    (setsid "$BIN_DIR/failbrauwser" --daemon >/dev/null 2>&1 &)
fi
restart_portal

# Remembered for Help > Check for Updates.
OPTIONS_FILE="${XDG_CONFIG_HOME:-$HOME/.config}/failbrauwser/install-options"
mkdir -p "$(dirname "$OPTIONS_FILE")"
echo "${KEPT[*]:-}" > "$OPTIONS_FILE"

if [ "$REOPEN" = 1 ] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    (setsid "$BIN_DIR/failbrauwser" >/dev/null 2>&1 &)
fi

say "Installed. Start it from your menu or run: failbrauwser"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "note: $BIN_DIR is not on your PATH" ;; esac
