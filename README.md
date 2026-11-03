> **Development-branch notice:** This branch contains the native Rust rewrite. The established Python application remains on `main`; the Rust package is versioned `4.0.0` in Cargo but is still an unreleased draft. Do not treat this branch as a published release or as a reason to remove an existing Python installation. See [RUST-PORT-STATUS.md](RUST-PORT-STATUS.md) for the implementation comparison and release gates.

# Crosshair

GTK3 crosshair overlay for Linux — Wayland (via GTK Layer Shell on
Plasma/wlroots compositors) and X11.

Draws a persistent crosshair on top of all windows. Useful for games that
don't ship a built-in crosshair, or any application where a fixed center
reference helps.

**v3.2.0** is a security/reliability hardening pass over the original
v3.1 build. See `CHANGELOG.md` for the full list of fixes and
`RUNBOOK.md` for how to validate this on your machine before trusting it.

---

## Features

- Shapes: dot, cross, x, plus, ring/circle, dot+ring, cross+dot, x+dot
- Fully adjustable: size, gap, thickness, color, opacity, outline, shadow
- Per-monitor targeting: all / primary / active (follows cursor) / by
  index / by connector name
- Tray icon (Ayatana AppIndicator, falling back to legacy AppIndicator3,
  falling back to `Gtk.StatusIcon`) with toggle/restart/quit while the
  overlay daemon is active
- Local IPC socket for scripted control (hotkeys, `--status`, `--toggle`,
  `--show`, `--hide`, `--set`, `--reload`, `--restart`, `--quit`)
- Click-through on both backends — the overlay never intercepts input
- Config persists to `$XDG_CONFIG_HOME/crosshair/config.json` (atomic
  writes; a corrupted file is preserved as a timestamped backup, not
  silently discarded)

---

## Requirements

- Python 3.10+
- GTK3 + PyGObject + pycairo
- **GTK Layer Shell** — mandatory on native Wayland (KDE Plasma
  Wayland, wlroots compositors). Without it, the app refuses to start
  under Wayland rather than drawing a window that can't guarantee
  placement.
- An AppIndicator backend (Ayatana or legacy) — optional; the tray
  falls back to `Gtk.StatusIcon` without it, and its absence never
  blocks the overlay.

```bash
# Fedora / Nobara
sudo dnf install python3-gobject python3-cairo gtk3 gtk-layer-shell \
    libayatana-appindicator-gtk3 xdg-utils desktop-file-utils

# Arch
sudo pacman -S python-gobject python-cairo gtk3 gtk-layer-shell \
    libayatana-appindicator xdg-utils desktop-file-utils

# Ubuntu / Debian
sudo apt install python3-gi python3-gi-cairo gir1.2-gtk-3.0 \
    gir1.2-gtklayershell-0.1 gir1.2-ayatanaappindicator3-0.1 \
    xdg-utils desktop-file-utils
```

Check what's actually available on your system:

```bash
python3 crosshair.py --doctor
```

`--doctor` exits non-zero if a *mandatory* dependency is missing
(GTK3/Gdk/pycairo always; GTK Layer Shell when the actual GDK backend is
Wayland). It checks the live GDK display first and uses session/environment
hints only when no display can be queried. Tray backends are reported but
never mandatory.

---

## Usage

```bash
python3 crosshair.py            # launch GUI (starts daemon if not running)
python3 crosshair.py --status   # print live daemon status
python3 crosshair.py --toggle   # cold start: show; running daemon: toggle
python3 crosshair.py --show
python3 crosshair.py --hide
python3 crosshair.py --set '{"color": "#00ff88", "size": 8}'
python3 crosshair.py --reload   # reload config.json from disk
python3 crosshair.py --restart  # wait for a fresh daemon to become IPC-ready
python3 crosshair.py --quit     # idempotent -- safe to call when not running
```

`--set` takes a strict JSON object: unknown keys, wrong types, and
out-of-range values are rejected outright (not silently clamped), and
the command's exit code and stderr message tell you why.

Crosshair is on-demand by default. No daemon is started at login, no settings
widgets are built for the Padfire/CLI path, and no background process remains
after the overlay is hidden and Settings is closed. A cold `--toggle` activates
the overlay; while the daemon is running, `--toggle` flips it. Hiding it saves
the state and shuts the daemon down after a short grace period when Settings
is closed. The daemon is required to maintain the active overlay; its IPC listener blocks instead
of polling. Fixed-monitor mode has no recurring monitor scan, and external
config edits are loaded only on an explicit `--reload`.

