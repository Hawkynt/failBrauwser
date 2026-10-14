#!/usr/bin/env bash
# Checks that a network share which stops answering does not hang failBrauwser: mounts
# GVfs's local test backend, freezes it (SIGSTOP, like a server that went away), opens a
# folder on it and expects the "not responding" message and dialog with the window still
# working. Runs on the current Wayland session with private settings and a private session
# bus (a window shows up for a few seconds); the backend is always resumed at the end.
#   scripts/share-hang-test.sh [binary]      (default: target/debug/failbrauwser)
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
BIN=${1:-$ROOT/target/debug/failbrauwser}
T=$(mktemp -d "${TMPDIR:-/tmp}/fb-hang-XXXXXX")
mkdir -p "$T/config" "$T/data" "$T/cache" "$T/share/d"
touch "$T/share/d/a.txt"
[ -d "$HOME/.config/dconf" ] && cp -r "$HOME/.config/dconf" "$T/config/"

gio mount localtest:/// </dev/null 2>/dev/null || true
SHARE="${XDG_RUNTIME_DIR:-/run/user/$(id -u)}/gvfs/localtest:$T/share/d"
timeout 5 ls "$SHARE" >/dev/null || { echo "GVfs localtest backend not available" >&2; exit 2; }
BACKEND=$(pgrep -f "^/usr/lib/gvfsd-localtest" | head -1)

cat > "$T/script.fbt" <<EOF
open $SHARE
wait-error is not responding
wait-dialog is not responding
dialog-button is not responding : Cancel
open $T
expect-location $T
EOF

trap 'kill -CONT "$BACKEND" 2>/dev/null; gio mount -u localtest:/// 2>/dev/null; rm -rf "$T"' EXIT
kill -STOP "$BACKEND"
export XDG_CONFIG_HOME=$T/config XDG_DATA_HOME=$T/data XDG_CACHE_HOME=$T/cache GTK_A11Y=none NO_AT_BRIDGE=1
# Into a file: helpers the private bus starts keep a pipe open after the app is gone.
timeout -s KILL 60 dbus-run-session -- env GDK_BACKEND=wayland FB_SELFTEST="$T/script.fbt" "$BIN" >"$T/log" 2>&1 </dev/null &
RUN=$!
# Leaving needs the share again (files on it are closed): resume once the result is in.
for _ in $(seq 600); do
    grep -q -E "SELFTEST (OK|FAIL)" "$T/log" 2>/dev/null && break
    kill -0 "$RUN" 2>/dev/null || break
    sleep 0.1
done
kill -CONT "$BACKEND"
wait "$RUN" 2>/dev/null || true
if grep -q "SELFTEST OK" "$T/log"; then
    echo "share hang test: OK"
else
    grep -v -E "dbus-daemon|SpiRegistry|portal" "$T/log" | tail -20 >&2
    echo "share hang test: FAILED" >&2
    exit 1
fi
