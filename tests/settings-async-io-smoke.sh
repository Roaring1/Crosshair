#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
SETTINGS_PID=""
FAKE_PID=""
SETTINGS="$REPO_ROOT/target/debug/crosshair-settings"
BIN="$REPO_ROOT/target/debug/crosshair"

cleanup() {
    if [[ -n "$SETTINGS_PID" ]]; then
        kill "$SETTINGS_PID" 2>/dev/null || true
        wait "$SETTINGS_PID" 2>/dev/null || true
    fi
    if [[ -n "$FAKE_PID" ]]; then
        kill "$FAKE_PID" 2>/dev/null || true
        wait "$FAKE_PID" 2>/dev/null || true
    fi
    rm -rf "$ROOT"
}
trap cleanup EXIT

for binary in "$SETTINGS" "$BIN"; do
    if [[ ! -x "$binary" ]]; then
        echo "Build both Crosshair executables before running this Settings UI test." >&2
        exit 2
    fi
done

mkdir -p "$ROOT/home" "$ROOT/config/crosshair" "$ROOT/state" "$ROOT/data" "$ROOT/runtime/crosshair"
chmod 700 "$ROOT/runtime" "$ROOT/runtime/crosshair"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_BIN_HOME="$ROOT/bin"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

SOCKET="$XDG_RUNTIME_DIR/crosshair/ipc.sock"
ACCEPTED="$ROOT/ipc-accepted"
REPLIED="$ROOT/ipc-replied"
COUNT_FILE="$ROOT/ipc-count"
export SOCKET ACCEPTED REPLIED COUNT_FILE
printf '%s\n' '{"enabled":true,"activity_mode":"ui"}' > "$XDG_CONFIG_HOME/crosshair/config.json"

# The fake daemon withholds its reply long enough for a second UI action,
# but responds before notify_overlay's two-second read timeout.
python3 - <<'PY' &
import os
import socket
import time

path = os.environ["SOCKET"]
accepted = os.environ["ACCEPTED"]
replied = os.environ["REPLIED"]
count_file = os.environ["COUNT_FILE"]
request_count = 0
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
os.chmod(path, 0o600)
server.listen(8)
while True:
    connection, _ = server.accept()
    try:
        payload = connection.recv(65536)
        request_count += 1
        with open(count_file, "w", encoding="utf-8") as marker:
            marker.write(str(request_count))
        with open(accepted, "w", encoding="utf-8") as marker:
            marker.write("accepted")
        time.sleep(1.5)
        with open(replied, "w", encoding="utf-8") as marker:
            marker.write("replied")
        try:
            connection.sendall(b'{"ok":true}\n')
        except OSError:
            pass
    finally:
        connection.close()
PY
FAKE_PID=$!

for _ in $(seq 1 100); do
    [[ -S "$SOCKET" ]] && break
    sleep 0.02
done
test -S "$SOCKET"

echo "[1] compile the managed-close helper and launch Settings"
cc "$SCRIPT_DIR/send-wm-delete.c" -o "$ROOT/send-wm-delete" -lX11
xvfb-run -a bash -c '
    set -euo pipefail
    settings="$1"
    accepted="$2"
    close_helper="$3"
    RUST_BACKTRACE=1 "$settings" >"$4/settings.log" 2>&1 &
    pid=$!
    window=""
    for _ in $(seq 1 100); do
        window="$(xdotool search --onlyvisible --pid "$pid" 2>/dev/null | head -1 || true)"
        [[ -n "$window" ]] && break
        sleep 0.05
    done
    test -n "$window"
    initial_height="$(xdotool getwindowgeometry --shell "$window" | sed -n "s/^HEIGHT=//p")"
    # App Activity adds a persistent General row; the compact first-launch
    # window is now 348px tall rather than the legacy 280px.
    test "$initial_height" -ge 320
    test "$initial_height" -lt 400

    echo "[2] wait until Settings sends its initial enabled-overlay preview"
    for _ in $(seq 1 40); do
        [[ -f "$accepted" ]] && break
        sleep 0.05
    done
    test -f "$accepted"
    test "$(cat "$COUNT_FILE")" = "1"

    echo "[3] switch to Appearance while the IPC peer is withholding its response"
    xdotool mousemove --window "$window" 280 24 click 1
    sleep 0.2
    responsive_height="$(xdotool getwindowgeometry --shell "$window" | sed -n "s/^HEIGHT=//p")"
    if [[ "$responsive_height" -lt 500 ]]; then
        echo "Settings event loop did not process Appearance while IPC was blocked (height=$responsive_height)." >&2
        exit 1
    fi
    echo "PASS: Appearance resized the window to $responsive_height px while IPC was waiting"

    echo "[4] change App Activity while the original preview request remains in flight"
    xdotool mousemove --window "$window" 75 24 click 1
    sleep 0.2
    xdotool mousemove --window "$window" 390 120 click 1
    sleep 0.1
    xdotool mousemove --sync --window "$window" 160 168 click 1
    sleep 0.1
    test ! -f "$REPLIED"

    echo "[5] wait for the queued launch preference to persist without a second overlay preview"
    saved_mode=false
    for _ in $(seq 1 80); do
        if [[ -f "$REPLIED" ]] && grep -qi "\"activity_mode\": \"command\"" "$XDG_CONFIG_HOME/crosshair/config.json"; then
            saved_mode=true
            break
        fi
        sleep 0.05
    done
    if [[ "$saved_mode" != true ]]; then
        echo "App Activity change was not persisted after the delayed preview response." >&2
        cat "$4/settings.log" >&2 || true
        exit 1
    fi
    if [[ "$(cat "$COUNT_FILE")" != "1" ]]; then
        echo "Changing App Activity unexpectedly sent an additional overlay preview request (count=$(cat "$COUNT_FILE"))." >&2
        exit 1
    fi
    echo "PASS: App Activity persisted after delayed IPC without waking the overlay again"

    echo "[6] close Settings cleanly after the queued App Activity change"
    xprop -id "$window" WM_PROTOCOLS || true
    "$close_helper" "$window"
    for _ in $(seq 1 180); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "Settings did not exit after the bounded worker/close flush." >&2
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        exit 1
    fi
    if ! wait "$pid"; then
        cat "$4/settings.log"
        echo "Settings exited with an error after the delayed IPC request." >&2
        exit 1
    fi
    if [[ ! -f "$REPLIED" || "$(cat "$COUNT_FILE")" != "1" ]]; then
        echo "Delayed IPC did not complete exactly one overlay preview request." >&2
        cat "$4/settings.log" >&2 || true
        exit 1
    fi
    echo "PASS: Settings closed cleanly after the delayed App Activity update"
' _ "$SETTINGS" "$ACCEPTED" "$ROOT/send-wm-delete" "$ROOT"

python3 - <<'PY'
import json
import os
from pathlib import Path

config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
saved = json.loads(config.read_text(encoding="utf-8"))
assert saved["enabled"] is True
assert saved["activity_mode"] == "command"
print("PASS: closing Settings retained the enabled overlay and Command only launch mode")
PY

echo "PASS: a delayed IPC response does not freeze the Settings event loop"
echo "PASS: App Activity metadata changes are saved without an extra overlay preview"
