#!/usr/bin/env python3
# Crosshair.py -- GTK3 crosshair overlay for Linux (Wayland + X11)
# 28/05/2026 patch: deduped draw code, X11 input passthrough + type hint,
#   dot_ring shape, --status flag, active-monitor in dropdown, fix tray toggle
#   not syncing checkbox, fix Gtk.Arrow deprecation, fix scale label layout,
#   math.tau, monitor_index max 32, ASCII section separators

import argparse
import json
import math
import os
import signal
import socket
import sys
import threading
import time
import subprocess
import shutil
from pathlib import Path

APP_ID = "purplecrosshair"
APP_NAME = "PurpleCrosshair"
APP_VERSION = "3.1"
DEFAULT_SOCKET = str(Path.home() / ".cache" / APP_ID / "ipc.sock")
DEFAULT_CONFIG = str(Path.home() / ".config" / APP_ID / "config.json")
DEFAULT_INSTALL_DIR = str(Path.home() / ".local" / "share" / APP_ID)
DEFAULT_BIN = str(Path.home() / ".local" / "bin" / APP_ID)
PID_FILE = str(Path.home() / ".cache" / APP_ID / "daemon.pid")

# --- config helpers

def _clamp(v, lo, hi):
    return lo if v < lo else hi if v > hi else v

def _hex_to_rgba(hexstr, alpha=1.0):
    s = (hexstr or "").strip()
    if not s:
        return (0.65, 0.0, 1.0, alpha)
    if s.startswith("#"):
        s = s[1:]
    if len(s) == 3:
        s = "".join([c + c for c in s])
    if len(s) != 6:
        return (0.65, 0.0, 1.0, alpha)
    try:
        r = int(s[0:2], 16) / 255.0
        g = int(s[2:4], 16) / 255.0
        b = int(s[4:6], 16) / 255.0
        return (r, g, b, alpha)
    except Exception:
        return (0.65, 0.0, 1.0, alpha)

def _rgba_to_hex(rgba):
    r, g, b = rgba[0], rgba[1], rgba[2]
    return "#{:02x}{:02x}{:02x}".format(
        int(_clamp(r, 0, 1) * 255),
        int(_clamp(g, 0, 1) * 255),
        int(_clamp(b, 0, 1) * 255),
    )

# --- config

def default_config():
    return {
        "enabled": True,
        "shape": "dot",  # dot, cross, x, plus, ring, circle, cross_dot, x_dot, dot_ring
        "opacity": 0.95,
        "size": 5,
        "thickness": 2,
        "gap": 3,
        "color": "#a000ff",
        "outline": {
            "enabled": True,
            "color": "#000000",
            "opacity": 0.80,
            "thickness": 1,
        },
        "shadow": {
            "enabled": False,
            "color": "#000000",
            "opacity": 0.35,
            "offset_x": 1,
            "offset_y": 1,
        },
        "monitor_mode": "all",
        "monitor_index": 0,
        "monitor_name": "",
        "preview_bg": "#222222",
        "auto_save": True,
    }

def load_config(path=DEFAULT_CONFIG):
    cfg = default_config()
    p = Path(path)
    if p.exists():
        try:
            data = json.loads(p.read_text(encoding="utf-8"))
            if isinstance(data, dict):
                cfg = deep_merge(cfg, data)
        except Exception:
            pass
    cfg = sanitize_config(cfg)
    return cfg

def save_config(cfg, path=DEFAULT_CONFIG):
    p = Path(path)
    p.parent.mkdir(parents=True, exist_ok=True)
    cfg = sanitize_config(cfg)
    p.write_text(json.dumps(cfg, indent=2, sort_keys=True), encoding="utf-8")

def deep_merge(base, override):
    out = dict(base)
    for k, v in (override or {}).items():
        if isinstance(v, dict) and isinstance(out.get(k), dict):
            out[k] = deep_merge(out[k], v)
        else:
            out[k] = v
    return out

_VALID_SHAPES = {"dot", "cross", "x", "plus", "ring", "circle",
                 "cross_dot", "x_dot", "dot_ring"}

def sanitize_config(cfg):
    orig = cfg if isinstance(cfg, dict) else None
    out = deep_merge(default_config(), orig or {})

    out["enabled"] = bool(out.get("enabled", True))
    out["shape"] = str(out.get("shape", "dot")).lower()
    if out["shape"] not in _VALID_SHAPES:
        out["shape"] = "dot"

    out["opacity"] = _clamp(float(out.get("opacity", 0.95)), 0.05, 1.0)
    out["size"] = _clamp(int(out.get("size", 5)), 1, 200)
    out["thickness"] = _clamp(int(out.get("thickness", 2)), 1, 100)
    out["gap"] = _clamp(int(out.get("gap", 3)), 0, 200)
    out["color"] = str(out.get("color", "#a000ff"))

    o = out.get("outline", {})
    if not isinstance(o, dict):
        o = {}
    out["outline"] = {
        "enabled": bool(o.get("enabled", True)),
        "color": str(o.get("color", "#000000")),
        "opacity": _clamp(float(o.get("opacity", 0.80)), 0.0, 1.0),
        "thickness": _clamp(int(o.get("thickness", 1)), 0, 100),
    }

    s = out.get("shadow", {})
    if not isinstance(s, dict):
        s = {}
    out["shadow"] = {
        "enabled": bool(s.get("enabled", False)),
        "color": str(s.get("color", "#000000")),
        "opacity": _clamp(float(s.get("opacity", 0.35)), 0.0, 1.0),
        "offset_x": _clamp(int(s.get("offset_x", 1)), -200, 200),
        "offset_y": _clamp(int(s.get("offset_y", 1)), -200, 200),
    }

    mm_raw = str(out.get("monitor_mode", "all"))
    mm = mm_raw.strip().lower()
    if mm.startswith("monitor"):
        digits = ""
        for ch in mm:
            if ch.isdigit():
                digits += ch
            elif digits:
                break
        if digits:
            out["monitor_mode"] = "index"
            out["monitor_index"] = int(digits) - 1
        else:
            out["monitor_mode"] = "all"
    elif mm in {"all", "primary", "active", "index", "name"}:
        out["monitor_mode"] = mm
    else:
        out["monitor_mode"] = "all"

    # 255 was unreasonably high; 32 is more than enough for any real setup
    out["monitor_index"] = _clamp(int(out.get("monitor_index", 0)), 0, 32)
    out["monitor_name"] = str(out.get("monitor_name", ""))
    out["preview_bg"] = str(out.get("preview_bg", "#222222"))
    out["auto_save"] = bool(out.get("auto_save", True))

    if orig is not None:
        orig.clear()
        orig.update(out)
        return orig
    return out

# --- pid / single-instance

def write_pid_file():
    p = Path(PID_FILE)
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(str(os.getpid()), encoding="utf-8")

def read_pid_file():
    try:
        return int(Path(PID_FILE).read_text(encoding="utf-8").strip())
    except Exception:
        return None

def pid_is_alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except (ProcessLookupError, PermissionError):
        return False
    except Exception:
        return False

def kill_existing_daemon(sock_path=DEFAULT_SOCKET, timeout=2.0):
    my_pid = os.getpid()
    killed = False

    sp = Path(sock_path)
    if sp.exists():
        ok, _ = ipc_send("quit", sock_path=sock_path, timeout=0.5)
        if ok:
            killed = True
            deadline = time.monotonic() + timeout
            pid = read_pid_file()
            while time.monotonic() < deadline:
                if pid is None or not pid_is_alive(pid) or pid == my_pid:
                    break
                time.sleep(0.05)

    pid = read_pid_file()
    if pid is not None and pid != my_pid and pid > 1 and pid_is_alive(pid):
        try:
            os.kill(pid, signal.SIGTERM)
            killed = True
        except Exception:
            pass
        deadline = time.monotonic() + timeout
        while time.monotonic() < deadline:
            if not pid_is_alive(pid):
                break
            time.sleep(0.05)
        if pid_is_alive(pid):
            try:
                os.kill(pid, signal.SIGKILL)
            except Exception:
                pass

    try:
        if sp.exists():
            sp.unlink()
    except Exception:
        pass

    return killed

def kill_all_python_crosshair():
    """Kill OTHER processes running this script, never ourselves."""
    my_pid = os.getpid()
    my_ppid = os.getppid()
    script_abs = str(Path(__file__).resolve())
    script_name = Path(__file__).name
    victims = set()

    for pattern in [script_abs, script_name]:
        try:
            result = subprocess.run(
                ["pgrep", "-a", "-f", pattern],
                capture_output=True, text=True, timeout=3,
            )
            for line in result.stdout.strip().splitlines():
                parts = line.split(None, 1)
                if not parts:
                    continue
                try:
                    pid = int(parts[0])
                except ValueError:
                    continue
                if pid in (my_pid, my_ppid, 0, 1):
                    continue
                victims.add(pid)
        except Exception:
            pass

    count = 0
    for pid in victims:
        try:
            os.kill(pid, signal.SIGTERM)
            count += 1
        except Exception:
            pass

    if count:
        time.sleep(0.4)
        for pid in victims:
            if pid_is_alive(pid):
                try:
                    os.kill(pid, signal.SIGKILL)
                except Exception:
                    pass

    return count

