#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "$SCRIPT_DIR/.." && pwd)"
BIN="${CROSSHAIR_BIN:-$REPO_ROOT/target/debug/crosshair}"
SETTINGS_BIN="${CROSSHAIR_SETTINGS_BIN:-$REPO_ROOT/target/debug/crosshair-settings}"
ROOT="$(mktemp -d)"

cleanup() {
    "$BIN" --quit >/dev/null 2>&1 || true
    rm -rf "$ROOT"
}
trap cleanup EXIT

if [[ ! -x "$BIN" || ! -x "$SETTINGS_BIN" ]]; then
    echo "Build both Crosshair executables before running the installer smoke test." >&2
    exit 2
fi

export HOME="$ROOT/home"
export XDG_CONFIG_HOME="$ROOT/config"
export XDG_DATA_HOME="$ROOT/data"
export XDG_STATE_HOME="$ROOT/state"
export XDG_RUNTIME_DIR="$ROOT/runtime"
export XDG_BIN_HOME="$ROOT/bin with space % and \$cash"

echo "[0] CLI help and version work without creating user directories"
HELP_ROOT="$ROOT/help"
HELP_OUTPUT="$(env HOME="$HELP_ROOT/home" XDG_CONFIG_HOME="$HELP_ROOT/config" XDG_DATA_HOME="$HELP_ROOT/data" XDG_STATE_HOME="$HELP_ROOT/state" XDG_RUNTIME_DIR="$HELP_ROOT/runtime" "$BIN" --help)"
grep -Fq "USAGE:" <<<"$HELP_OUTPUT"
grep -Fq -- "--toggle" <<<"$HELP_OUTPUT"
SHORT_HELP_OUTPUT="$(env HOME="$HELP_ROOT/home" XDG_CONFIG_HOME="$HELP_ROOT/config" XDG_DATA_HOME="$HELP_ROOT/data" XDG_STATE_HOME="$HELP_ROOT/state" XDG_RUNTIME_DIR="$HELP_ROOT/runtime" "$BIN" -h)"
grep -Fq "USAGE:" <<<"$SHORT_HELP_OUTPUT"
VERSION_OUTPUT="$(env HOME="$HELP_ROOT/home" XDG_CONFIG_HOME="$HELP_ROOT/config" XDG_DATA_HOME="$HELP_ROOT/data" XDG_STATE_HOME="$HELP_ROOT/state" XDG_RUNTIME_DIR="$HELP_ROOT/runtime" "$BIN" --version)"
test "$VERSION_OUTPUT" = "Crosshair 4.0.0"
test ! -e "$HELP_ROOT"
echo "PASS: --help, -h and --version are filesystem-free"

mkdir -p "$HOME" "$XDG_CONFIG_HOME/crosshair" "$XDG_DATA_HOME" "$XDG_STATE_HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"

CONFIG="$XDG_CONFIG_HOME/crosshair/config.json"
AUTOSTART="$XDG_CONFIG_HOME/autostart/crosshair.desktop"
APP_ENTRY="$XDG_DATA_HOME/applications/crosshair.desktop"
APP_ICON="$XDG_DATA_HOME/icons/hicolor/scalable/apps/crosshair.svg"
printf '%s\n' '{"sentinel":"retain-me"}' > "$CONFIG"

echo "[1] explicit autostart installation"
"$BIN" --autostart
test -f "$AUTOSTART"
test -f "$APP_ENTRY"
test -f "$APP_ICON"
grep -Fq "Icon=crosshair" "$APP_ENTRY"
cmp -s "$APP_ICON" "$REPO_ROOT/crosshair.svg"
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
app_content = app_entry.read_text(encoding="utf-8")
assert f'Exec="{escaped}"' in app_content, app_content
assert "--config" not in app_content, app_content
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
test -f "$APP_ICON"
grep -Fq "Icon=crosshair" "$APP_ENTRY"

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
test -f "$APP_ICON"
rm "$APP_ENTRY"
ln -s "$ROOT/outside.desktop" "$APP_ENTRY"
if "$BIN" --uninstall >/dev/null 2>&1; then
    echo "Uninstall unexpectedly accepted a symlinked app-menu entry." >&2
    exit 1
fi
test -f "$XDG_BIN_HOME/crosshair"
test -f "$XDG_BIN_HOME/crosshair-settings"
test -f "$AUTOSTART"
test -f "$APP_ICON"
test -L "$APP_ENTRY"
test "$(cat "$ROOT/outside.desktop")" = "outside-preserve"
rm "$APP_ENTRY"

echo "[6] uninstall refuses a symlinked icon before removing any other artifact"
mv "$APP_ICON" "$ROOT/original-crosshair.svg"
ln -s "$ROOT/outside.desktop" "$APP_ICON"
if "$BIN" --uninstall >/dev/null 2>&1; then
    echo "Uninstall unexpectedly accepted a symlinked app icon." >&2
    exit 1
fi
test -f "$XDG_BIN_HOME/crosshair"
test -f "$XDG_BIN_HOME/crosshair-settings"
test -f "$AUTOSTART"
test -L "$APP_ICON"
rm "$APP_ICON"
mv "$ROOT/original-crosshair.svg" "$APP_ICON"

echo "[7] command-only launch stays quiet without disabling explicit CLI commands"
MODE_ROOT="$ROOT/command-only-mode"
mkdir -p "$MODE_ROOT/home" "$MODE_ROOT/config/crosshair" "$MODE_ROOT/data" "$MODE_ROOT/state" "$MODE_ROOT/runtime"
chmod 700 "$MODE_ROOT/runtime"
printf '%s\n' '{"activity_mode":"command","enabled":false}' > "$MODE_ROOT/config/crosshair/config.json"
MODE_OUTPUT="$(env HOME="$MODE_ROOT/home" XDG_CONFIG_HOME="$MODE_ROOT/config" XDG_DATA_HOME="$MODE_ROOT/data" XDG_STATE_HOME="$MODE_ROOT/state" XDG_RUNTIME_DIR="$MODE_ROOT/runtime" "$BIN" 2>&1)"
test -z "$MODE_OUTPUT"
MODE_HELP="$(env HOME="$MODE_ROOT/home" XDG_CONFIG_HOME="$MODE_ROOT/config" XDG_DATA_HOME="$MODE_ROOT/data" XDG_STATE_HOME="$MODE_ROOT/state" XDG_RUNTIME_DIR="$MODE_ROOT/runtime" "$BIN" --help)"
grep -Fq -- "--toggle" <<<"$MODE_HELP"
test ! -e "$MODE_ROOT/runtime/crosshair.sock"
echo "PASS: command-only plain launch is quiet; explicit CLI remains available"

echo "[8] uninstall removes application files but retains config"
"$BIN" --uninstall
test ! -e "$AUTOSTART"
test ! -e "$XDG_BIN_HOME/crosshair"
test ! -e "$XDG_BIN_HOME/crosshair-settings"
test ! -e "$APP_ENTRY"
test ! -e "$APP_ICON"
test "$(cat "$CONFIG")" = '{"sentinel":"retain-me"}'

echo "PASS: isolated install/autostart/uninstall lifecycle; config retained and uninstall preflighted"
