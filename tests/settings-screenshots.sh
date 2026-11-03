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
click_at 315 24
capture "02-appearance-top"

echo "[3/6] Open the main color palette"
# Main color swatch: left pane, Color row.
click_at 122 265
capture "03-color-picker"

echo "[4/6] Select a preset and verify that it persists"
# Row 2, column 4 of the palette is #0ea5e9.
click_at 204 353
sleep 0.8
capture "04-color-selected"
grep -qi '#0ea5e9' "$XDG_CONFIG_HOME/crosshair/config.json"

echo "[5/7] Expand Outline to create real scrollable content"
click_at 220 322
sleep 0.35
capture "05-appearance-expanded-top"

echo "[6/7] Scroll the Appearance pane to its lower controls"
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 5
sleep 0.5
capture "06-appearance-lower"
if cmp -s "$OUT/05-appearance-expanded-top.png" "$OUT/06-appearance-lower.png"; then
    echo "Appearance ScrollView did not move when the wheel was used" >&2
    exit 1
fi

echo "[7/7] Return to the top to capture the full control layout"
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 4
sleep 0.5
capture "07-appearance-top-again"

xdotool windowclose "$WINDOW"
for _ in $(seq 1 100); do
    kill -0 "$SETTINGS_PID" 2>/dev/null || break
    sleep 0.05
done
! kill -0 "$SETTINGS_PID" 2>/dev/null

echo "PASS: captured Rust Settings screens and exited Settings cleanly"
