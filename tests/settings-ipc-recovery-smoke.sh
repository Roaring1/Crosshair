#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "$0")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
SETTINGS_PID=""
FAKE_PID=""
SETTINGS="$REPO_ROOT/target/debug/crosshair-settings"

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

if [[ ! -x "$SETTINGS" ]]; then
    echo "Build the Crosshair Settings executable before running this test." >&2
    exit 2
fi

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
FIRST="$ROOT/first-request-failed"
RECOVERED="$ROOT/recovery-request-succeeded"
export SOCKET FIRST RECOVERED
printf '%s\n' '{"enabled":false}' > "$XDG_CONFIG_HOME/crosshair/config.json"

# The first client is disconnected without a response. Subsequent clients
# receive a valid response, proving the Settings worker remains reusable.
python3 - <<'PY' &
import os
import socket
from pathlib import Path

server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(os.environ["SOCKET"])
os.chmod(os.environ["SOCKET"], 0o600)
server.listen(8)
server.settimeout(20)
count = 0
while True:
    connection, _ = server.accept()
    count += 1
    try:
        connection.recv(65536)
        if count == 1:
            Path(os.environ["FIRST"]).write_text("closed", encoding="utf-8")
            # Simulate a daemon crash after accepting a request but before
            # returning its acknowledgement.
            connection.close()
            continue
        connection.sendall(b'{"ok":true}\n')
        Path(os.environ["RECOVERED"]).write_text(str(count), encoding="utf-8")
    finally:
        connection.close()
PY
FAKE_PID=$!

for _ in $(seq 1 100); do
    [[ -S "$SOCKET" ]] && break
    sleep 0.02
done
test -S "$SOCKET"

cc "$SCRIPT_DIR/send-wm-delete.c" -o "$ROOT/send-wm-delete" -lX11
xvfb-run -a bash -c '
    set -euo pipefail
    settings="$1"
    close_helper="$2"
    root="$3"
    RUST_BACKTRACE=1 "$settings" >"$root/settings.log" 2>&1 &
    pid=$!
    window=""
    for _ in $(seq 1 100); do
        window="$(xdotool search --onlyvisible --pid "$pid" 2>/dev/null | head -1 || true)"
        [[ -n "$window" ]] && break
        sleep 0.05
    done
    test -n "$window"

    echo "[1] trigger an update against a peer that drops its first connection"
    xdotool mousemove --window "$window" 31 72 click 1
    for _ in $(seq 1 100); do
        [[ -f "$FIRST" ]] && break
        sleep 0.05
    done
    test -f "$FIRST"
    sleep 0.25
    python3 - <<"PY"
import json
import os
from pathlib import Path

config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
assert json.loads(config.read_text(encoding="utf-8"))["enabled"] is False
print("PASS: a dropped IPC acknowledgement did not persist an un-applied update")
PY

    echo "[2] toggle twice to submit a fresh update after the peer is healthy"
    xdotool mousemove --window "$window" 31 72 click 1
    sleep 0.02
    xdotool mousemove --window "$window" 31 72 click 1
    for _ in $(seq 1 200); do
        if [[ -f "$RECOVERED" ]] && python3 - <<"PY" 2>/dev/null
import json
import os
from pathlib import Path
config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
raise SystemExit(0 if json.loads(config.read_text(encoding="utf-8"))["enabled"] else 1)
PY
        then
            break
        fi
        sleep 0.05
    done
    test -f "$RECOVERED"
    python3 - <<"PY"
import json
import os
from pathlib import Path
config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
assert json.loads(config.read_text(encoding="utf-8"))["enabled"] is True
print("PASS: the Settings worker recovered and persisted the later successful update")
PY

    echo "[3] close Settings normally after IPC recovery"
    "$close_helper" "$(printf '%s' "$window")"
    for _ in $(seq 1 180); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "Settings did not close after IPC recovery." >&2
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        exit 1
    fi
    if ! wait "$pid"; then
        cat "$root/settings.log"
        echo "Settings exited with an error after IPC recovery." >&2
        exit 1
    fi
    echo "PASS: Settings closed cleanly after recovering from the dropped IPC response"
' _ "$SETTINGS" "$ROOT/send-wm-delete" "$ROOT"
