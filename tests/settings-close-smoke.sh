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

mkdir -p "$ROOT/home" "$ROOT/config" "$ROOT/state" "$ROOT/data" "$ROOT/runtime"
chmod 700 "$ROOT/runtime"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

echo "[1] start enabled overlay"
"$BIN" --show
"$BIN" --status

echo "[2] open Settings as a separate process"
"$SETTINGS" >"$ROOT/settings.log" 2>&1 &
SETTINGS_PID=$!
WINDOW=""
for _ in $(seq 1 100); do
    if ! kill -0 "$SETTINGS_PID" 2>/dev/null; then
        cat "$ROOT/settings.log"
        echo "Settings exited before showing a window" >&2
        exit 1
    fi
    WINDOW="$(xdotool search --onlyvisible --pid "$SETTINGS_PID" 2>/dev/null | head -1 || true)"
    if [[ -n "$WINDOW" ]]; then break; fi
    sleep 0.05
done
test -n "$WINDOW"
echo "settings pid=$SETTINGS_PID window=$WINDOW"
if [[ -n "${CROSSHAIR_SETTINGS_SCREENSHOT:-}" ]]; then
    command -v import >/dev/null || { echo "ImageMagick 'import' is required to capture a Settings screenshot." >&2; exit 2; }
    import -window "$WINDOW" "$CROSSHAIR_SETTINGS_SCREENSHOT"
fi

echo "[3] close Settings window as if the user pressed X"
xdotool windowclose "$WINDOW"
for _ in $(seq 1 100); do
    if ! kill -0 "$SETTINGS_PID" 2>/dev/null; then break; fi
    sleep 0.05
done
! kill -0 "$SETTINGS_PID" 2>/dev/null

echo "[4] overlay remains enabled after Settings exits"
"$BIN" --status | tee "$ROOT/overlay-status.log"
grep -q 'Crosshair daemon: running' "$ROOT/overlay-status.log"
grep -q 'Overlay: enabled' "$ROOT/overlay-status.log"
test -S "$XDG_RUNTIME_DIR/crosshair/ipc.sock"

echo "[5] hide overlay; it exits independently"
"$BIN" --hide
sleep 0.4
"$BIN" --status
test ! -S "$XDG_RUNTIME_DIR/crosshair/ipc.sock"

echo "PASS: closing Settings exits only Settings and leaves the overlay running"
