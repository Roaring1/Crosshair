#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
SETTINGS_PID=""
BIN="${CROSSHAIR_BIN:-$REPO_ROOT/target/debug/crosshair}"
SETTINGS="${CROSSHAIR_SETTINGS_BIN:-$REPO_ROOT/target/debug/crosshair-settings}"

cleanup() {
    "$BIN" --quit >/dev/null 2>&1 || true
    if [[ -n "$SETTINGS_PID" ]]; then
        kill "$SETTINGS_PID" 2>/dev/null || true
        wait "$SETTINGS_PID" 2>/dev/null || true
    fi
    rm -rf "$ROOT"
}
trap cleanup EXIT

for binary in "$BIN" "$SETTINGS"; do
    if [[ ! -x "$binary" ]]; then
        echo "Build both Crosshair executables before running this Settings UI test." >&2
        exit 2
    fi
done

mkdir -p "$ROOT/home" "$ROOT/config" "$ROOT/state" "$ROOT/data" "$ROOT/runtime"
chmod 700 "$ROOT/runtime"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_BIN_HOME="$ROOT/bin with space % and \$cash"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

AUTOSTART="$XDG_CONFIG_HOME/autostart/crosshair.desktop"
APP_ENTRY="$XDG_DATA_HOME/applications/crosshair.desktop"
export AUTOSTART APP_ENTRY

echo "[1] start overlay and open Settings"
"$BIN" --show
"$SETTINGS" >"$ROOT/settings.log" 2>&1 &
SETTINGS_PID=$!
WINDOW=""
for _ in $(seq 1 100); do
    if ! kill -0 "$SETTINGS_PID" 2>/dev/null; then
        cat "$ROOT/settings.log"
        echo "Settings exited before showing a window." >&2
        exit 1
    fi
    WINDOW="$(xdotool search --onlyvisible --pid "$SETTINGS_PID" 2>/dev/null | head -1 || true)"
    [[ -n "$WINDOW" ]] && break
    sleep 0.05
done
test -n "$WINDOW"
window_height() {
    xdotool getwindowgeometry --shell "$WINDOW" | awk -F= '$1 == "HEIGHT" { print $2 }'
}
INITIAL_WINDOW_HEIGHT="$(window_height)"
test "$INITIAL_WINDOW_HEIGHT" = 520

echo "[2] expand General startup controls and click Install"
# App Activity adds 68 px to the General page, so the startup disclosure
# and its expanded install action moved down by the same amount.
xdotool mousemove --window "$WINDOW" 589 283 click 1
sleep 0.45
test "$(window_height)" = "$INITIAL_WINDOW_HEIGHT"
xdotool mousemove --window "$WINDOW" 140 430 click 1
for _ in $(seq 1 80); do
    [[ -f "$AUTOSTART" ]] && break
    sleep 0.05
done
test -f "$AUTOSTART"
test -f "$APP_ENTRY"
grep -q -- "--daemon --no-ui" "$AUTOSTART"
echo "PASS: Settings Install autostart created the XDG entry"
test "$(window_height)" = "$INITIAL_WINDOW_HEIGHT"

echo "[3] click Remove autostart"
# Feedback is rendered inline without resizing the native window. The
# top-aligned General content keeps both startup actions in the same place.
sleep 0.35
test "$(window_height)" = "$INITIAL_WINDOW_HEIGHT"
xdotool mousemove --window "$WINDOW" 468 450 click 1
for _ in $(seq 1 80); do
    [[ ! -e "$AUTOSTART" ]] && break
    sleep 0.05
done
test ! -e "$AUTOSTART"
echo "PASS: Settings Remove autostart removed the XDG entry"

echo "[4] close Settings; the overlay must remain alive"
xdotool windowclose "$WINDOW"
for _ in $(seq 1 100); do
    kill -0 "$SETTINGS_PID" 2>/dev/null || break
    sleep 0.05
done
! kill -0 "$SETTINGS_PID" 2>/dev/null
SETTINGS_PID=""
"$BIN" --status | tee "$ROOT/overlay-status.log"
grep -q "Crosshair daemon: running" "$ROOT/overlay-status.log"
grep -q "Overlay: enabled" "$ROOT/overlay-status.log"

echo "[5] hide overlay; no Crosshair process should remain"
"$BIN" --hide
sleep 0.4
"$BIN" --status
test ! -S "$XDG_RUNTIME_DIR/crosshair/ipc.sock"
echo "PASS: Settings autostart controls work and preserve Padfire/overlay lifecycle"
