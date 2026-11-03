#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
OUT="${CROSSHAIR_SCREENSHOT_DIR:-$REPO_ROOT/artifacts/settings-ui}"
BIN="${CROSSHAIR_BIN:-$REPO_ROOT/target/debug/crosshair}"
SETTINGS="${CROSSHAIR_SETTINGS_BIN:-$REPO_ROOT/target/debug/crosshair-settings}"
SETTINGS_PID=""
WINDOW=""

cleanup() {
    "$BIN" --quit >/dev/null 2>&1 || true
    if [[ -n "$WINDOW" ]]; then
        xdotool windowclose "$WINDOW" >/dev/null 2>&1 || true
    fi
    if [[ -n "$SETTINGS_PID" ]]; then
        kill "$SETTINGS_PID" 2>/dev/null || true
        wait "$SETTINGS_PID" 2>/dev/null || true
    fi
    rm -rf "$ROOT"
}
trap cleanup EXIT

for command in xdotool import identify xsetroot; do
    command -v "$command" >/dev/null || {
        echo "Required screenshot command is missing: $command" >&2
        exit 2
    }
done

test -x "$BIN"
test -x "$SETTINGS"
mkdir -p "$OUT"
rm -f "$OUT"/*.png "$OUT"/manifest.txt

mkdir -p "$ROOT/home" "$ROOT/config" "$ROOT/state" "$ROOT/data" "$ROOT/runtime"
chmod 700 "$ROOT/runtime"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

xsetroot -solid "#171a22"
"$BIN" --show
"$SETTINGS" >"$ROOT/settings.log" 2>&1 &
SETTINGS_PID=$!

for _ in $(seq 1 120); do
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
xdotool windowmove "$WINDOW" 40 40
xdotool windowfocus "$WINDOW" || true
sleep 0.5

eval "$(xdotool getwindowgeometry --shell "$WINDOW")"
click_at() {
    xdotool mousemove --sync "$((X + $1))" "$((Y + $2))"
    xdotool click 1
    sleep 0.35
}
capture() {
    local name="$1"
    sleep 0.35
    import -window root "$OUT/$name.png"
    test -s "$OUT/$name.png"
    local dimensions
    dimensions="$(identify -format '%wx%h' "$OUT/$name.png")"
    test "$dimensions" = "1280x900"
    echo "$name.png $dimensions $(stat -c '%s bytes' "$OUT/$name.png")" | tee -a "$OUT/manifest.txt"
}

echo "[1/5] Capture General"
capture "01-general"

echo "[2/5] Open Appearance"
click_at 145 24
capture "02-appearance-top"

echo "[3/5] Open the main color palette"
# The main color swatch sits in the first color row in the left Appearance pane.
click_at 122 250
capture "03-color-picker"

echo "[4/5] Dismiss the palette and inspect the lower scrolling content"
click_at 560 360
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 5
sleep 0.4
capture "04-appearance-lower"

echo "[5/5] Return to the top to capture the full control layout"
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 4
sleep 0.4
capture "05-appearance-top-again"

xdotool windowclose "$WINDOW"
for _ in $(seq 1 100); do
    kill -0 "$SETTINGS_PID" 2>/dev/null || break
    sleep 0.05
done
! kill -0 "$SETTINGS_PID" 2>/dev/null

echo "PASS: captured Rust Settings screens and exited Settings cleanly"
