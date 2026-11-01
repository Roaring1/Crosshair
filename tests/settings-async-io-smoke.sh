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
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY

SOCKET="$XDG_RUNTIME_DIR/crosshair/ipc.sock"
ACCEPTED="$ROOT/ipc-accepted"
export SOCKET ACCEPTED
printf '%s\n' '{"enabled":false}' > "$XDG_CONFIG_HOME/crosshair/config.json"

# The fake daemon accepts the IPC payload but deliberately withholds its
# response longer than notify_overlay's two-second read timeout.
python3 - <<'PY' &
import os
import socket
import time

path = os.environ["SOCKET"]
accepted = os.environ["ACCEPTED"]
server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(path)
os.chmod(path, 0o600)
server.listen(8)
while True:
    connection, _ = server.accept()
    try:
        payload = connection.recv(65536)
        with open(accepted, "w", encoding="utf-8") as marker:
            marker.write("accepted")
        time.sleep(2.6)
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
    test "$initial_height" -lt 300

    echo "[2] enable the overlay; wait until the fake IPC peer accepts the request"
    xdotool mousemove --window "$window" 31 72 click 1
    for _ in $(seq 1 40); do
        [[ -f "$accepted" ]] && break
        sleep 0.05
    done
    test -f "$accepted"

    echo "[3] switch to Appearance while the IPC peer is withholding its response"
    xdotool mousemove --window "$window" 280 24 click 1
    sleep 0.2
    responsive_height="$(xdotool getwindowgeometry --shell "$window" | sed -n "s/^HEIGHT=//p")"
    if [[ "$responsive_height" -lt 400 ]]; then
        echo "Settings event loop did not process Appearance while IPC was blocked (height=$responsive_height)." >&2
        exit 1
    fi
    echo "PASS: Appearance resized the window to $responsive_height px while IPC was waiting"

    echo "[4] verify WM_DELETE_WINDOW and request a normal window close"
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
    echo "PASS: Settings closed cleanly after the delayed IPC request"
' _ "$SETTINGS" "$ACCEPTED" "$ROOT/send-wm-delete" "$ROOT"

python3 - <<'PY'
import json
import os
from pathlib import Path

config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
assert json.loads(config.read_text(encoding="utf-8"))["enabled"] is True
print("PASS: closing Settings flushed the final auto-saved Enabled state")
PY

echo "PASS: a delayed IPC response does not freeze the Settings event loop"
