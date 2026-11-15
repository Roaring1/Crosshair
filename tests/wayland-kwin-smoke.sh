#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
CROSSHAIR_BIN="${CROSSHAIR_BIN:-$ROOT_DIR/target/debug/crosshair}"
if [[ "$CROSSHAIR_BIN" != /* ]]; then
    CROSSHAIR_BIN="$ROOT_DIR/$CROSSHAIR_BIN"
fi
if [[ ! -x "$CROSSHAIR_BIN" ]]; then
    echo "Crosshair binary not found or not executable: $CROSSHAIR_BIN" >&2
    echo "Build it first with: cargo build --locked --features settings-ui --bins" >&2
    exit 1
fi
for dependency in kwin_wayland wayland-info dbus-run-session; do
    if ! command -v "$dependency" >/dev/null 2>&1; then
        echo "Missing optional Wayland smoke-test dependency: $dependency" >&2
        exit 1
    fi
done

TEST_TMP="$(mktemp -d "${TMPDIR:-/tmp}/crosshair-wayland-smoke.XXXXXX")"
cleanup_outer() {
    rm -rf "$TEST_TMP"
}
trap cleanup_outer EXIT
mkdir -p "$TEST_TMP/home" "$TEST_TMP/runtime" "$TEST_TMP/config" \
    "$TEST_TMP/data" "$TEST_TMP/cache" "$TEST_TMP/state" "$TEST_TMP/kdehome"
chmod 700 "$TEST_TMP/runtime"

export TEST_TMP CROSSHAIR_BIN
export HOME="$TEST_TMP/home"
export XDG_RUNTIME_DIR="$TEST_TMP/runtime"
export XDG_CONFIG_HOME="$TEST_TMP/config"
export XDG_DATA_HOME="$TEST_TMP/data"
export XDG_CACHE_HOME="$TEST_TMP/cache"
export XDG_STATE_HOME="$TEST_TMP/state"
export KDEHOME="$TEST_TMP/kdehome"
export WAYLAND_DISPLAY=crosshair-kwin-smoke
export XDG_SESSION_TYPE=wayland
unset DISPLAY

dbus-run-session -- bash -s <<'INNER'
set -euo pipefail
KWIN_PID=""
CROSSHAIR_PID=""
cleanup_inner() {
    if [[ -n "$CROSSHAIR_PID" ]]; then
        "$CROSSHAIR_BIN" --quit >/dev/null 2>&1 || true
        kill -TERM "$CROSSHAIR_PID" 2>/dev/null || true
        wait "$CROSSHAIR_PID" 2>/dev/null || true
    fi
    if [[ -n "$KWIN_PID" ]]; then
        kill -TERM "$KWIN_PID" 2>/dev/null || true
        wait "$KWIN_PID" 2>/dev/null || true
    fi
}
trap cleanup_inner EXIT

# KWin's virtual backend creates a private virtual output. This compositor
# runs inside this test's DBus and XDG directories, not the user's session.
kwin_wayland --virtual --socket "$WAYLAND_DISPLAY" \
    --no-global-shortcuts --no-kactivities --no-lockscreen \
    >"$TEST_TMP/kwin.log" 2>&1 &
KWIN_PID=$!

ready=0
for _ in $(seq 1 100); do
    if [[ -S "$XDG_RUNTIME_DIR/$WAYLAND_DISPLAY" ]]; then
        ready=1
        break
    fi
    if ! kill -0 "$KWIN_PID" 2>/dev/null; then
        cat "$TEST_TMP/kwin.log" >&2
        exit 1
    fi
    sleep 0.1
done
if [[ "$ready" -ne 1 ]]; then
    cat "$TEST_TMP/kwin.log" >&2
    echo "KWin virtual Wayland socket did not become ready" >&2
    exit 1
fi

timeout 5s wayland-info >"$TEST_TMP/globals.txt"
if ! grep -q "zwlr_layer_shell_v1" "$TEST_TMP/globals.txt"; then
    cat "$TEST_TMP/globals.txt" >&2
    cat "$TEST_TMP/kwin.log" >&2
    echo "KWin does not advertise wlr-layer-shell" >&2
    exit 1
fi

"$CROSSHAIR_BIN" --list-monitors >"$TEST_TMP/monitors.json"
if ! grep -q "Virtual-0" "$TEST_TMP/monitors.json"; then
    cat "$TEST_TMP/monitors.json" >&2
    echo "Expected KWin's Virtual-0 output was not discovered" >&2
    exit 1
fi

"$CROSSHAIR_BIN" --set '{"enabled":true,"shape":"cross"}'
"$CROSSHAIR_BIN" --daemon >"$TEST_TMP/crosshair.log" 2>&1 &
CROSSHAIR_PID=$!
ready=0
for _ in $(seq 1 100); do
    if "$CROSSHAIR_BIN" --status >"$TEST_TMP/status.txt" 2>/dev/null \
        && grep -q "^Output:" "$TEST_TMP/status.txt"; then
        ready=1
        break
    fi
    if ! kill -0 "$CROSSHAIR_PID" 2>/dev/null; then
        cat "$TEST_TMP/crosshair.log" >&2
        cat "$TEST_TMP/status.txt" >&2 2>/dev/null || true
        exit 2
    fi
    sleep 0.1
done
if [[ "$ready" -ne 1 ]]; then
    cat "$TEST_TMP/crosshair.log" >&2
    cat "$TEST_TMP/status.txt" >&2 2>/dev/null || true
    echo "Crosshair did not finish layer-surface configuration" >&2
    exit 2
fi

# Allow the first configure event to render and then exercise a live redraw.
sleep 0.5
kill -0 "$CROSSHAIR_PID"
"$CROSSHAIR_BIN" --set '{"enabled":true,"shape":"dot_ring","size":44,"thickness":4}'
sleep 0.3
kill -0 "$CROSSHAIR_PID"
"$CROSSHAIR_BIN" --status >"$TEST_TMP/updated-status.txt"
grep -q "^Overlay: enabled" "$TEST_TMP/updated-status.txt"
grep -q "^Output: Virtual-0" "$TEST_TMP/updated-status.txt"

"$CROSSHAIR_BIN" --quit
if wait "$CROSSHAIR_PID"; then
    CROSSHAIR_PID=""
else
    echo "Crosshair exited unsuccessfully after --quit" >&2
    cat "$TEST_TMP/crosshair.log" >&2
    exit 3
fi

printf '%s\n' "PASS: KWin advertised wlr-layer-shell and Crosshair configured/rendered its overlay, applied a live appearance update, and shut down cleanly."
cat "$TEST_TMP/updated-status.txt"
INNER
