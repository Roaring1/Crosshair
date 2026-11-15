#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
ROOT="$(mktemp -d)"
FAKE_PID=""
BIN="${CROSSHAIR_BIN:-$REPO_ROOT/target/debug/crosshair}"

cleanup() {
    if [[ -n "$FAKE_PID" ]]; then
        kill "$FAKE_PID" 2>/dev/null || true
        wait "$FAKE_PID" 2>/dev/null || true
    fi
    rm -rf "$ROOT"
}
trap cleanup EXIT

test -x "$BIN"
mkdir -p "$ROOT/home" "$ROOT/config/crosshair" "$ROOT/state" "$ROOT/data" "$ROOT/runtime/crosshair"
chmod 700 "$ROOT/runtime" "$ROOT/runtime/crosshair"
export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_STATE_HOME="$ROOT/state"
export XDG_DATA_HOME="$ROOT/data"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_SESSION_TYPE=x11
unset WAYLAND_DISPLAY DISPLAY

SOCKET="$XDG_RUNTIME_DIR/crosshair/ipc.sock"
REQUESTS="$ROOT/requests.log"
export SOCKET REQUESTS
printf '%s\n' '{"enabled":true}' > "$XDG_CONFIG_HOME/crosshair/config.json"

# This fake peer accepts IPC requests but deliberately withholds each reply
# longer than the CLI's two-second deadline. It models a daemon that is alive
# but unresponsive, which must not be mistaken for a cleanly absent daemon.
python3 - <<'PY' &
import json
import os
import socket
import time
from pathlib import Path

server = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
server.bind(os.environ["SOCKET"])
os.chmod(os.environ["SOCKET"], 0o600)
server.listen(8)
server.settimeout(20)
try:
    while True:
        connection, _ = server.accept()
        with connection:
            payload = bytearray()
            while not payload.endswith(b"\n"):
                chunk = connection.recv(65536)
                if not chunk:
                    break
                payload.extend(chunk)
            try:
                command = json.loads(payload.decode("utf-8"))["cmd"]
            except Exception:
                command = "invalid"
            with Path(os.environ["REQUESTS"]).open("a", encoding="utf-8") as log:
                log.write(command + "\n")
                log.flush()
            time.sleep(2.5)
            try:
                connection.sendall(b'{"ok":true}\n')
            except OSError:
                pass
finally:
    server.close()
PY
FAKE_PID=$!

for _ in $(seq 1 100); do
    [[ -S "$SOCKET" ]] && break
    sleep 0.02
done
test -S "$SOCKET"

echo "[1] hide saves OFF but must report an unresponsive live daemon"
set +e
HIDE_OUTPUT="$("$BIN" --hide 2>&1)"
HIDE_STATUS=$?
set -e
if [[ "$HIDE_STATUS" -eq 0 ]]; then
    echo "--hide incorrectly reported success without an IPC acknowledgement." >&2
    exit 1
fi
grep -q "did not acknowledge shutdown" <<<"$HIDE_OUTPUT"
python3 - "$XDG_CONFIG_HOME/crosshair/config.json" <<'PY'
import json
import sys
from pathlib import Path
config = json.loads(Path(sys.argv[1]).read_text(encoding="utf-8"))
assert config["enabled"] is False, config
print("PASS: OFF preference persisted, but shutdown was not falsely reported as successful")
PY

echo "[2] status must report uncertainty, not claim the daemon is absent"
set +e
STATUS_OUTPUT="$("$BIN" --status 2>&1)"
STATUS_STATUS=$?
set -e
if [[ "$STATUS_STATUS" -eq 0 ]]; then
    echo "--status incorrectly reported a definite state for an unresponsive IPC peer." >&2
    exit 1
fi
grep -q "Could not determine whether the Crosshair daemon is still running" <<<"$STATUS_OUTPUT"
echo "PASS: an unresponsive daemon is not misreported as OFF"

echo "[3] toggle must not cold-start a second daemon when IPC is unresponsive"
set +e
TOGGLE_OUTPUT="$("$BIN" --toggle 2>&1)"
TOGGLE_STATUS=$?
set -e
if [[ "$TOGGLE_STATUS" -eq 0 ]]; then
    echo "--toggle incorrectly treated an unresponsive daemon as absent." >&2
    exit 1
fi
grep -q "may still be running but did not acknowledge toggle" <<<"$TOGGLE_OUTPUT"
# Give a mistaken cold-start fallback enough time to enqueue its first
# status poll; the fake peer will record it after the pending toggle request.
sleep 3
python3 - "$REQUESTS" <<'PY'
import sys
from pathlib import Path
requests = Path(sys.argv[1]).read_text(encoding="utf-8").splitlines()
assert requests == ["hide", "status", "toggle"], requests
print("PASS: no duplicate daemon startup/status polling was attempted after the unresponsive toggle")
PY

echo "PASS: CLI hide/status/toggle distinguish a stopped daemon from a live but unresponsive IPC endpoint."
