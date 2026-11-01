#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
PROBE_PID=""
BIN="$REPO_ROOT/target/debug/crosshair"
PROBE="$ROOT/crosshair-x11-input-probe"

cleanup() {
    "$BIN" --quit >/dev/null 2>&1 || true
    if [[ -n "$PROBE_PID" ]]; then
        kill "$PROBE_PID" 2>/dev/null || true
        wait "$PROBE_PID" 2>/dev/null || true
    fi
    rm -rf "$ROOT"
}
trap cleanup EXIT

command -v cc >/dev/null || { echo "A C compiler is required for the X11 input probe." >&2; exit 2; }
cc "$SCRIPT_DIR/x11_input_probe.c" -o "$PROBE" -lX11 -lXext
mkdir -p "$ROOT/home" "$ROOT/config" "$ROOT/state" "$ROOT/data" "$ROOT/runtime"
chmod 700 "$ROOT/runtime"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

"$PROBE" >"$ROOT/probe.log" 2>&1 &
PROBE_PID=$!
for _ in $(seq 1 50); do
    grep -q '^READY ' "$ROOT/probe.log" && break
    sleep 0.05
done
grep '^READY ' "$ROOT/probe.log"

echo "[1] cold toggle starts overlay"
"$BIN" --toggle
"$BIN" --status

echo "[2] inspect overlay ARGB and pass-through properties"
WINDOW="$(xdotool search --onlyvisible --name '^Crosshair$' | tail -1)"
echo "window=$WINDOW"
"$PROBE" inspect "$WINDOW" | tee "$ROOT/inspect.log"
grep -q '^DEPTH 32$' "$ROOT/inspect.log"
grep -q '^INPUT_HINT false$' "$ROOT/inspect.log"
grep -q '^INPUT_SHAPE_RECTS 0$' "$ROOT/inspect.log"
xprop -id "$WINDOW" _NET_WM_WINDOW_TYPE _NET_WM_STATE

echo "[3] click reaches the window underneath the crosshair"
xdotool mousemove --sync 640 360 click 1
for _ in $(seq 1 30); do
    grep -q '^CLICK ' "$ROOT/probe.log" && break
    sleep 0.05
done
grep '^CLICK ' "$ROOT/probe.log"

echo "[4] hide removes the window and exits the daemon"
"$BIN" --hide
sleep 0.4
"$BIN" --status
test ! -S "$XDG_RUNTIME_DIR/crosshair/ipc.sock"
! xdotool search --onlyvisible --name '^Crosshair$' >/dev/null 2>&1

echo "PASS: X11 ARGB, no-focus, empty input shape, pointer pass-through and clean OFF exit"
