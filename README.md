# Crosshair

GTK3 crosshair overlay for Linux — works on Wayland and X11.

Draws a persistent crosshair on top of all windows. Useful for games that don't have a built-in crosshair, or any application where a fixed center reference helps.

![screenshot placeholder]

---

## Features

- Multiple shapes: cross, dot, circle, dot+ring, T-shape, diagonal X
- Fully adjustable: size, gap, thickness, color (RGBA), opacity
- Per-monitor targeting — picks the monitor your cursor is on, or a fixed index
- Tray icon with toggle, shape picker, and quit
- IPC socket for headless control (`--status`, `--toggle`, `--quit`)
- Input passthrough on X11 (the overlay window does not steal clicks)
- Settings persist to `~/.config/roaring/crosshair.json`

---

## Requirements

- Python 3.10+
- GTK3 (`python-gobject` / `gir1.2-gtk-3.0`)
- `libappindicator3` for tray support (optional but recommended)

```bash
# Arch
sudo pacman -S python-gobject gtk3 libappindicator-gtk3

# Fedora / Nobara
sudo dnf install python3-gobject gtk3 libappindicator-gtk3
```

---

## Usage

```bash
python3 crosshair.py          # launch GUI
python3 crosshair.py --status # print running/stopped to stdout
python3 crosshair.py --toggle # show/hide running instance
python3 crosshair.py --quit   # quit running instance
```

Settings window opens on launch. Close it to minimize to tray.

---

## Install (optional, adds to app launcher)

```bash
cp crosshair.py ~/.local/bin/crosshair
chmod +x ~/.local/bin/crosshair

# Desktop entry
cat > ~/.local/share/applications/crosshair.desktop << EOF
[Desktop Entry]
Name=Crosshair
Exec=python3 /home/$USER/.local/bin/crosshair
Icon=input-gaming
Type=Application
Categories=Utility;
EOF
```
