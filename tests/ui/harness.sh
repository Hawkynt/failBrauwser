#!/bin/bash
# Runs failBrauwser with a self-test script inside a headless sway session.
#   harness.sh <binary> <script> <workdir>
# The script may use $T for <workdir> and $UI for this folder. Config, data and trash live below <workdir>, so the
# user's settings and trash are never touched; the GTK theme settings are copied over.
set -u
BIN=$1; SCRIPT=$2; T=$3
# Parallel runs would pick up each other's compositor socket: one at a time.
exec 9>"${TMPDIR:-/tmp}/failbrauwser-ui-test.lock"
flock 9
mkdir -p "$T/config" "$T/data" "$T/cache"
# The look comes from the user's desktop (even when HOME is pointed at demo data).
TH=${FB_THEME_HOME:-$HOME}
[ -d "$TH/.config/gtk-3.0" ] && cp -r "$TH/.config/gtk-3.0" "$T/config/" 2>/dev/null
# On Wayland GTK takes theme, icons and fonts from GSettings (the dconf database).
[ -d "$TH/.config/dconf" ] && cp -r "$TH/.config/dconf" "$T/config/" 2>/dev/null
# The window manager's theme (tileWin): the progress window draws its charts in its colors.
if [ -d "$TH/.config/tileWin" ]; then
  mkdir -p "$T/config/tileWin"
  for f in current-theme color-scheme themes; do
    [ -e "$TH/.config/tileWin/$f" ] && cp -r "$TH/.config/tileWin/$f" "$T/config/tileWin/" 2>/dev/null
  done
fi
# Icon and GTK themes installed per user live in the data dir; keep them visible (a theme's
# light or dark variant is looked up there).
[ -d "$TH/.local/share/icons" ] && ln -sfn "$TH/.local/share/icons" "$T/data/icons"
[ -d "$TH/.local/share/themes" ] && ln -sfn "$TH/.local/share/themes" "$T/data/themes"

# $T: the work folder; $UI: this folder (helpers next to the scripts).
UI=$(cd "$(dirname "$0")" && pwd)
sed "s|\$T|$T|g; s|\$UI|$UI|g" "$SCRIPT" > "$T/script.fbt"
# A runtime folder of its own (short: socket paths are limited): the session's sockets and
# GVfs mounts stay apart from the user's, and a share hanging there cannot hang the test.
RUNDIR=$(mktemp -d /tmp/fbui.XXXXXX)
chmod 700 "$RUNDIR"
CONF=$(mktemp)
# sway, or where it is missing tileWin (built on sway; set up as its own tests run it):
# no panel, no session restore, its display name written to a file.
WM=sway
command -v sway >/dev/null || WM=tilewin
DISPLAY_FILE="$T/display"
rm -f "$DISPLAY_FILE"
if [ "$WM" = sway ]; then
cat > "$CONF" <<SWAY
output HEADLESS-1 resolution 1280x800 bg #2e3440 solid_color
default_border normal
SWAY
else
cat > "$CONF" <<TILEWIN
output HEADLESS-1 resolution 1280x800
wallpaper solid #2e3440
session_restore no
panel_command true
animations off
default_border normal
exec sh -c 'printf %s "\$WAYLAND_DISPLAY" > "$DISPLAY_FILE"'
TILEWIN
export TILEWIN_NO_APP_TWEAKS=1 TILEWIN_WAYLAND_SOCKET=wayland-fb-test-$$
export SWAYSOCK="$RUNDIR/wm.sock" TILEWINSOCK="$RUNDIR/wm.sock"
fi
# Before the private bus starts: services it activates (Tumbler, …) use the test folders too.
export XDG_CONFIG_HOME="$T/config" XDG_DATA_HOME="$T/data" XDG_CACHE_HOME="$T/cache" GTK_A11Y=none NO_AT_BRIDGE=1
export XDG_RUNTIME_DIR=$RUNDIR
exec dbus-run-session -- bash -c '
  unset DISPLAY
  before=$(ls "$XDG_RUNTIME_DIR" | grep -E "^wayland-[0-9]+$" | sort)
  WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 WLR_RENDERER=pixman '"$WM"' -c "'"$CONF"'" >"'"$T"'/sway.log" 2>&1 &
  SP=$!
  for i in $(seq 200); do
    if [ -s "'"$DISPLAY_FILE"'" ]; then new=$(cat "'"$DISPLAY_FILE"'"); break; fi
    new=$(comm -13 <(echo "$before") <(ls "$XDG_RUNTIME_DIR" | grep -E "^wayland-[0-9]+$" | sort) | head -1)
    [ -n "$new" ] && [ "'"$WM"'" = sway ] && break; new=; sleep 0.05
  done
  export WAYLAND_DISPLAY=$new GDK_BACKEND=wayland FB_SELFTEST="'"$T"'/script.fbt"
  timeout 60 "'"$BIN"'"
  rc=$?
  kill $SP 2>/dev/null; wait $SP 2>/dev/null
  exit $rc
' 2>&1 | grep -v -E "dbus-daemon|SpiRegistry|fusermount"
rc=${PIPESTATUS[0]}
rm -f "$CONF" "${SWAYSOCK:-}"
fusermount -uz "$RUNDIR/gvfs" 2>/dev/null || fusermount3 -uz "$RUNDIR/gvfs" 2>/dev/null
rm -rf "$RUNDIR" 2>/dev/null
exit $rc
