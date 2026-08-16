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
  falling back to `Gtk.StatusIcon`) with toggle/restart/quit
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
(GTK3/Gdk/pycairo always; GTK Layer Shell only when the session is
Wayland). Tray backends are reported but never mandatory.

---

## Usage

```bash
python3 crosshair.py            # launch GUI (starts daemon if not running)
python3 crosshair.py --status   # print live daemon status
python3 crosshair.py --toggle   # show/hide the running instance
python3 crosshair.py --show
python3 crosshair.py --hide
python3 crosshair.py --set '{"color": "#00ff88", "size": 8}'
python3 crosshair.py --reload   # reload config.json from disk
python3 crosshair.py --restart  # save, then hand off to a fresh daemon
python3 crosshair.py --quit     # idempotent -- safe to call when not running
```

`--set` takes a strict JSON object: unknown keys, wrong types, and
out-of-range values are rejected outright (not silently clamped), and
the command's exit code and stderr message tell you why.

The settings window opens on launch by default; close it to minimize to
tray (`--daemon --no-ui` skips opening it, e.g. for autostart).

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
python3 crosshair.py --install           # ~/.local/bin/crosshair + app-menu entry
python3 crosshair.py --autostart         # + XDG autostart entry (no systemd unit)
python3 crosshair.py --desktop-shortcut  # double-click icon on the Desktop

python3 crosshair.py --uninstall         # removes the above, keeps your config
python3 crosshair.py --purge-config      # explicitly removes the config too
```

Autostart is an XDG `.desktop` entry under
`$XDG_CONFIG_HOME/autostart/` — nothing is added to systemd. If you
previously installed the pre-3.2 build (which used a systemd user
service), `--autostart` and `--uninstall` both detect and remove that
legacy unit automatically.

---

## Paths

| Purpose | Path |
| --- | --- |
| Config | `${XDG_CONFIG_HOME:-~/.config}/crosshair/config.json` |
| Socket | `$XDG_RUNTIME_DIR/crosshair/ipc.sock` (mode 0600) |
| Lock | `$XDG_RUNTIME_DIR/crosshair/daemon.lock` |
| Log | `${XDG_STATE_HOME:-~/.local/state}/crosshair/crosshair.log` |
| Installed script | `${XDG_DATA_HOME:-~/.local/share}/crosshair/crosshair.py` |
| Launcher | `~/.local/bin/crosshair` |
| App-menu entry | `${XDG_DATA_HOME:-~/.local/share}/applications/crosshair.desktop` |
| Autostart entry | `${XDG_CONFIG_HOME:-~/.config}/autostart/crosshair.desktop` |

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

```bash
pip install --break-system-packages pytest pytest-cov ruff
ruff check .
pytest -q
```

`crosshair_core.py` is a GTK-free module (paths, atomic config I/O,
the single-instance lock, and the IPC protocol) with its own test
suite in `tests/test_core.py`. `crosshair.py` is the GTK application
that imports it. See `RUNBOOK.md` for the full machine-level release
gate, including what's already been verified in a sandboxed headless
X11 environment versus what still needs a real Wayland/KDE session.
