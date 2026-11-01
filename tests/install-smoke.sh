#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BIN="$REPO_ROOT/target/debug/crosshair"
ROOT="$(mktemp -d)"

cleanup() {
    "$BIN" --quit >/dev/null 2>&1 || true
    rm -rf "$ROOT"
}
trap cleanup EXIT

if [[ ! -x "$BIN" || ! -x "$REPO_ROOT/target/debug/crosshair-settings" ]]; then
    echo "Build both Crosshair executables before running the installer smoke test." >&2
    exit 2
fi

export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_DATA_HOME="$ROOT/data"
export XDG_STATE_HOME="$ROOT/state"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_BIN_HOME="$ROOT/bin with space % and \$cash"
mkdir -p "$HOME" "$XDG_CONFIG_HOME/crosshair" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

CONFIG="$XDG_CONFIG_HOME/crosshair/config.json"
AUTOSTART="$XDG_CONFIG_HOME/autostart/crosshair.desktop"
APP_ENTRY="$XDG_DATA_HOME/applications/crosshair.desktop"
printf '%s\n' '{"sentinel":"retain-me"}' > "$CONFIG"

echo "[1] explicit autostart installation"
"$BIN" --autostart
test -f "$AUTOSTART"
test -f "$APP_ENTRY"
python3 - <<'PY'
import os
import pathlib
import stat

entry = pathlib.Path(os.environ["XDG_CONFIG_HOME"]) / "autostart" / "crosshair.desktop"
executable_path = pathlib.Path(os.environ["XDG_BIN_HOME"]) / "crosshair"
app_entry = pathlib.Path(os.environ["XDG_DATA_HOME"]) / "applications" / "crosshair.desktop"
executable = str(executable_path)
content = entry.read_text(encoding="utf-8")
escaped = (
    executable.replace("\\", "\\\\")
    .replace('"', '\\\"')
    .replace("`", "\\`")
    .replace("$", "\\$")
    .replace("%", "%%")
)
assert f'Exec="{escaped}" --daemon --no-ui' in content, content
assert f'TryExec={executable.replace(chr(92), chr(92) * 2)}' in content, content
assert "NoDisplay=true" in content
assert stat.S_IMODE(entry.parent.stat().st_mode) == 0o700
assert stat.S_IMODE(entry.stat().st_mode) == 0o600
assert stat.S_IMODE(executable_path.stat().st_mode) == 0o700
assert stat.S_IMODE(app_entry.parent.stat().st_mode) == 0o700
assert stat.S_IMODE(app_entry.stat().st_mode) == 0o600
print("PASS: Desktop Entry escaping, owner-only executables and private XDG permissions")
PY

echo "[2] remove opt-in autostart"
"$BIN" --remove-autostart
test ! -e "$AUTOSTART"

echo "[3] ordinary install removes a previous opt-in autostart"
"$BIN" --autostart
test -f "$AUTOSTART"
"$BIN" --install
test ! -e "$AUTOSTART"
test -f "$APP_ENTRY"

echo "[4] refuse symlinked autostart and abort normal install before partial writes"
printf '%s\n' "outside-preserve" > "$ROOT/outside.desktop"
ln -s "$ROOT/outside.desktop" "$AUTOSTART"
if "$BIN" --remove-autostart >/dev/null 2>&1; then
    echo "Symlinked autostart entry was unexpectedly removed." >&2
    exit 1
fi
test -L "$AUTOSTART"
test "$(cat "$ROOT/outside.desktop")" = "outside-preserve"

# Remove the artifacts created by the earlier successful install. The next
# normal install must fail before it recreates any of them.
rm -f "$XDG_BIN_HOME/crosshair" "$XDG_BIN_HOME/crosshair-settings" "$APP_ENTRY"
if "$BIN" --install >/dev/null 2>&1; then
    echo "Normal install unexpectedly accepted a symlinked autostart entry." >&2
    exit 1
fi
test -L "$AUTOSTART"
test ! -e "$XDG_BIN_HOME/crosshair"
test ! -e "$XDG_BIN_HOME/crosshair-settings"
test ! -e "$APP_ENTRY"
test "$(cat "$ROOT/outside.desktop")" = "outside-preserve"
rm "$AUTOSTART"

echo "[5] reject app-menu symlink before replacing any install artifact"
ln -s "$ROOT/outside.desktop" "$APP_ENTRY"
if "$BIN" --install >/dev/null 2>&1; then
    echo "Normal install unexpectedly accepted a symlinked app-menu entry." >&2
    exit 1
fi
test -L "$APP_ENTRY"
test ! -e "$XDG_BIN_HOME/crosshair"
test ! -e "$XDG_BIN_HOME/crosshair-settings"
test "$(cat "$ROOT/outside.desktop")" = "outside-preserve"
rm "$APP_ENTRY"

echo "[6] uninstall preflights every target before removing any"
"$BIN" --autostart
test -f "$AUTOSTART"
test -f "$XDG_BIN_HOME/crosshair"
test -f "$XDG_BIN_HOME/crosshair-settings"
rm "$APP_ENTRY"
ln -s "$ROOT/outside.desktop" "$APP_ENTRY"
if "$BIN" --uninstall >/dev/null 2>&1; then
    echo "Uninstall unexpectedly accepted a symlinked app-menu entry." >&2
    exit 1
fi
test -f "$XDG_BIN_HOME/crosshair"
test -f "$XDG_BIN_HOME/crosshair-settings"
test -f "$AUTOSTART"
test -L "$APP_ENTRY"
test "$(cat "$ROOT/outside.desktop")" = "outside-preserve"
rm "$APP_ENTRY"

echo "[7] uninstall removes application files but retains config"
"$BIN" --uninstall
test ! -e "$AUTOSTART"
test ! -e "$XDG_BIN_HOME/crosshair"
test ! -e "$XDG_BIN_HOME/crosshair-settings"
test ! -e "$APP_ENTRY"
test "$(cat "$CONFIG")" = '{"sentinel":"retain-me"}'

echo "PASS: isolated install/autostart/uninstall lifecycle; config retained and uninstall preflighted"