# --- ipc

def ipc_send(command, payload=None, sock_path=DEFAULT_SOCKET, timeout=0.5):
    msg = {"cmd": command}
    if payload is not None:
        msg["payload"] = payload
    raw = (json.dumps(msg) + "\n").encode("utf-8")

    sp = Path(sock_path)
    if not sp.exists():
        return False, "not_running"

    try:
        s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        s.settimeout(timeout)
        s.connect(sock_path)
        s.sendall(raw)
        try:
            resp = s.recv(4096)
        except Exception:
            resp = b""
        s.close()
        return True, resp.decode("utf-8", errors="ignore").strip() or "ok"
    except Exception as e:
        return False, str(e)

def ipc_server(sock_path, handler):
    sp = Path(sock_path)
    sp.parent.mkdir(parents=True, exist_ok=True)
    if sp.exists():
        try:
            sp.unlink()
        except Exception:
            pass

    srv = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
    srv.bind(sock_path)
    srv.listen(8)
    os.chmod(sock_path, 0o600)

    while True:
        try:
            conn, _ = srv.accept()
        except Exception:
            break
        try:
            data = b""
            conn.settimeout(1.0)
            while True:
                try:
                    chunk = conn.recv(4096)
                except Exception:
                    break
                if not chunk:
                    break
                data += chunk
                if b"\n" in data:
                    break

            line = data.split(b"\n", 1)[0].decode("utf-8", errors="ignore").strip()
            if not line:
                conn.sendall(b"empty\n")
                conn.close()
                continue

            try:
                req = json.loads(line)
            except Exception:
                conn.sendall(b"bad_json\n")
                conn.close()
                continue

            resp = handler(req)
            if resp is None:
                resp = "ok"
            conn.sendall((str(resp) + "\n").encode("utf-8"))
        except Exception as e:
            try:
                conn.sendall((f"error:{e}\n").encode("utf-8"))
            except Exception:
                pass
        finally:
            try:
                conn.close()
            except Exception:
                pass

    try:
        srv.close()
    except Exception:
        pass

# --- installer

def install_self():
    install_dir = Path(DEFAULT_INSTALL_DIR)
    install_dir.mkdir(parents=True, exist_ok=True)
    target_py = install_dir / f"{APP_ID}.py"
    src = Path(__file__).resolve()
    target_py.write_text(src.read_text(encoding="utf-8"), encoding="utf-8")

    bin_path = Path(DEFAULT_BIN)
    bin_path.parent.mkdir(parents=True, exist_ok=True)
    launcher = f"""#!/usr/bin/env bash
exec python3 "{target_py}" "$@"
"""
    bin_path.write_text(launcher, encoding="utf-8")
    os.chmod(bin_path, 0o755)
    return str(target_py), str(bin_path)

def install_desktop_shortcut():
    _, bin_path = install_self()
    desktop_dir = Path.home() / "Desktop"
    desktop_dir.mkdir(parents=True, exist_ok=True)
    shortcut = desktop_dir / f"{APP_ID}.desktop"
    shortcut.write_text(f"""[Desktop Entry]
Version=1.0
Type=Application
Name={APP_NAME}
Comment=Crosshair overlay -- click to open settings
Exec={bin_path}
Icon=crosshairs
Terminal=false
Categories=Utility;
StartupNotify=false
""", encoding="utf-8")
    os.chmod(shortcut, 0o755)
    return str(shortcut)

def install_autostart():
    _, bin_path = install_self()
    unit_dir = Path.home() / ".config" / "systemd" / "user"
    unit_dir.mkdir(parents=True, exist_ok=True)
    unit_path = unit_dir / f"{APP_ID}.service"
    unit_path.write_text(f"""[Unit]
Description={APP_NAME} overlay
After=graphical-session.target

[Service]
Type=simple
ExecStart={bin_path} --daemon --no-ui
Restart=on-failure
RestartSec=2

[Install]
WantedBy=default.target
""", encoding="utf-8")

    autostart_dir = Path.home() / ".config" / "autostart"
    autostart_dir.mkdir(parents=True, exist_ok=True)
    desktop_path = autostart_dir / f"{APP_ID}.desktop"
    desktop_path.write_text(f"""[Desktop Entry]
Type=Application
Name={APP_NAME}
Exec={bin_path} --daemon --no-ui
X-KDE-autostart-after=panel
X-KDE-StartupNotify=false
NoDisplay=true
""", encoding="utf-8")

    sysd_ok = False
    try:
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["systemctl", "--user", "enable", "--now", f"{APP_ID}.service"],
                       check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        sysd_ok = True
    except Exception:
        sysd_ok = False

    return str(unit_path), str(desktop_path), sysd_ok, bin_path

