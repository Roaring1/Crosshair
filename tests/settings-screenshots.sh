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
# Tab selection must be derived from one mutually exclusive state, rather
# than two independently toggled/checkable buttons that can both stay active.
grep -Fq 'primary: root.active-tab == 0;' "$REPO_ROOT/ui/settings.slint"
grep -Fq 'primary: root.active-tab == 1;' "$REPO_ROOT/ui/settings.slint"
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
mkdir -p "$XDG_CONFIG_HOME/crosshair"
printf '%s\n' '{"enabled":false,"activity_mode":"command"}' > "$XDG_CONFIG_HOME/crosshair/config.json"
QUIET_OUTPUT="$("$BIN" 2>&1)"
test -z "$QUIET_OUTPUT"
test ! -e "$XDG_RUNTIME_DIR/crosshair.sock"
# Explicit CLI control remains usable in Command only mode.
"$BIN" --show
# Explicit Settings launch also remains available in Command only mode.
"$BIN" --config

for _ in $(seq 1 120); do
    WINDOW="$(xdotool search --onlyvisible --name '^Crosshair Settings$' 2>/dev/null | head -1 || true)"
    if [[ -n "$WINDOW" ]]; then
        SETTINGS_PID="$(xdotool getwindowpid "$WINDOW" 2>/dev/null || true)"
        if [[ -n "$SETTINGS_PID" ]]; then break; fi
    fi
    sleep 0.05
done
test -n "$SETTINGS_PID"
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
window_height() {
    xdotool getwindowgeometry --shell "$WINDOW" | awk -F= '$1 == "HEIGHT" { print $2 }'
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

echo "[1/8] Capture General"
capture "01-general"
INITIAL_WINDOW_HEIGHT="$(window_height)"
test -n "$INITIAL_WINDOW_HEIGHT"

echo "[2/8] Inspect and change App Activity preference"
click_at 390 120
capture "02-app-activity-options"
# Select App UI from the menu, starting from Command only. The explicit --config
# path above must have opened Settings despite the quiet default launch.
click_at 350 128
sleep 0.8
grep -qi '"activity_mode": "ui"' "$XDG_CONFIG_HOME/crosshair/config.json"

echo "[3/8] Open Appearance"
click_at 315 24
test "$(window_height)" = "$INITIAL_WINDOW_HEIGHT"
capture "03-appearance-top"

echo "[4/8] Open the main color palette"
# Main color swatch: left pane, Color row.
click_at 114 272
capture "04-color-picker"

echo "[5/8] Select a preset and verify that it persists"
# Row 3, column 4 of the palette is #ec4899.
click_at 208 396
sleep 0.8
capture "05-color-selected"
grep -qi '#ec4899' "$XDG_CONFIG_HOME/crosshair/config.json"

echo "[6/8] Expand Outline to create real scrollable content"
click_at 220 320
sleep 0.35
capture "06-appearance-expanded-top"

echo "[7/8] Scroll the Appearance pane to its lower controls"
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 5
sleep 0.5
capture "07-appearance-lower"
if cmp -s "$OUT/06-appearance-expanded-top.png" "$OUT/07-appearance-lower.png"; then
    echo "Appearance ScrollView did not move when the wheel was used" >&2
    exit 1
fi

echo "[8/8] Return to the top to capture the full control layout"
xdotool mousemove --sync "$((X + 280))" "$((Y + 420))"
xdotool click --repeat 7 --delay 45 4
sleep 0.5
capture "08-appearance-top-again"

xdotool windowclose "$WINDOW"
for _ in $(seq 1 100); do
    kill -0 "$SETTINGS_PID" 2>/dev/null || break
    sleep 0.05
done
! kill -0 "$SETTINGS_PID" 2>/dev/null

echo "PASS: Command only stayed quiet, explicit --show/--config remained available, App Activity choices rendered, Settings screenshots/scrolling passed, and Settings exited cleanly"
