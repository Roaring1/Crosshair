#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
SETTINGS_PID=""
BIN="$REPO_ROOT/target/debug/crosshair"
SETTINGS="$REPO_ROOT/target/debug/crosshair-settings"

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

echo "[2] expand General startup controls and click Install"
xdotool mousemove --window "$WINDOW" 515 187 click 1
sleep 0.45
xdotool mousemove --window "$WINDOW" 140 398 click 1
for _ in $(seq 1 80); do
    [[ -f "$AUTOSTART" ]] && break
    sleep 0.05
done
test -f "$AUTOSTART"
test -f "$APP_ENTRY"
grep -q -- "--daemon --no-ui" "$AUTOSTART"
echo "PASS: Settings Install autostart created the XDG entry"

echo "[3] click Remove autostart"
# The one-line result feedback expands the window slightly, leaving the
# optional startup buttons fully in view.
sleep 0.35
xdotool mousemove --window "$WINDOW" 414 442 click 1
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