def uninstall_autostart():
    try:
        subprocess.run(["systemctl", "--user", "disable", "--now", f"{APP_ID}.service"],
                       check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        subprocess.run(["systemctl", "--user", "daemon-reload"], check=False,
                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    except Exception:
        pass

    removed = []
    for p in [
        Path.home() / ".config" / "systemd" / "user" / f"{APP_ID}.service",
        Path.home() / ".config" / "autostart" / f"{APP_ID}.desktop",
    ]:
        if p.exists():
            try:
                p.unlink()
                removed.append(str(p))
            except Exception:
                pass
    return removed

# === daemon (GTK overlay + UI)

def run_daemon(open_config=False):
    import warnings
    warnings.filterwarnings("ignore", category=DeprecationWarning)

    # single-instance: kill any stale/zombie previous copies
    kill_all_python_crosshair()
    kill_existing_daemon()
    # wait briefly for killed processes to release the socket
    sock_path_check = Path(DEFAULT_SOCKET)
    _deadline = time.monotonic() + 1.5
    while time.monotonic() < _deadline and sock_path_check.exists():
        time.sleep(0.05)

    try:
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("Gdk", "3.0")
        from gi.repository import Gtk, Gdk, GLib
    except Exception as e:
        print("GTK3 (PyGObject) is not available. Install python3-gobject + gtk3.",
              file=sys.stderr)
        print(str(e), file=sys.stderr)
        return 2

    # suppress harmless GTK icon-theme warnings (C-level, not catchable with warnings module)
    try:
        def _gtk_log_suppress(domain, level, message, user_data):
            pass
        GLib.log_set_handler("Gtk", GLib.LogLevelFlags.LEVEL_WARNING, _gtk_log_suppress, None)
    except Exception:
        pass

    HAVE_LAYERSHELL = False
    GtkLayerShell = None
    try:
        gi.require_version("GtkLayerShell", "0.1")
        from gi.repository import GtkLayerShell as _GtkLayerShell
        GtkLayerShell = _GtkLayerShell
        HAVE_LAYERSHELL = True
    except Exception:
        HAVE_LAYERSHELL = False

    HAVE_APPINDICATOR = False
    AppIndicator3 = None
    try:
        gi.require_version("AppIndicator3", "0.1")
        from gi.repository import AppIndicator3 as _AppIndicator3
        AppIndicator3 = _AppIndicator3
        HAVE_APPINDICATOR = True
    except Exception:
        HAVE_APPINDICATOR = False

    cfg_path = DEFAULT_CONFIG
    cfg = load_config(cfg_path)

    try:
        import cairo
    except Exception as e:
        print("pycairo is not available. Install python3-cairo.", file=sys.stderr)
        print(str(e), file=sys.stderr)
        return 2

    write_pid_file()

    # === shared crosshair drawing
    # Used by both the overlay window and the settings preview.
    # No cairo save/restore -- callers handle that if needed.
    def _draw_crosshair(cr, cx, cy, c):
        shape = c.get("shape", "dot")
        opacity = float(c.get("opacity", 0.95))
        size = float(c.get("size", 5))
        thick = float(c.get("thickness", 2))
        gap = float(c.get("gap", 3))
        main_rgba = _hex_to_rgba(c.get("color", "#a000ff"), opacity)

        outline = c.get("outline", {})
        outline_on = bool(outline.get("enabled", True)) and int(outline.get("thickness", 1)) > 0
        outline_thick = float(outline.get("thickness", 1))
        outline_rgba = _hex_to_rgba(
            outline.get("color", "#000000"),
            float(outline.get("opacity", 0.8)) * opacity,
        )

        shadow = c.get("shadow", {})
        shadow_on = bool(shadow.get("enabled", False))
        sh_rgba = _hex_to_rgba(
            shadow.get("color", "#000000"),
            float(shadow.get("opacity", 0.35)) * opacity,
        )
        sh_dx = float(shadow.get("offset_x", 1))
        sh_dy = float(shadow.get("offset_y", 1))

        cr.set_line_cap(cairo.LINE_CAP_ROUND)
        cr.set_line_join(cairo.LINE_JOIN_ROUND)

        def stroke_lines(rgba, width, dx=0.0, dy=0.0):
            cr.set_source_rgba(*rgba)
            cr.set_line_width(max(0.5, width))
            if shape in ("cross", "cross_dot", "plus"):
                g = 0.0 if shape == "plus" else gap
                cr.move_to(cx+dx,   cy+dy-g);       cr.line_to(cx+dx,   cy+dy-(g+size))
                cr.move_to(cx+dx,   cy+dy+g);       cr.line_to(cx+dx,   cy+dy+(g+size))
                cr.move_to(cx+dx-g, cy+dy);         cr.line_to(cx+dx-(g+size), cy+dy)
                cr.move_to(cx+dx+g, cy+dy);         cr.line_to(cx+dx+(g+size), cy+dy)
                cr.stroke()
            if shape in ("x", "x_dot"):
                cr.move_to(cx+dx-gap, cy+dy-gap);   cr.line_to(cx+dx-(gap+size), cy+dy-(gap+size))
                cr.move_to(cx+dx+gap, cy+dy+gap);   cr.line_to(cx+dx+(gap+size), cy+dy+(gap+size))
                cr.move_to(cx+dx-gap, cy+dy+gap);   cr.line_to(cx+dx-(gap+size), cy+dy+(gap+size))
                cr.move_to(cx+dx+gap, cy+dy-gap);   cr.line_to(cx+dx+(gap+size), cy+dy-(gap+size))
                cr.stroke()
            if shape in ("ring", "circle", "dot_ring"):
                cr.arc(cx+dx, cy+dy, max(1.0, size), 0.0, math.tau)
                cr.stroke()

        def fill_dot(rgba, radius, dx=0.0, dy=0.0):
            cr.set_source_rgba(*rgba)
            cr.arc(cx+dx, cy+dy, max(0.5, radius), 0.0, math.tau)
            cr.fill()

        # shadow pass
        if shadow_on:
            if shape == "dot":
                fill_dot(sh_rgba, max(1.0, size), sh_dx, sh_dy)
            elif shape in ("ring", "circle"):
                stroke_lines(sh_rgba, thick, sh_dx, sh_dy)
            elif shape == "dot_ring":
                stroke_lines(sh_rgba, thick, sh_dx, sh_dy)
                fill_dot(sh_rgba, max(1.0, thick * 0.9), sh_dx, sh_dy)
            else:
                stroke_lines(sh_rgba, thick, sh_dx, sh_dy)
                if shape in ("cross_dot", "x_dot"):
                    fill_dot(sh_rgba, max(1.0, thick * 0.9), sh_dx, sh_dy)

        # outline pass
        if outline_on:
            ow = thick + outline_thick * 2.0
            if shape == "dot":
                fill_dot(outline_rgba, max(1.0, size + outline_thick))
            elif shape in ("ring", "circle"):
                stroke_lines(outline_rgba, ow)
            elif shape == "dot_ring":
                stroke_lines(outline_rgba, ow)
                fill_dot(outline_rgba, max(1.0, thick * 0.9 + outline_thick))
            else:
                stroke_lines(outline_rgba, ow)
                if shape in ("cross_dot", "x_dot"):
                    fill_dot(outline_rgba, max(1.0, thick * 0.9 + outline_thick))

        # main pass
        if shape == "dot":
            fill_dot(main_rgba, max(1.0, size))
        elif shape in ("ring", "circle"):
            stroke_lines(main_rgba, thick)
        elif shape == "dot_ring":
            stroke_lines(main_rgba, thick)
            fill_dot(main_rgba, max(1.0, thick * 0.9))
        else:
            stroke_lines(main_rgba, thick)
            if shape in ("cross_dot", "x_dot"):
                fill_dot(main_rgba, max(1.0, thick * 0.9))

    SHAPES = ["dot", "cross", "x", "plus", "ring", "circle",
              "cross_dot", "x_dot", "dot_ring"]

    # === overlay window

    class OverlayWindow(Gtk.Window):
        def __init__(self, monitor, is_wayland, use_layershell):
            super().__init__(type=Gtk.WindowType.TOPLEVEL)
            self.monitor = monitor
            self.is_wayland = is_wayland
            self.use_layershell = use_layershell

            self.set_decorated(False)
            self.set_app_paintable(True)
            self.set_skip_taskbar_hint(True)
            self.set_skip_pager_hint(True)
            self.set_accept_focus(False)
            self.set_focus_on_map(False)
            self.stick()

            screen = self.get_screen()
            visual = screen.get_rgba_visual()
            if visual is not None:
                self.set_visual(visual)

            self.darea = Gtk.DrawingArea()
            self.darea.connect("draw", self.on_draw)
            self.add(self.darea)

            self._apply_window_rules()
            self._apply_size_and_position()
            self.show_all()

        def _apply_window_rules(self):
            if self.use_layershell:
                GtkLayerShell.init_for_window(self)
                GtkLayerShell.set_layer(self, GtkLayerShell.Layer.OVERLAY)
                try:
                    GtkLayerShell.set_keyboard_mode(self, GtkLayerShell.KeyboardMode.NONE)
                except AttributeError:
                    try:
                        GtkLayerShell.set_keyboard_interactivity(self, False)
                    except Exception:
                        pass
                GtkLayerShell.set_monitor(self, self.monitor)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.TOP, True)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.LEFT, True)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.RIGHT, False)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.BOTTOM, False)
                GtkLayerShell.set_exclusive_zone(self, -1)
            else:
                # X11: UTILITY type floats on top without full WM management
                try:
                    self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
                except Exception:
                    pass
                try:
                    self.set_keep_above(True)
                except Exception:
                    pass

            # pass all mouse events through the canvas to whatever is below
            # on X11 this is critical -- without it the canvas intercepts clicks
            # on Wayland/layershell this is a no-op (safe to call either way)
            self.connect("realize", self._on_realize_passthrough)

        def _on_realize_passthrough(self, *_):
            try:
                self.input_shape_combine_region(cairo.Region())
            except Exception:
                pass

        def _canvas_size_px(self):
            c = cfg
            size = int(c["size"])
            thick = int(c["thickness"])
            gap = int(c["gap"])
            o = c["outline"]
            outline_thick = int(o["thickness"]) if o.get("enabled") else 0
            sh = c["shadow"]
            shadow_pad = 0
            if sh.get("enabled"):
                shadow_pad = max(abs(int(sh.get("offset_x", 0))),
                                 abs(int(sh.get("offset_y", 0)))) + 2
            shape = c["shape"]
            if shape == "dot":
                ext = size + outline_thick + shadow_pad
            elif shape in ("ring", "circle", "dot_ring"):
                ext = size + thick + outline_thick + shadow_pad
            else:
                ext = (gap + size) + thick + outline_thick + shadow_pad
            return max(16, int(ext * 2 + 8))

        def _apply_size_and_position(self):
            canvas = self._canvas_size_px()
            self.set_default_size(canvas, canvas)
            self.darea.set_size_request(canvas, canvas)
            try:
                geo = self.monitor.get_geometry()
                x_margin = int((geo.width - canvas) / 2)
                y_margin = int((geo.height - canvas) / 2)
                if self.use_layershell:
                    GtkLayerShell.set_margin(self, GtkLayerShell.Edge.LEFT, max(0, x_margin))
                    GtkLayerShell.set_margin(self, GtkLayerShell.Edge.TOP, max(0, y_margin))
                else:
                    self.move(geo.x + x_margin, geo.y + y_margin)
            except Exception:
                pass
            self.queue_draw()

        def apply_config(self):
            self._apply_size_and_position()
            self.queue_draw()

        def on_draw(self, widget, cr):
            alloc = widget.get_allocation()
            w = float(alloc.width)
            h = float(alloc.height)

            cr.save()
            cr.set_operator(cairo.OPERATOR_SOURCE)
            cr.set_source_rgba(0, 0, 0, 0)
            cr.paint()
            cr.restore()

            if not cfg.get("enabled", True):
                return False

            _draw_crosshair(cr, w * 0.5, h * 0.5, cfg)
            return False

    # === app state

    is_wayland = (os.environ.get("XDG_SESSION_TYPE", "").lower() == "wayland")
    display = Gdk.Display.get_default()
    if is_wayland and not HAVE_LAYERSHELL:
        print("Wayland: GtkLayerShell not available. Install gtk-layer-shell.",
              file=sys.stderr)
    if display is None:
        print("No graphical display detected.", file=sys.stderr)
        return 2

    _mon_name_cache = {}

    def monitor_pretty_name(mon):
        # Cache by object identity -- EDID reads (manufacturer/model) can block
        # for several seconds on some drivers. get_connector() is always instant.
        key = id(mon)
        if key in _mon_name_cache:
            return _mon_name_cache[key]
        name = ""
        try:
            conn = (mon.get_connector() or "").strip()
            if conn:
                name = conn
        except AttributeError:
            pass
        if not name:
            try:
                mfg   = (mon.get_manufacturer() or "").strip()
                model = (mon.get_model()        or "").strip()
                name  = " ".join([x for x in (mfg, model) if x]).strip()
            except Exception:
                pass
        if not name:
            name = "Unknown"
        _mon_name_cache[key] = name
        return name

    def get_all_monitors():
        mons = []
        try:
            n = int(display.get_n_monitors() or 0)
        except Exception:
            n = 0
        for i in range(n):
            try:
                mons.append(display.get_monitor(i))
            except Exception:
                pass
        return mons

    def get_monitor_choices_for_ui():
        # "active" follows the cursor -- useful when swapping between displays
        choices = [
            ("all",                  "all",    None),
            ("primary",              "primary", None),
            ("active (cursor mon.)", "active",  None),
        ]
        mons = get_all_monitors()
        for i, mon in enumerate(mons):
            choices.append((f"monitor {i+1} [{monitor_pretty_name(mon)}]", "index", i))
        return choices

    def _fallback_primary(mons):
        try:
            primary = display.get_primary_monitor()
        except Exception:
            primary = None
        if primary is not None:
            return [primary]
        return [mons[0]] if mons else []

    def get_monitors():
        mons = get_all_monitors()
        if not mons:
            return []
        mm = str(cfg.get("monitor_mode", "all")).strip().lower()
        if mm == "primary":
            return _fallback_primary(mons)
        if mm == "index":
            idx = _clamp(int(cfg.get("monitor_index", 0) or 0), 0, len(mons) - 1)
            cfg["monitor_index"] = idx
            return [mons[idx]]
        if mm == "name":
            needle = str(cfg.get("monitor_name", "")).strip().lower()
            if needle:
                for mon in mons:
                    if needle in monitor_pretty_name(mon).lower():
                        return [mon]
            return _fallback_primary(mons)
        if mm == "active":
            try:
                seat = display.get_default_seat()
                pointer = seat.get_pointer() if seat else None
                if pointer is not None:
                    pos = pointer.get_position()
                    x = y = 0
                    if isinstance(pos, tuple):
                        if len(pos) >= 3:
                            x, y = int(pos[1]), int(pos[2])
                        elif len(pos) == 2:
                            x, y = int(pos[0]), int(pos[1])
                    mon = display.get_monitor_at_point(x, y)
                    if mon is not None:
                        return [mon]
            except Exception:
                pass
            return _fallback_primary(mons)
        return mons  # all

    windows = []

    def rebuild_windows():
        nonlocal windows
        for w in windows:
            try:
                w.destroy()
            except Exception:
                pass
        windows = []
        use_layershell = bool(is_wayland and HAVE_LAYERSHELL)
        for mon in get_monitors():
            windows.append(OverlayWindow(mon, is_wayland=is_wayland,
                                         use_layershell=use_layershell))

    rebuild_windows()

    # defined early so SettingsWindow and tray can reference it;
    # real implementation added via _quit_callbacks below
    _quit_callbacks = []

    def request_quit():
        for cb in list(_quit_callbacks):
            try:
                cb()
            except Exception:
                pass

    # === settings window

    class SettingsWindow(Gtk.Window):
        def __init__(self):
            super().__init__(title=f"{APP_NAME} Settings v{APP_VERSION}")
            self.set_default_size(540, 640)
            self._debounce_id = None
            self._suppress_changes = False
            self._pending_rebuild = False

            outer = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
            outer.set_border_width(8)
            self.add(outer)

            # header bar
            hb = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            outer.pack_start(hb, False, False, 0)

            self.btn_toggle = Gtk.Button(label="Toggle")
            self.btn_toggle.connect("clicked", lambda *_: self._toggle_enabled())
            hb.pack_start(self.btn_toggle, False, False, 0)

            self.btn_show = Gtk.Button(label="Show")
            self.btn_show.connect("clicked", lambda *_: self._set_enabled(True))
            hb.pack_start(self.btn_show, False, False, 0)

            self.btn_hide = Gtk.Button(label="Hide")
            self.btn_hide.connect("clicked", lambda *_: self._set_enabled(False))
            hb.pack_start(self.btn_hide, False, False, 0)

            self.btn_restart = Gtk.Button(label="↺ Restart")
            self.btn_restart.set_tooltip_text(
                "Saves config, kills all running instances, and re-launches fresh.\n"
                "Use this if the UI becomes unresponsive."
            )
            self.btn_restart.connect("clicked", lambda *_: self._restart())
            hb.pack_start(self.btn_restart, False, False, 0)

            self.btn_save = Gtk.Button(label="💾 Save")
            self.btn_save.connect("clicked", lambda *_: self._save())
            hb.pack_end(self.btn_save, False, False, 0)

            self.btn_quit = Gtk.Button(label="✕ Quit")
            self.btn_quit.connect("clicked", lambda *_: request_quit())
            hb.pack_end(self.btn_quit, False, False, 0)

            nb = Gtk.Notebook()
            outer.pack_start(nb, True, True, 0)

            # tab: General
            tab_gen = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            tab_gen.set_border_width(10)
            nb.append_page(tab_gen, Gtk.Label(label="General"))

            self.chk_enabled = Gtk.CheckButton(label="Enabled")
            self.chk_enabled.set_active(bool(cfg.get("enabled", True)))
            self.chk_enabled.connect("toggled", lambda *_: self._changed())
            tab_gen.pack_start(self.chk_enabled, False, False, 0)

            self.chk_auto_save = Gtk.CheckButton(label="Auto-save on change")
            self.chk_auto_save.set_active(bool(cfg.get("auto_save", True)))
            self.chk_auto_save.set_tooltip_text(
                "When on, every UI change is immediately written to disk.\n"
                "When off, use the Save button."
            )
            self.chk_auto_save.connect("toggled", lambda *_: self._changed())
            tab_gen.pack_start(self.chk_auto_save, False, False, 0)

            row_shape = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            tab_gen.pack_start(row_shape, False, False, 0)
            lbl_shape = Gtk.Label(label="Shape", xalign=0)
            lbl_shape.set_size_request(120, -1)
            row_shape.pack_start(lbl_shape, False, False, 0)
            self.cmb_shape = Gtk.ComboBoxText()
            for s in SHAPES:
                self.cmb_shape.append_text(s)
            active_idx = SHAPES.index(cfg.get("shape", "dot")) if cfg.get("shape", "dot") in SHAPES else 0
            self.cmb_shape.set_active(active_idx)
            self.cmb_shape.connect("changed", lambda *_: self._changed())
            row_shape.pack_end(self.cmb_shape, False, False, 0)

            # monitor selector
            row_mon = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            tab_gen.pack_start(row_mon, False, False, 0)
            lbl_mon = Gtk.Label(label="Monitor mode", xalign=0)
            lbl_mon.set_size_request(120, -1)
            row_mon.pack_start(lbl_mon, False, False, 0)

            self._mon_choice_mode = str(cfg.get("monitor_mode", "all")).strip().lower()
            self._mon_choice_index = int(cfg.get("monitor_index", 0) or 0)
            self._mon_choice_name = str(cfg.get("monitor_name", ""))

            self.mon_btn = Gtk.MenuButton()
            self.mon_btn.set_size_request(320, -1)
            mon_inner = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
            self.mon_btn.add(mon_inner)
            self.mon_btn_label = Gtk.Label(label="", xalign=0)
            mon_inner.pack_start(self.mon_btn_label, True, True, 0)
            # Gtk.Arrow is deprecated since 3.14 -- use a plain label instead
            mon_inner.pack_end(Gtk.Label(label="▾"), False, False, 0)
            self.mon_pop = Gtk.Popover.new(self.mon_btn)
            self.mon_pop.set_border_width(6)
            self.mon_pop_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=2)
            self.mon_pop.add(self.mon_pop_box)
            self.mon_btn.set_popover(self.mon_pop)
            self._refresh_monitor_dropdown()
            row_mon.pack_end(self.mon_btn, True, True, 0)

            btn_reset = Gtk.Button(label="Reset to Defaults")
            btn_reset.set_tooltip_text("Restore all settings to their default values.")
            btn_reset.connect("clicked", lambda *_: self._reset_defaults())
            tab_gen.pack_start(btn_reset, False, False, 0)

            # tab: Appearance
            tab_app = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            tab_app.set_border_width(10)
            nb.append_page(tab_app, Gtk.Label(label="Appearance"))

            self.scale_opacity = self._add_scale(tab_app, "Opacity", 0.05, 1.0,
                                                  cfg.get("opacity", 0.95), step=0.01)
            self.scale_size = self._add_scale(tab_app, "Size", 1, 200, cfg.get("size", 5))
            self.scale_thick = self._add_scale(tab_app, "Thickness", 1, 100, cfg.get("thickness", 2))
            self.scale_gap = self._add_scale(tab_app, "Gap", 0, 200, cfg.get("gap", 3))

            row_col = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            tab_app.pack_start(row_col, False, False, 0)
            lbl_col = Gtk.Label(label="Color", xalign=0)
            lbl_col.set_size_request(120, -1)
            row_col.pack_start(lbl_col, False, False, 0)
            self.btn_color = Gtk.ColorButton()
            self.btn_color.set_use_alpha(False)
            self.btn_color.set_rgba(self._gdk_rgba_from_hex(cfg.get("color", "#a000ff")))
            self.btn_color.connect("color-set", lambda *_: self._changed())
            row_col.pack_end(self.btn_color, False, False, 0)

            # outline frame
            fr_out = Gtk.Frame(label="Outline")
            tab_app.pack_start(fr_out, False, False, 0)
            box_out = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
            box_out.set_border_width(8)
            fr_out.add(box_out)

            self.chk_outline = Gtk.CheckButton(label="Enable outline")
            self.chk_outline.set_active(bool(cfg.get("outline", {}).get("enabled", True)))
            self.chk_outline.connect("toggled", lambda *_: self._changed())
            box_out.pack_start(self.chk_outline, False, False, 0)

            row_out_col = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            box_out.pack_start(row_out_col, False, False, 0)
            lbl_oc = Gtk.Label(label="Outline color", xalign=0)
            lbl_oc.set_size_request(120, -1)
            row_out_col.pack_start(lbl_oc, False, False, 0)
            self.btn_outline_color = Gtk.ColorButton()
            self.btn_outline_color.set_use_alpha(False)
            self.btn_outline_color.set_rgba(
                self._gdk_rgba_from_hex(cfg.get("outline", {}).get("color", "#000000"))
            )
            self.btn_outline_color.connect("color-set", lambda *_: self._changed())
            row_out_col.pack_end(self.btn_outline_color, False, False, 0)

            self.scale_outline_op = self._add_scale(box_out, "Outline opacity", 0.0, 1.0,
                cfg.get("outline", {}).get("opacity", 0.8), step=0.01)
            self.scale_outline_th = self._add_scale(box_out, "Outline thickness", 0, 100,
                cfg.get("outline", {}).get("thickness", 1))

            # shadow frame
            fr_sh = Gtk.Frame(label="Shadow")
            tab_app.pack_start(fr_sh, False, False, 0)
            box_sh = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=8)
            box_sh.set_border_width(8)
            fr_sh.add(box_sh)

            self.chk_shadow = Gtk.CheckButton(label="Enable shadow")
            self.chk_shadow.set_active(bool(cfg.get("shadow", {}).get("enabled", False)))
            self.chk_shadow.connect("toggled", lambda *_: self._changed())
            box_sh.pack_start(self.chk_shadow, False, False, 0)

            row_sh_col = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            box_sh.pack_start(row_sh_col, False, False, 0)
            lbl_sc = Gtk.Label(label="Shadow color", xalign=0)
            lbl_sc.set_size_request(120, -1)
            row_sh_col.pack_start(lbl_sc, False, False, 0)
            self.btn_shadow_color = Gtk.ColorButton()
            self.btn_shadow_color.set_use_alpha(False)
            self.btn_shadow_color.set_rgba(
                self._gdk_rgba_from_hex(cfg.get("shadow", {}).get("color", "#000000"))
            )
            self.btn_shadow_color.connect("color-set", lambda *_: self._changed())
            row_sh_col.pack_end(self.btn_shadow_color, False, False, 0)

            self.scale_shadow_op = self._add_scale(box_sh, "Shadow opacity", 0.0, 1.0,
                cfg.get("shadow", {}).get("opacity", 0.35), step=0.01)

            row_sh_off = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            box_sh.pack_start(row_sh_off, False, False, 0)
            lbl_soff = Gtk.Label(label="Offset X / Y", xalign=0)
            lbl_soff.set_size_request(120, -1)
            row_sh_off.pack_start(lbl_soff, False, False, 0)
            self.spin_sh_x = Gtk.SpinButton.new_with_range(-200, 200, 1)
            self.spin_sh_y = Gtk.SpinButton.new_with_range(-200, 200, 1)
            self.spin_sh_x.set_value(float(cfg.get("shadow", {}).get("offset_x", 1)))
            self.spin_sh_y.set_value(float(cfg.get("shadow", {}).get("offset_y", 1)))
            self.spin_sh_x.connect("value-changed", lambda *_: self._changed())
            self.spin_sh_y.connect("value-changed", lambda *_: self._changed())
            row_sh_off.pack_end(self.spin_sh_y, False, False, 0)
            row_sh_off.pack_end(self.spin_sh_x, False, False, 0)

            # tab: Preview
            tab_prev = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            tab_prev.set_border_width(10)
            nb.append_page(tab_prev, Gtk.Label(label="Preview"))

            self.preview = Gtk.DrawingArea()
            self.preview.set_size_request(240, 240)
            self.preview.connect("draw", self._draw_preview)
            tab_prev.pack_start(self.preview, True, True, 0)

            row_bg = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            tab_prev.pack_start(row_bg, False, False, 0)
            lbl_pbg = Gtk.Label(label="Preview background", xalign=0)
            lbl_pbg.set_size_request(150, -1)
            row_bg.pack_start(lbl_pbg, False, False, 0)
            self.btn_preview_bg = Gtk.ColorButton()
            self.btn_preview_bg.set_use_alpha(False)
            self.btn_preview_bg.set_rgba(self._gdk_rgba_from_hex(cfg.get("preview_bg", "#222222")))
            self.btn_preview_bg.connect("color-set", lambda *_: self._preview_bg_changed())
            row_bg.pack_end(self.btn_preview_bg, False, False, 0)

            # tab: Hotkeys & Autostart
            tab_hot = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            tab_hot.set_border_width(10)
            nb.append_page(tab_hot, Gtk.Label(label="Hotkeys & Autostart"))

            lbl = Gtk.Label()
            lbl.set_xalign(0)
            script_hint = self._launcher_hint()
            lbl.set_markup(
                "Wayland blocks global key grabs. On KDE, bind a shortcut to a command.\n\n"
                f"<b>Toggle:</b>\n<tt>{script_hint} --toggle</tt>\n\n"
                f"<b>Show:</b>\n<tt>{script_hint} --show</tt>\n\n"
                f"<b>Hide:</b>\n<tt>{script_hint} --hide</tt>\n"
            )
            lbl.set_line_wrap(True)
            lbl.set_selectable(True)
            tab_hot.pack_start(lbl, False, False, 0)

            row_copy = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
            tab_hot.pack_start(row_copy, False, False, 0)
            btn_copy = Gtk.Button(label="Copy toggle command")
            btn_copy.connect("clicked", lambda *_: self._copy_text(f"{script_hint} --toggle"))
            row_copy.pack_start(btn_copy, False, False, 0)

            btn_install = Gtk.Button(label="Install + Autostart")
            btn_install.connect("clicked", lambda *_: self._install_autostart())
            row_copy.pack_end(btn_install, False, False, 0)

            btn_uninstall = Gtk.Button(label="Remove Autostart")
            btn_uninstall.connect("clicked", lambda *_: self._remove_autostart())
            row_copy.pack_end(btn_uninstall, False, False, 0)

            tip = Gtk.Label()
            tip.set_xalign(0)
            tip.set_line_wrap(True)
            tip.set_markup(
                "<b>Fullscreen tip:</b> If crosshair vanishes in fullscreen,\n"
                "set <tt>KWIN_DRM_NO_DIRECT_SCANOUT=1</tt> and re-login."
            )
            tab_hot.pack_start(tip, False, False, 0)

            # tab: System
            tab_sys = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
            tab_sys.set_border_width(10)
            nb.append_page(tab_sys, Gtk.Label(label="System"))

            btn_cfg_folder = Gtk.Button(label="📁 Open Config Folder")
            btn_cfg_folder.set_tooltip_text(str(Path(DEFAULT_CONFIG).parent))
            btn_cfg_folder.connect("clicked", lambda *_: self._open_folder(
                str(Path(DEFAULT_CONFIG).parent)
            ))
            tab_sys.pack_start(btn_cfg_folder, False, False, 0)

            btn_script = Gtk.Button(label="📄 Open Script Location")
            btn_script.set_tooltip_text(str(Path(__file__).resolve().parent))
            btn_script.connect("clicked", lambda *_: self._open_folder(
                str(Path(__file__).resolve().parent)
            ))
            tab_sys.pack_start(btn_script, False, False, 0)

            btn_edit_cfg = Gtk.Button(label="✏️ Edit Config in Text Editor")
            btn_edit_cfg.connect("clicked", lambda *_: self._edit_config())
            tab_sys.pack_start(btn_edit_cfg, False, False, 0)

            btn_reload = Gtk.Button(label="🔄 Reload Config from Disk")
            btn_reload.set_tooltip_text(
                "Discards current UI state and reloads config.json from disk."
            )
            btn_reload.connect("clicked", lambda *_: self._reload_config())
            tab_sys.pack_start(btn_reload, False, False, 0)

            btn_kill_restart = Gtk.Button(label="☠ Kill All & Restart Fresh")
            btn_kill_restart.set_tooltip_text(
                "Kills every running instance of this script (including this one),\n"
                "then re-launches a fresh copy."
            )
            btn_kill_restart.connect("clicked", lambda *_: self._kill_all_restart())
            tab_sys.pack_start(btn_kill_restart, False, False, 0)

            sep = Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL)
            tab_sys.pack_start(sep, False, False, 0)

            info = Gtk.Label()
            info.set_xalign(0)
            info.set_line_wrap(True)
            info.set_markup(
                f"<b>Config:</b> <tt>{DEFAULT_CONFIG}</tt>\n"
                f"<b>Socket:</b> <tt>{DEFAULT_SOCKET}</tt>\n"
                f"<b>PID file:</b> <tt>{PID_FILE}</tt>\n"
                f"<b>Version:</b> {APP_VERSION}"
            )
            info.set_selectable(True)
            tab_sys.pack_start(info, False, False, 0)

            # status bar
            self.statusbar = Gtk.Label(label="", xalign=0)
            try:
                from gi.repository import Pango
                self.statusbar.set_ellipsize(Pango.EllipsizeMode.END)
            except Exception:
                self.statusbar.set_ellipsize(3)
            outer.pack_start(self.statusbar, False, False, 0)
            self._update_statusbar()

            self.connect("delete-event", lambda *_: self.hide_on_delete())
            # sync UI to current cfg whenever the window becomes visible --
            # catches toggles/reloads that happened while the window was hidden
            self.connect("show", lambda *_: self._sync_ui_from_cfg())

        # helpers

        def _update_statusbar(self):
            try:
                mons = get_all_monitors()
                mon_info = " | ".join(
                    f"#{i+1} {monitor_pretty_name(m)}" for i, m in enumerate(mons)
                )
                enabled = "ON" if cfg.get("enabled", True) else "OFF"
                self.statusbar.set_text(
                    f"Crosshair: {enabled}  |  {len(mons)} monitor(s): {mon_info}"
                )
            except Exception:
                pass

        def _launcher_hint(self):
            binp = Path(DEFAULT_BIN)
            if binp.exists():
                return str(binp)
            return f'python3 "{Path(__file__).resolve()}"'

        def _copy_text(self, text):
            try:
                cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
                cb.set_text(text, -1)
                cb.store()
            except Exception:
                pass

        def _open_folder(self, path):
            try:
                subprocess.Popen(["xdg-open", path])
            except Exception:
                pass

        def _edit_config(self):
            save_config(cfg, cfg_path)
            try:
                subprocess.Popen(["xdg-open", DEFAULT_CONFIG])
            except Exception:
                pass

        def _install_autostart(self):
            unit_path, desktop_path, sysd_ok, bin_path = install_autostart()
            msg = f"Installed:\n  {bin_path}\n  {unit_path}\n  {desktop_path}\n"
            msg += "\nSystemd autostart: " + ("enabled." if sysd_ok else "fallback .desktop created.")
            self._info_dialog("Install complete", msg)

        def _remove_autostart(self):
            removed = uninstall_autostart()
            msg = "Removed:\n" + ("\n".join(removed) if removed else "(nothing found)")
            self._info_dialog("Autostart removed", msg)

        def _info_dialog(self, title, body):
            dlg = Gtk.MessageDialog(parent=self, modal=True,
                                    message_type=Gtk.MessageType.INFO,
                                    buttons=Gtk.ButtonsType.OK, text=title)
            dlg.format_secondary_text(body)
            dlg.run()
            dlg.destroy()

        def _gdk_rgba_from_hex(self, hexstr):
            rgba = _hex_to_rgba(hexstr, 1.0)
            gr = Gdk.RGBA()
            gr.red, gr.green, gr.blue, gr.alpha = rgba[0], rgba[1], rgba[2], 1.0
            return gr

        def _add_scale(self, parent_box, label, lo, hi, val, step=1):
            row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=10)
            parent_box.pack_start(row, False, False, 0)
            # fixed-width label so the scale widget gets most of the row
            lbl = Gtk.Label(label=label, xalign=0)
            lbl.set_size_request(120, -1)
            row.pack_start(lbl, False, False, 0)
            adj = Gtk.Adjustment(value=float(val), lower=float(lo), upper=float(hi),
                                  step_increment=float(step), page_increment=float(step) * 10)
            scale = Gtk.Scale(orientation=Gtk.Orientation.HORIZONTAL, adjustment=adj)
            scale.set_digits(2 if step < 1 else 0)
            scale.set_value_pos(Gtk.PositionType.RIGHT)
            scale.connect("value-changed", lambda *_: self._changed())
            row.pack_end(scale, True, True, 0)
            return scale

        def _toggle_enabled(self):
            # set_active fires 'toggled' which calls _changed() -- don't call it again
            self.chk_enabled.set_active(not self.chk_enabled.get_active())

        def _set_enabled(self, on):
            self.chk_enabled.set_active(bool(on))

        def _reset_defaults(self):
            dlg = Gtk.MessageDialog(
                parent=self, modal=True, message_type=Gtk.MessageType.QUESTION,
                buttons=Gtk.ButtonsType.YES_NO,
                text="Reset to Defaults?",
            )
            dlg.format_secondary_text(
                "All settings will be restored to their default values.\n"
                "This cannot be undone."
            )
            resp = dlg.run()
            dlg.destroy()
            if resp == Gtk.ResponseType.YES:
                new_cfg = default_config()
                save_config(new_cfg, cfg_path)
                loaded = load_config(cfg_path)
                cfg.clear()
                cfg.update(loaded)
                apply_config(rebuild=True)
                self._sync_ui_from_cfg()

        def _reload_config(self):
            loaded = load_config(cfg_path)
            cfg.clear()
            cfg.update(loaded)
            apply_config(rebuild=True)
            self._sync_ui_from_cfg()

        def _preview_bg_changed(self):
            c = self.btn_preview_bg.get_rgba()
            cfg["preview_bg"] = _rgba_to_hex((c.red, c.green, c.blue, 1.0))
            self.preview.queue_draw()
            if cfg.get("auto_save", True):
                save_config(cfg, cfg_path)

        def _restart(self):
            """Save config, spawn a fresh daemon, then exit this one."""
            self._read_ui_to_cfg()
            save_config(cfg, cfg_path)
            script = str(Path(__file__).resolve())
            subprocess.Popen(
                [sys.executable, script, "--daemon", "--no-ui"],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            # give the new process a moment to grab the socket before we quit
            GLib.timeout_add(300, lambda: (request_quit(), False)[1])

        def _kill_all_restart(self):
            self._read_ui_to_cfg()
            save_config(cfg, cfg_path)
            script = str(Path(__file__).resolve())
            script_name = Path(__file__).name
            my_pid = os.getpid()
            helper = (
                f"import time, os, signal, subprocess, sys\n"
                f"my_pid = {my_pid}\n"
                f"script = {repr(script)}\n"
                f"script_name = {repr(script_name)}\n"
                f"deadline = time.monotonic() + 3.0\n"
                f"while time.monotonic() < deadline:\n"
                f"    try:\n"
                f"        os.kill(my_pid, 0)\n"
                f"        time.sleep(0.1)\n"
                f"    except OSError:\n"
                f"        break\n"
                f"try:\n"
                f"    r = subprocess.run(['pgrep','-f',script_name], capture_output=True, text=True)\n"
                f"    for line in r.stdout.strip().splitlines():\n"
                f"        try:\n"
                f"            pid = int(line.strip())\n"
                f"            if pid != os.getpid() and pid > 1:\n"
                f"                os.kill(pid, signal.SIGTERM)\n"
                f"        except Exception:\n"
                f"            pass\n"
                f"except Exception:\n"
                f"    pass\n"
                f"time.sleep(0.3)\n"
                f"subprocess.Popen([sys.executable, script, '--daemon', '--no-ui'],\n"
                f"    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,\n"
                f"    start_new_session=True)\n"
            )
            subprocess.Popen(
                [sys.executable, "-c", helper],
                stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                start_new_session=True,
            )
            request_quit()

        def _refresh_monitor_dropdown(self):
            try:
                for ch in list(self.mon_pop_box.get_children()):
                    self.mon_pop_box.remove(ch)
            except Exception:
                pass

            choices = get_monitor_choices_for_ui()
            active_label = None
            for lbl, mode, idx in choices:
                if self._mon_choice_mode == mode:
                    if mode != "index" or idx == self._mon_choice_index:
                        active_label = lbl
                        break

            if active_label is None and self._mon_choice_mode == "index":
                mons = [c for c in choices if c[1] == "index"]
                if mons:
                    self._mon_choice_index = _clamp(self._mon_choice_index, 0, len(mons) - 1)
                    active_label = mons[self._mon_choice_index][0]

            if active_label is None:
                self._mon_choice_mode = "all"
                self._mon_choice_index = 0
                active_label = "all"

            self.mon_btn_label.set_text(active_label)

            def add_item(lbl, mode, idx):
                btn = Gtk.ModelButton(label=lbl)
                btn.set_hexpand(True)
                btn.set_halign(Gtk.Align.FILL)

                def _pick(*_):
                    self._mon_choice_mode = mode
                    if mode == "index" and idx is not None:
                        self._mon_choice_index = int(idx)
                    self.mon_btn_label.set_text(lbl)
                    try:
                        self.mon_pop.popdown()
                    except Exception:
                        try:
                            self.mon_pop.hide()
                        except Exception:
                            pass
                    self._changed(rebuild=True)

                btn.connect("clicked", _pick)
                self.mon_pop_box.pack_start(btn, False, False, 0)

            for lbl, mode, idx in choices:
                add_item(lbl, mode, idx)

            self.mon_pop_box.show_all()
            self.mon_pop.show_all()

        def refresh_monitor_options(self):
            self._refresh_monitor_dropdown()
            self._update_statusbar()

        def _sync_ui_from_cfg(self):
            """Push current cfg values back into all UI widgets."""
            self._suppress_changes = True
            try:
                self.chk_enabled.set_active(bool(cfg.get("enabled", True)))
                self.chk_auto_save.set_active(bool(cfg.get("auto_save", True)))
                shape = cfg.get("shape", "dot")
                if shape in SHAPES:
                    self.cmb_shape.set_active(SHAPES.index(shape))
                self.scale_opacity.set_value(float(cfg.get("opacity", 0.95)))
                self.scale_size.set_value(float(cfg.get("size", 5)))
                self.scale_thick.set_value(float(cfg.get("thickness", 2)))
                self.scale_gap.set_value(float(cfg.get("gap", 3)))
                self.btn_color.set_rgba(self._gdk_rgba_from_hex(cfg.get("color", "#a000ff")))
                o = cfg.get("outline", {})
                self.chk_outline.set_active(bool(o.get("enabled", True)))
                self.btn_outline_color.set_rgba(self._gdk_rgba_from_hex(o.get("color", "#000000")))
                self.scale_outline_op.set_value(float(o.get("opacity", 0.8)))
                self.scale_outline_th.set_value(float(o.get("thickness", 1)))
                s = cfg.get("shadow", {})
                self.chk_shadow.set_active(bool(s.get("enabled", False)))
                self.btn_shadow_color.set_rgba(self._gdk_rgba_from_hex(s.get("color", "#000000")))
                self.scale_shadow_op.set_value(float(s.get("opacity", 0.35)))
                self.spin_sh_x.set_value(float(s.get("offset_x", 1)))
                self.spin_sh_y.set_value(float(s.get("offset_y", 1)))
                self.btn_preview_bg.set_rgba(self._gdk_rgba_from_hex(cfg.get("preview_bg", "#222222")))
                self._mon_choice_mode = str(cfg.get("monitor_mode", "all"))
                self._mon_choice_index = int(cfg.get("monitor_index", 0) or 0)
                self._mon_choice_name = str(cfg.get("monitor_name", ""))
                self._refresh_monitor_dropdown()
            finally:
                self._suppress_changes = False
            self.preview.queue_draw()
            self._update_statusbar()

        def _read_ui_to_cfg(self):
            cfg["enabled"] = bool(self.chk_enabled.get_active())
            cfg["auto_save"] = bool(self.chk_auto_save.get_active())
            cfg["shape"] = self.cmb_shape.get_active_text() or "dot"
            cfg["monitor_mode"] = str(self._mon_choice_mode or "all")
            cfg["monitor_index"] = int(self._mon_choice_index or 0)
            cfg["monitor_name"] = str(self._mon_choice_name or "")
            cfg["opacity"] = float(self.scale_opacity.get_value())
            cfg["size"] = int(self.scale_size.get_value())
            cfg["thickness"] = int(self.scale_thick.get_value())
            cfg["gap"] = int(self.scale_gap.get_value())
            c = self.btn_color.get_rgba()
            cfg["color"] = _rgba_to_hex((c.red, c.green, c.blue, 1.0))
            cfg["outline"]["enabled"] = bool(self.chk_outline.get_active())
            oc = self.btn_outline_color.get_rgba()
            cfg["outline"]["color"] = _rgba_to_hex((oc.red, oc.green, oc.blue, 1.0))
            cfg["outline"]["opacity"] = float(self.scale_outline_op.get_value())
            cfg["outline"]["thickness"] = int(self.scale_outline_th.get_value())
            cfg["shadow"]["enabled"] = bool(self.chk_shadow.get_active())
            sc = self.btn_shadow_color.get_rgba()
            cfg["shadow"]["color"] = _rgba_to_hex((sc.red, sc.green, sc.blue, 1.0))
            cfg["shadow"]["opacity"] = float(self.scale_shadow_op.get_value())
            cfg["shadow"]["offset_x"] = int(self.spin_sh_x.get_value())
            cfg["shadow"]["offset_y"] = int(self.spin_sh_y.get_value())
            sanitize_config(cfg)

        def _changed(self, rebuild=False):
            if self._suppress_changes:
                return
            if rebuild:
                self._pending_rebuild = True
            if self._debounce_id is not None:
                try:
                    GLib.source_remove(self._debounce_id)
                except Exception:
                    pass
                self._debounce_id = None

            def do_apply():
                self._debounce_id = None
                do_rebuild = self._pending_rebuild
                self._pending_rebuild = False
                self._read_ui_to_cfg()
                apply_config(rebuild=do_rebuild)
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
                self.preview.queue_draw()
                self._update_statusbar()
                return False

            self._debounce_id = GLib.timeout_add(60, do_apply)

        def _save(self):
            self._read_ui_to_cfg()
            save_config(cfg, cfg_path)
            self._update_statusbar()
            self._info_dialog("Saved", f"Config saved to:\n{cfg_path}")

        def _draw_preview(self, widget, cr):
            alloc = widget.get_allocation()
            w = float(alloc.width)
            h = float(alloc.height)
            bg = _hex_to_rgba(cfg.get("preview_bg", "#222222"), 1.0)
            cr.set_source_rgba(*bg)
            cr.rectangle(0, 0, w, h)
            cr.fill()
            if cfg.get("enabled", True):
                _draw_crosshair(cr, w * 0.5, h * 0.5, cfg)
            return False

    settings_window = SettingsWindow()

    # apply config
    def apply_config(rebuild=False):
        sanitize_config(cfg)
        if rebuild:
            rebuild_windows()
        for w in windows:
            w.apply_config()

    # tray
    def _setup_tray():
        if HAVE_APPINDICATOR:
            indicator = AppIndicator3.Indicator.new(
                APP_ID,
                "cross",
                AppIndicator3.IndicatorCategory.APPLICATION_STATUS,
            )
            icon_names = ["crosshairs", "input-mouse", "utilities-system-monitor",
                          "applications-games"]
            for name in icon_names:
                try:
                    theme = Gtk.IconTheme.get_default()
                    if theme.has_icon(name):
                        try:
                            indicator.set_icon_full(name, APP_NAME)
                        except AttributeError:
                            indicator.set_icon(name)
                        break
                except Exception:
                    pass

            indicator.set_status(AppIndicator3.IndicatorStatus.ACTIVE)
            menu = Gtk.Menu()

            def _mi(label, fn):
                item = Gtk.MenuItem(label=label)
                item.connect("activate", lambda *_: fn())
                menu.append(item)
                return item

            def _open_settings():
                settings_window.show_all()
                settings_window.present()
                return False

            _mi(f"{APP_NAME} Settings",
                lambda: GLib.idle_add(_open_settings))

            def _tray_toggle():
                cfg["enabled"] = not cfg.get("enabled", True)
                apply_config()
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
                # only sync the settings UI if the window is open -- no point
                # updating 15 widgets on a hidden window
                if settings_window.get_visible():
                    settings_window._sync_ui_from_cfg()
                else:
                    settings_window._update_statusbar()

            _mi("Toggle", _tray_toggle)
            menu.append(Gtk.SeparatorMenuItem())
            _mi("Restart", lambda: settings_window._restart())
            _mi("Quit", request_quit)
            menu.show_all()
            indicator.set_menu(menu)
            return indicator

        else:
            # Gtk.StatusIcon fallback (deprecated but available everywhere)
            try:
                icon = Gtk.StatusIcon()
                icon.set_from_icon_name("input-mouse")
                icon.set_tooltip_text(APP_NAME)
                def _open_settings():
                    settings_window.show_all()
                    settings_window.present()
                    return False

                icon.connect("activate",
                             lambda *_: GLib.idle_add(_open_settings))
                icon.connect("popup-menu", lambda icon, btn, t: _tray_popup(icon, btn, t))
                icon.set_visible(True)

                def _tray_popup(icon, btn, t):
                    menu = Gtk.Menu()

                    def _mi(label, fn):
                        item = Gtk.MenuItem(label=label)
                        item.connect("activate", lambda *_: fn())
                        menu.append(item)

                    def _open_settings():
                        settings_window.show_all()
                        settings_window.present()
                        return False

                    _mi("Settings",
                        lambda: GLib.idle_add(_open_settings))

                    def _tray_toggle():
                        cfg["enabled"] = not cfg.get("enabled", True)
                        apply_config()
                        if cfg.get("auto_save", True):
                            save_config(cfg, cfg_path)
                        # sync settings window checkbox -- was missing before
                        if settings_window.get_visible():
                            settings_window._sync_ui_from_cfg()
                        else:
                            settings_window._update_statusbar()

                    _mi("Toggle", _tray_toggle)
                    menu.append(Gtk.SeparatorMenuItem())
                    _mi("Restart", lambda: settings_window._restart())
                    _mi("Quit", request_quit)
                    menu.show_all()
                    menu.popup(None, None, None, None, btn, t)

                return icon
            except Exception:
                return None

    tray = _setup_tray()

    # ipc command handling
    should_quit = {"value": False}

    def _do_quit():
        should_quit["value"] = True
        try:
            Path(PID_FILE).unlink(missing_ok=True)
        except Exception:
            pass
        Gtk.main_quit()

    _quit_callbacks.append(_do_quit)

    def handle_ipc(req):
        cmd = str(req.get("cmd", "")).lower()
        payload = req.get("payload", None)

        def _gl(fn, *args):
            GLib.idle_add(fn, *args, priority=GLib.PRIORITY_DEFAULT)

        if cmd == "toggle":
            def _t():
                cfg["enabled"] = not bool(cfg.get("enabled", True))
                apply_config()
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
                if settings_window.get_visible():
                    settings_window._sync_ui_from_cfg()
                else:
                    settings_window._update_statusbar()
            _gl(_t)
            return "ok"
        if cmd == "show":
            def _s():
                cfg["enabled"] = True
                apply_config()
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
            _gl(_s)
            return "ok"
        if cmd == "hide":
            def _h():
                cfg["enabled"] = False
                apply_config()
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
            _gl(_h)
            return "ok"
        if cmd == "config":
            def _c():
                settings_window.show_all()
                settings_window.present()
            _gl(_c)
            return "ok"
        if cmd == "quit":
            _gl(request_quit)
            return "ok"
        if cmd == "reload":
            def _r():
                loaded = load_config(cfg_path)
                cfg.clear()
                cfg.update(loaded)
                apply_config(rebuild=True)
                settings_window._sync_ui_from_cfg()
            _gl(_r)
            return "ok"
        if cmd == "restart":
            _gl(settings_window._restart)
            return "ok"
        if cmd == "set" and isinstance(payload, dict):
            def _set():
                merged = sanitize_config(deep_merge(cfg, payload))
                cfg.clear()
                cfg.update(merged)
                apply_config(rebuild=True)
                if cfg.get("auto_save", True):
                    save_config(cfg, cfg_path)
            _gl(_set)
            return "ok"
        return "unknown"

    # start IPC server thread
    sock_path = DEFAULT_SOCKET
    t = threading.Thread(target=ipc_server, args=(sock_path, handle_ipc), daemon=True)
    t.start()

    # config file watcher
    _cfg_mtime = [0.0]
    try:
        p = Path(cfg_path)
        if p.exists():
            _cfg_mtime[0] = p.stat().st_mtime
    except Exception:
        pass

    def _watch_config_file():
        try:
            p = Path(cfg_path)
            if not p.exists():
                return True
            mt = p.stat().st_mtime
            if mt != _cfg_mtime[0]:
                _cfg_mtime[0] = mt
                new_cfg = load_config(cfg_path)
                cfg.clear()
                cfg.update(new_cfg)
                apply_config(rebuild=True)
                settings_window._sync_ui_from_cfg()
        except Exception:
            pass
        return True

    GLib.timeout_add(2000, _watch_config_file)

    # monitor change detection (signal + polling fallback)
    _last_mon_count = [len(get_all_monitors())]

    def on_mon_change(*_):
        apply_config(rebuild=True)
        try:
            settings_window.refresh_monitor_options()
        except Exception:
            pass

    try:
        display.connect("monitor-added", on_mon_change)
        display.connect("monitor-removed", on_mon_change)
    except Exception:
        pass

    def _poll_monitors():
        n = len(get_all_monitors())
        if n != _last_mon_count[0]:
            _last_mon_count[0] = n
            on_mon_change()
        return True

    GLib.timeout_add(3000, _poll_monitors)

    if open_config:
        GLib.idle_add(lambda: (settings_window.show_all(), settings_window.present(), False)[-1])

    Gtk.main()

    # cleanup
    try:
        sp = Path(sock_path)
        if sp.exists():
            sp.unlink()
    except Exception:
        pass
    try:
        Path(PID_FILE).unlink(missing_ok=True)
    except Exception:
        pass

    return 0

# --- misc

def start_daemon_background():
    try:
        subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--daemon", "--no-ui"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        return True
    except Exception:
        return False

def doctor():
    print(f"{APP_NAME} v{APP_VERSION} doctor")
    print(f"Session type: {os.environ.get('XDG_SESSION_TYPE', '(unknown)')}")
    print(f"Config:  {DEFAULT_CONFIG}")
    print(f"Socket:  {DEFAULT_SOCKET}")
    print(f"PID:     {PID_FILE}")
    import importlib
    for label, gi_ns, ver, pkg in [
        ("GTK3",          "Gtk",          "3.0", "python3-gobject + gtk3"),
        ("GtkLayerShell", "GtkLayerShell","0.1", "gtk-layer-shell"),
        ("AppIndicator3", "AppIndicator3","0.1", "libappindicator3-1"),
        ("pycairo",       None,           None,  "python3-cairo / python3-gi-cairo"),
    ]:
        try:
            if gi_ns is None:
                importlib.import_module("cairo")
            else:
                import gi
                gi.require_version(gi_ns, ver)
                importlib.import_module(f"gi.repository.{gi_ns}")
            print(f"  {label}: OK")
        except Exception as e:
            print(f"  {label}: MISSING -- {pkg}")
            print(f"    {e}")
    pid = read_pid_file()
    if pid:
        alive = pid_is_alive(pid)
        print(f"Daemon PID {pid}: {'running' if alive else 'stale (not running)'}")
    else:
        print("No PID file found -- daemon not running.")
    return 0

# --- CLI

def main():
    parser = argparse.ArgumentParser(prog=APP_ID, add_help=True,
                                     description=f"{APP_NAME} v{APP_VERSION} -- crosshair overlay")
    parser.add_argument("--daemon",           action="store_true", help="Run overlay daemon")
    parser.add_argument("--config",           action="store_true", help="Open config UI (talks to daemon)")
    parser.add_argument("--toggle",           action="store_true", help="Toggle enabled")
    parser.add_argument("--show",             action="store_true", help="Enable/show")
    parser.add_argument("--hide",             action="store_true", help="Disable/hide")
    parser.add_argument("--quit",             action="store_true", help="Quit daemon")
    parser.add_argument("--restart",          action="store_true", help="Restart daemon")
    parser.add_argument("--reload",           action="store_true", help="Reload config from disk")
    parser.add_argument("--status",           action="store_true", help="Print daemon status and config summary")
    parser.add_argument("--set",              type=str,            help="Set config keys via JSON")
    parser.add_argument("--install",          action="store_true", help="Install into ~/.local/bin")
    parser.add_argument("--autostart",        action="store_true", help="Install + set up autostart")
    parser.add_argument("--desktop-shortcut", action="store_true",
                        help="Create a double-clickable .desktop icon on ~/Desktop")
    parser.add_argument("--no-ui",            action="store_true", help="Daemon: do not open config UI on start")
    parser.add_argument("--doctor",           action="store_true", help="Print dependency diagnostics")
    parser.add_argument("--kill-all",         action="store_true",
                        help="Kill ALL running instances of this script (nuclear option)")
    args = parser.parse_args()

    if args.doctor:
        return doctor()

    if args.status:
        pid = read_pid_file()
        alive = bool(pid and pid_is_alive(pid))
        print(f"daemon: {'running (pid ' + str(pid) + ')' if alive else 'not running'}")
        cfg = load_config()
        print(f"config:  {DEFAULT_CONFIG}")
        print(f"enabled: {cfg.get('enabled', True)}")
        print(f"shape:   {cfg.get('shape', 'dot')}")
        print(f"color:   {cfg.get('color', '#a000ff')}")
        print(f"monitor: {cfg.get('monitor_mode', 'all')}")
        return 0

    if args.kill_all:
        n = kill_all_python_crosshair()
        kill_existing_daemon()
        print(f"Killed {n} instance(s).")
        return 0

    if args.install:
        py_path, bin_path = install_self()
        print(f"Script:  {py_path}")
        print(f"Command: {bin_path}")
        return 0

    if args.autostart:
        unit_path, desktop_path, sysd_ok, bin_path = install_autostart()
        print(f"Command:  {bin_path}")
        print(f"Systemd:  {unit_path}")
        print(f"Desktop:  {desktop_path}")
        print("Autostart:", "enabled (systemd user)" if sysd_ok else "fallback (.desktop)")
        return 0

    if args.desktop_shortcut:
        path = install_desktop_shortcut()
        print(f"Desktop shortcut created: {path}")
        print("Double-click it in your file manager to launch.")
        print("Do NOT run it from terminal -- it is a launcher, not a shell script.")
        return 0

    if args.daemon:
        return run_daemon(open_config=(not args.no_ui))

    def _send_or_start(cmd, payload=None, need_daemon=True):
        ok, _resp = ipc_send(cmd, payload=payload)
        if ok:
            return True
        if cmd == "quit":
            print("Daemon not running.")
            return False
        if not need_daemon:
            return False
        started = start_daemon_background()
        if not started:
            return False
        for _ in range(80):
            time.sleep(0.05)
            ok, _resp = ipc_send(cmd, payload=payload)
            if ok:
                return True
        return False

    if args.restart:
        ok, _ = ipc_send("restart")
        if not ok:
            start_daemon_background()
        return 0

    explicit = (args.toggle or args.show or args.hide or args.config or
                args.quit or args.reload or (args.set is not None))

    if explicit:
        cmd = None
        payload = None
        if args.toggle:       cmd = "toggle"
        elif args.show:       cmd = "show"
        elif args.hide:       cmd = "hide"
        elif args.config:     cmd = "config"
        elif args.quit:       cmd = "quit"
        elif args.reload:     cmd = "reload"
        elif args.set is not None:
            cmd = "set"
            try:
                payload = json.loads(args.set)
                if not isinstance(payload, dict):
                    raise ValueError("JSON must be an object")
            except Exception as e:
                print('--set expects a JSON object, e.g. --set \'{"enabled": false}\'',
                      file=sys.stderr)
                print(str(e), file=sys.stderr)
                return 2

        if _send_or_start(cmd, payload=payload):
            return 0
        print("Could not contact daemon.", file=sys.stderr)
        return 2

    # no args: start daemon, or bring up config if already running
    if _send_or_start("config"):
        return 0
    return run_daemon(open_config=True)


if __name__ == "__main__":
    raise SystemExit(main())