The settings window is created only when explicitly requested with
`--config` or by opening the app-menu entry. Closing it while the overlay is
hidden also allows the daemon to exit. Login autostart remains available as
an explicit opt-in via `--autostart`, but is not installed by default.

### Wayland hotkeys

Wayland compositors generally don't allow apps to grab global hotkeys.
On KDE, bind a custom shortcut (System Settings → Shortcuts → Custom
Shortcuts) to a command instead:

```
~/.local/bin/crosshair --toggle
```

---

## Install / autostart / uninstall

```bash
python3 crosshair.py --install           # on-demand launcher + app-menu entry
python3 crosshair.py --autostart         # explicitly opt into login autostart
python3 crosshair.py --desktop-shortcut  # double-click icon on the Desktop

python3 crosshair.py --uninstall         # removes the above, keeps your config
python3 crosshair.py --purge-config      # explicitly removes the config too
```

The normal `--install` path removes a stale Crosshair XDG autostart entry
so an older installation does not leave a daemon running at login. Use
`--autostart` only to opt back in; it writes an XDG `.desktop` entry and
does not create a systemd unit. Legacy pre-3.2 systemd user units are still
detected and removed by the existing migration/uninstall path.

---

## Paths

| Purpose | Path |
| --- | --- |
| Config | `${XDG_CONFIG_HOME:-~/.config}/crosshair/config.json` |
| Socket | `$XDG_RUNTIME_DIR/crosshair/ipc.sock` (mode 0600) |
| Lock | `$XDG_RUNTIME_DIR/crosshair/daemon.lock` |
| Log | `${XDG_STATE_HOME:-~/.local/state}/crosshair/crosshair.log` |
| Installed script | `${XDG_DATA_HOME:-~/.local/share}/crosshair/crosshair.py` |
| Installed core | `${XDG_DATA_HOME:-~/.local/share}/crosshair/crosshair_core.py` |
| Launcher | `~/.local/bin/crosshair` |
| App-menu entry | `${XDG_DATA_HOME:-~/.local/share}/applications/crosshair.desktop` |
| App icon | `${XDG_DATA_HOME:-~/.local/share}/icons/hicolor/scalable/apps/crosshair.svg` |
| Desktop shortcut | `$(xdg-user-dir DESKTOP)/crosshair.desktop` |
| Optional autostart entry | `${XDG_CONFIG_HOME:-~/.config}/autostart/crosshair.desktop` |

If `$XDG_RUNTIME_DIR` isn't set or valid, a private cache-backed
fallback is used and `--doctor` will warn about it. On a normal desktop
login this shouldn't happen.

---

## Troubleshooting

1. `crosshair --doctor`
2. `crosshair --status`
3. `tail -n 200 "${XDG_STATE_HOME:-$HOME/.local/state}/crosshair/crosshair.log"`
4. If the daemon seems stuck and `--quit` doesn't respond, inspect the
   lock holder with `lslocks`/`lsof` and check `/proc/<pid>/cmdline`
   before doing anything manual — don't signal a process just because
   its name looks like `crosshair.py`. See `RUNBOOK.md` for the full
   recovery procedure.

---

## Development

Use a virtual environment so test/lint tools do not modify the system Python:

```bash
python3 -m venv .venv
. .venv/bin/activate
python -m pip install --requirement requirements-dev.txt
ruff check .
bandit -q -r crosshair.py crosshair_core.py -ll
python -m compileall -q crosshair.py crosshair_core.py tests
python -m pytest -q
```

The dependency audit uses a separate Python 3.14 environment because the patched `filelock==4.2.1` release requires Python 3.11 or newer:

```bash
python3.14 -m venv .audit-venv
.audit-venv/bin/python -m pip install --requirement requirements-audit.txt
.audit-venv/bin/pip-audit -r requirements-audit.txt
```

GitHub Actions runs the pinned lint, medium/high Bandit security scan, compile, and test checks on Python 3.10–3.14, then runs the dependency audit in a separate Python 3.14 job. This avoids installing a vulnerable older `filelock` into the Python 3.10 test environment. Dependabot checks the pinned actions and development/audit dependencies weekly.

`crosshair_core.py` is a GTK-free module (paths, atomic config I/O,
the single-instance lock, IPC protocol, and bounded GTK-dispatch helper).
Its regression suite is in `tests/test_core.py`; CLI/install/startup
regressions are in `tests/test_cli.py`. `crosshair.py` is the GTK
application that imports it. See `RUNBOOK.md` for the full machine-level release
gate, including what's already been verified in a sandboxed headless
X11 environment versus what still needs a real Wayland/KDE session.
