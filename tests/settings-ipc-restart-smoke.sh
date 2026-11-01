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
FIRST="$ROOT/first-request-accepted"
FIRST_REPLY="$ROOT/first-request-replied"
RESTARTED="$ROOT/replacement-listener-ready"
LATEST="$ROOT/latest-request-accepted"
REPORT="$ROOT/server-report.json"
export SOCKET FIRST FIRST_REPLY RESTARTED LATEST REPORT
printf '%s\n' '{"enabled":false}' > "$XDG_CONFIG_HOME/crosshair/config.json"

# Keep both listeners live, then atomically replace the socket pathname while
# the first request is in flight. This models a daemon restart without a gap
# that could cause Settings to launch a real daemon in the isolated test.
python3 - <<'PY' &
import json
import os
import socket
import time
from pathlib import Path

path = os.environ["SOCKET"]
replacement_path = path + ".next"

def listener(socket_path: str) -> socket.socket:
    result = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    result.bind(socket_path)
    os.chmod(socket_path, 0o600)
    result.listen(4)
    return result

old_server = listener(path)
new_server = listener(replacement_path)
first, _ = old_server.accept()
first_payload = json.loads(first.recv(65536).decode("utf-8"))["payload"]
Path(os.environ["FIRST"]).write_text("accepted", encoding="utf-8")
assert first_payload["enabled"] is True
# Allow several independent UI changes and debounce callbacks to queue.
time.sleep(1.5)
os.replace(replacement_path, path)
first.sendall(b'{"ok":true}\n')
first.close()
old_server.close()
Path(os.environ["FIRST_REPLY"]).write_text("replied", encoding="utf-8")
Path(os.environ["RESTARTED"]).write_text("ready", encoding="utf-8")

latest, _ = new_server.accept()
latest_payload = json.loads(latest.recv(65536).decode("utf-8"))["payload"]
assert latest_payload["enabled"] is True
latest.sendall(b'{"ok":true}\n')
latest.close()
new_server.close()
Path(os.environ["LATEST"]).write_text("accepted", encoding="utf-8")
Path(os.environ["REPORT"]).write_text(
    json.dumps({"first_enabled": first_payload["enabled"],
                "latest_enabled": latest_payload["enabled"],
                "replacement_requests": 1}),
    encoding="utf-8",
)
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

    echo "[1] start the first preview update and wait until the old listener accepts it"
    xdotool mousemove --window "$window" 31 72 click 1
    for _ in $(seq 1 100); do
        [[ -f "$FIRST" ]] && break
        sleep 0.05
    done
    test -f "$FIRST"

    echo "[2] make four rapid setting changes while the first IPC request is delayed"
    for _ in 1 2 3 4; do
        xdotool mousemove --window "$window" 31 72 click 1
        sleep 0.1
    done
    test ! -f "$FIRST_REPLY"

    echo "[3] wait for the replacement listener and the final, latest preview"
    for _ in $(seq 1 200); do
        [[ -f "$LATEST" ]] && break
        sleep 0.05
    done
    test -f "$FIRST_REPLY"
    test -f "$RESTARTED"
    test -f "$LATEST"
    python3 - <<"PY"
import json
import os
from pathlib import Path

report = json.loads(Path(os.environ["REPORT"]).read_text(encoding="utf-8"))
assert report == {"first_enabled": True, "latest_enabled": True, "replacement_requests": 1}
config = Path(os.environ["XDG_CONFIG_HOME"]) / "crosshair" / "config.json"
assert json.loads(config.read_text(encoding="utf-8"))["enabled"] is True
print("PASS: stale intermediate previews were skipped and only the latest update reached the restarted listener")
PY

    echo "[4] close Settings after the latest update has persisted"
    "$close_helper" "$window"
    for _ in $(seq 1 180); do
        kill -0 "$pid" 2>/dev/null || break
        sleep 0.05
    done
    if kill -0 "$pid" 2>/dev/null; then
        echo "Settings did not close after the simulated daemon restart." >&2
        kill "$pid" 2>/dev/null || true
        wait "$pid" 2>/dev/null || true
        exit 1
    fi
    if ! wait "$pid"; then
        cat "$root/settings.log"
        echo "Settings exited with an error after the simulated daemon restart." >&2
        exit 1
    fi
    echo "PASS: Settings closed cleanly after rapid edits during daemon restart"
' _ "$SETTINGS" "$ROOT/send-wm-delete" "$ROOT"
