#!/usr/bin/env python3
# Crosshair.py -- GTK3 crosshair overlay for Linux (Wayland + X11)
#
# v3.2.0 hardening pass (Nobara/KDE audit):
#   - Removed all pgrep/pkill/PID-file process-name killing (C-01/C-02).
#     Single-instance is now an advisory flock under $XDG_RUNTIME_DIR.
#   - Config load/save moved to crosshair_core: atomic writes, symlink
#     refusal, corrupt-file preservation, strict IPC validation (C-05/H-16).
#   - IPC is a bounded-size JSON protocol with peer-UID checking and a
#     boolean `ok` on every response (H-06).
#   - Actual GDK backend is detected (not just $XDG_SESSION_TYPE); native
#     Wayland without GTK Layer Shell now fails clearly instead of drawing
#     an unmanaged fallback window (C-06/C-07).
#   - Autostart is XDG-only; the old systemd user service is removed on
#     install/uninstall (C-03).
#   - Restart uses a pipe-EOF handoff so the replacement never races the
#     outgoing process for the lock/socket (H-11).
#   - See INTEGRATION.md and the release-gate checklist for the full list.
#
# 28/05/2026 patch (v3.1, preserved behavior): deduped draw code, X11 input
#   passthrough + type hint, dot_ring shape, --status flag, active-monitor
#   in dropdown, fix tray toggle not syncing checkbox, fix Gtk.Arrow
#   deprecation, fix scale label layout, math.tau, ASCII section separators

import argparse
import contextlib
import importlib
import json
import logging
import math
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

import crosshair_core as core

APP_ID = core.APP_ID
APP_NAME = core.APP_NAME
APP_VERSION = core.APP_VERSION

# --- small pure helpers (unchanged from v3.1) ---------------------------

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
    ri = int(_clamp(r, 0, 1) * 255)
    gi = int(_clamp(g, 0, 1) * 255)
    bi = int(_clamp(b, 0, 1) * 255)
    return f"#{ri:02x}{gi:02x}{bi:02x}"

def _sanitize_inplace(cfg: dict) -> dict:
    """Sanitize cfg in place -- matches the mutation pattern the GUI relies on."""
    clean = core.sanitize_config(cfg)
    cfg.clear()
    cfg.update(clean)
    return cfg

# --- installer / autostart (XDG-only, atomic, symlink-safe) -------------

def _desktop_entry_exec(bin_path: Path) -> str:
    # Desktop Entry Specification quoting: wrap in double quotes, escape
    # embedded backslashes/quotes.
    escaped = str(bin_path).replace("\\", "\\\\").replace('"', '\\"')
    return f'"{escaped}" --daemon --no-ui'

def _validate_desktop_file(path: Path) -> None:
    exe = shutil.which("desktop-file-validate")
    if not exe:
        print(f"Note: desktop-file-validate not found; skipped validation of {path}",
              file=sys.stderr)
        return
    try:
        result = subprocess.run([exe, str(path)], capture_output=True, text=True, timeout=5)
    except (OSError, subprocess.TimeoutExpired) as exc:
        raise core.CoreError(f"Could not run desktop-file-validate: {exc}") from exc
    if result.returncode != 0:
        with contextlib.suppress(OSError):
            path.unlink(missing_ok=True)
        raise core.CoreError(
            f"Generated desktop entry failed validation, removed:\n"
            f"{result.stdout}{result.stderr}".strip()
        )

def _desktop_dir() -> Path:
    exe = shutil.which("xdg-user-dir")
    if exe:
        try:
            result = subprocess.run([exe, "DESKTOP"], capture_output=True, text=True, timeout=3)
            candidate = result.stdout.strip()
            if result.returncode == 0 and candidate:
                return Path(candidate)
        except (OSError, subprocess.TimeoutExpired):
            pass
    return Path.home() / "Desktop"

def _remove_legacy_systemd_unit() -> list[str]:
    """Remove the v3.1 systemd user service, if present (C-03 migration)."""
    removed = []
    unit = Path.home() / ".config" / "systemd" / "user" / f"{APP_ID}.service"
    if unit.exists():
        try:
            subprocess.run(["systemctl", "--user", "disable", "--now", f"{APP_ID}.service"],
                           check=False, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
                           timeout=5)
            subprocess.run(["systemctl", "--user", "daemon-reload"], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=5)
        except (OSError, subprocess.TimeoutExpired):
            pass
        try:
            unit.unlink()
            removed.append(str(unit))
        except OSError:
            pass
    return removed

def _migrate_legacy_config(paths: core.AppPaths, log: logging.Logger) -> None:
    """One-time copy from the old purplecrosshair config location.

    Explicit and logged, called only from run_daemon() startup -- never at
    import time, so --help/--doctor/--status never touch the filesystem
    for this.
    """
    old_config = paths.home / ".config" / "purplecrosshair" / "config.json"
    if not old_config.exists() or paths.config_file.exists():
        return
    if old_config.is_symlink():
        log.warning("Skipping legacy config migration: source is a symlink (%s)", old_config)
        return
    try:
        data = old_config.read_bytes()
        core.atomic_write_bytes(paths.config_file, data, mode=0o600)
        log.info("Migrated legacy config from %s to %s", old_config, paths.config_file)
    except OSError as exc:
        log.warning("Legacy config migration failed: %s", exc)

def install_self(paths: core.AppPaths) -> tuple[Path, Path]:
    src = Path(__file__).resolve()
    if src.is_symlink():
        raise core.CoreError(f"Refusing to install from symlink source: {src}")
    core.atomic_write_bytes(paths.installed_source, src.read_bytes(), mode=0o644)
    launcher = f'#!/usr/bin/env bash\nexec python3 "{paths.installed_source}" "$@"\n'
    core.atomic_write_text(paths.bin_file, launcher, mode=0o755)
    app_entry = (
        "[Desktop Entry]\n"
        "Version=1.0\n"
        "Type=Application\n"
        f"Name={APP_NAME}\n"
        "Comment=Crosshair overlay for games without a built-in reticle\n"
        f"Exec={_desktop_entry_exec(paths.bin_file)}\n"
        "Icon=input-gaming\n"
        "Terminal=false\n"
        "Categories=Game;\n"
        "StartupNotify=false\n"
    )
    core.atomic_write_text(paths.application_entry, app_entry, mode=0o644)
    _validate_desktop_file(paths.application_entry)
    return paths.installed_source, paths.bin_file

def install_autostart(paths: core.AppPaths, log: logging.Logger | None = None) -> tuple[Path, Path]:
    _, bin_path = install_self(paths)
    content = (
        "[Desktop Entry]\n"
        "Type=Application\n"
        "Version=1.0\n"
        f"Name={APP_NAME}\n"
        "Comment=Crosshair overlay\n"
        f"TryExec={bin_path}\n"
        f"Exec={_desktop_entry_exec(bin_path)}\n"
        "Icon=input-gaming\n"
        "Terminal=false\n"
        "Categories=Utility;\n"
        "StartupNotify=false\n"
        "X-KDE-autostart-after=panel\n"
        "NoDisplay=true\n"
    )
    core.atomic_write_text(paths.autostart_entry, content, mode=0o644)
    _validate_desktop_file(paths.autostart_entry)
    removed_legacy = _remove_legacy_systemd_unit()
    if removed_legacy and log:
        log.info("Removed legacy systemd autostart unit(s): %s", removed_legacy)
    return bin_path, paths.autostart_entry

def install_desktop_shortcut(paths: core.AppPaths) -> Path:
    _, bin_path = install_self(paths)
    desktop_dir = _desktop_dir()
    if desktop_dir.is_symlink():
        raise core.CoreError(f"Refusing symlink desktop directory: {desktop_dir}")
    desktop_dir.mkdir(parents=True, exist_ok=True)
    shortcut = desktop_dir / f"{APP_ID}.desktop"
    content = (
        "[Desktop Entry]\n"
        "Version=1.0\n"
        "Type=Application\n"
        f"Name={APP_NAME}\n"
        "Comment=Crosshair overlay -- click to open settings\n"
        f"Exec={_desktop_entry_exec(bin_path)}\n"
        "Icon=input-gaming\n"
        "Terminal=false\n"
        "Categories=Utility;\n"
        "StartupNotify=false\n"
    )
    core.atomic_write_text(shortcut, content, mode=0o755)
    _validate_desktop_file(shortcut)
    return shortcut

def uninstall_app(paths: core.AppPaths) -> list[str]:
    removed = []
    targets = (
        paths.autostart_entry, paths.application_entry, paths.bin_file, paths.installed_source,
    )
    for p in targets:
        try:
            if p.exists() or p.is_symlink():
                p.unlink()
                removed.append(str(p))
        except OSError:
            pass
    removed += _remove_legacy_systemd_unit()
    for d in (paths.installed_source.parent, paths.application_entry.parent):
        try:
            if d.exists() and not any(d.iterdir()):
                d.rmdir()
        except OSError:
            pass
    return removed

def purge_config(paths: core.AppPaths) -> list[str]:
    removed = []
    try:
        if paths.config_dir.exists():
            for p in sorted(paths.config_dir.glob("*")):
                if p.is_file() or p.is_symlink():
                    p.unlink()
                    removed.append(str(p))
            if not any(paths.config_dir.iterdir()):
                paths.config_dir.rmdir()
                removed.append(str(paths.config_dir))
    except OSError as exc:
        print(f"Could not fully purge config: {exc}", file=sys.stderr)
    return removed

def start_daemon_background(paths: core.AppPaths) -> bool:
    try:
        subprocess.Popen(
            [sys.executable, str(Path(__file__).resolve()), "--daemon", "--no-ui"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
            start_new_session=True,
        )
        return True
    except OSError:
        return False

# --- doctor ---------------------------------------------------------------

def doctor(paths: core.AppPaths) -> int:
    mandatory_ok = True
    lines = [f"{APP_NAME} v{APP_VERSION} doctor"]
    session_hint = os.environ.get("XDG_SESSION_TYPE", "").strip().lower()
    lines.append(f"XDG_SESSION_TYPE (hint only, not authoritative): {session_hint or '(unset)'}")
    lines.append(f"Config:   {paths.config_file}")
    lines.append(f"Socket:   {paths.socket_file}")
    lines.append(f"Lock:     {paths.lock_file}")
    lines.append(f"Log:      {paths.log_file}")
    if paths.runtime_fallback:
        lines.append("WARNING: XDG_RUNTIME_DIR is unset/invalid; using a private cache "
                      "fallback. A normal desktop login should provide a valid runtime dir.")

    checks = [
        ("GTK3",          "Gtk",           "3.0", True),
        ("Gdk",           "Gdk",           "3.0", True),
        ("pycairo",       None,            None,  True),
        ("GtkLayerShell", "GtkLayerShell", "0.1", session_hint == "wayland"),
    ]
    try:
        import gi
    except Exception as exc:
        lines.append(f"  [MISS] PyGObject (MANDATORY): {exc}")
        mandatory_ok = False
        gi = None

    for label, ns, ver, mandatory in checks:
        try:
            if ns is None:
                importlib.import_module("cairo")
            else:
                if gi is None:
                    raise RuntimeError("PyGObject not available")
                gi.require_version(ns, ver)
                importlib.import_module(f"gi.repository.{ns}")
            lines.append(f"  [OK]   {label}")
        except Exception as exc:
            marker = "MANDATORY" if mandatory else "optional"
            lines.append(f"  [MISS] {label} ({marker}): {exc}")
            if mandatory:
                mandatory_ok = False

    tray_ok = False
    if gi is not None:
        for ns in ("AyatanaAppIndicator3", "AppIndicator3"):
            try:
                gi.require_version(ns, "0.1")
                importlib.import_module(f"gi.repository.{ns}")
                lines.append(f"  [OK]   tray via {ns}")
                tray_ok = True
                break
            except Exception:
                continue
    if not tray_ok:
        lines.append("  [WARN] no AppIndicator backend found; tray falls back to Gtk.StatusIcon "
                      "(this never blocks the overlay)")

    try:
        response = core.ipc_call(paths, {"cmd": "status"}, timeout=0.5)
        lines.append(f"Daemon:   running (pid {response.get('pid', '?')}, "
                      f"backend={response.get('backend', '?')}, "
                      f"layershell={response.get('layershell', '?')})")
    except core.IpcUnavailable:
        lines.append("Daemon:   not running")

    print("\n".join(lines))
    return 0 if mandatory_ok else 1

# --- daemon (GTK overlay + UI) --------------------------------------------

def run_daemon(paths: core.AppPaths, open_config: bool = False, verbose: bool = False) -> int:
    import warnings
    warnings.filterwarnings("ignore", category=DeprecationWarning)

    log = core.setup_logging(paths, verbose=verbose)
    log.info("Starting %s v%s (pid %s)", APP_NAME, APP_VERSION, os.getpid())
    if paths.runtime_fallback:
        log.warning("XDG_RUNTIME_DIR unset/invalid; using private fallback: %s", paths.runtime_dir)

    _migrate_legacy_config(paths, log)

    try:
        import gi
        gi.require_version("Gtk", "3.0")
        gi.require_version("Gdk", "3.0")
        from gi.repository import Gdk, GLib, GObject, Gtk
    except Exception as exc:
        msg = "GTK3 (PyGObject) is not available. Install python3-gobject + gtk3."
        print(msg, file=sys.stderr)
        print(str(exc), file=sys.stderr)
        log.error("%s -- %s", msg, exc)
        return 2

    try:
        def _gtk_log_suppress(domain, level, message, user_data):
            pass
        GLib.log_set_handler("Gtk", GLib.LogLevelFlags.LEVEL_WARNING, _gtk_log_suppress, None)
    except Exception:
        pass

    try:
        import cairo
    except Exception as exc:
        msg = "pycairo is not available. Install python3-cairo / python3-gi-cairo."
        print(msg, file=sys.stderr)
        print(str(exc), file=sys.stderr)
        log.error("%s -- %s", msg, exc)
        return 2

    display = Gdk.Display.get_default()
    if display is None:
        msg = "No graphical display detected."
        print(msg, file=sys.stderr)
        log.error(msg)
        return 2

    def _detect_backend(disp):
        # The GObject type name is intrinsic to the C library and is reliable
        # even when the optional GdkWayland/GdkX11 GI namespaces aren't
        # installed -- unlike $XDG_SESSION_TYPE, which can disagree with the
        # actual GDK backend under XWayland.
        try:
            type_name = GObject.type_name(disp)
        except Exception:
            type_name = type(disp).__name__
        lowered = type_name.lower()
        for key in ("wayland", "x11", "broadway", "win32", "quartz"):
            if key in lowered:
                return key
        return os.environ.get("XDG_SESSION_TYPE", "").strip().lower() or "unknown"

    backend = _detect_backend(display)
    is_wayland = (backend == "wayland")
    log.info("Detected GDK backend: %s", backend)

    HAVE_LAYERSHELL = False
    GtkLayerShell = None
    try:
        gi.require_version("GtkLayerShell", "0.1")
        from gi.repository import GtkLayerShell as _GtkLayerShell
        GtkLayerShell = _GtkLayerShell
        HAVE_LAYERSHELL = bool(GtkLayerShell.is_supported()) if hasattr(
            GtkLayerShell, "is_supported") else True
    except Exception:
        HAVE_LAYERSHELL = False

    if is_wayland and not HAVE_LAYERSHELL:
        msg = ("Native Wayland session detected but GTK Layer Shell is unavailable or "
               "unsupported by this compositor. Install gtk-layer-shell (Nobara/Fedora: "
               "'sudo dnf install gtk-layer-shell') and retry. Refusing to fall back to an "
               "unmanaged window that cannot guarantee placement/stacking.")
        print(msg, file=sys.stderr)
        log.error(msg)
        return 2
    use_layershell = bool(is_wayland and HAVE_LAYERSHELL)

    HAVE_APPINDICATOR = False
    AppIndicator3 = None
    for _ns in ("AyatanaAppIndicator3", "AppIndicator3"):
        try:
            gi.require_version(_ns, "0.1")
            AppIndicator3 = importlib.import_module(f"gi.repository.{_ns}")
            HAVE_APPINDICATOR = True
            log.info("Tray backend: %s", _ns)
            break
        except Exception:
            continue
    if not HAVE_APPINDICATOR:
        log.info("No AppIndicator backend found; tray falls back to Gtk.StatusIcon")

    try:
        core.ensure_runtime(paths)
    except core.CoreError as exc:
        print(str(exc), file=sys.stderr)
        log.error(str(exc))
        return 2

    result = core.load_config(paths.config_file, log)
    cfg = result.config
    _cfg_fingerprint = [core.file_fingerprint(paths.config_file)]

    def _save_cfg():
        core.save_config(cfg, paths.config_file)
        _cfg_fingerprint[0] = core.file_fingerprint(paths.config_file)

    def _save_if_needed(was_auto_save: bool, now_auto_save: bool):
        # Always save when auto-save is on, AND force one save on the
        # on->off transition so that turning auto-save off is itself
        # persisted (H-03) -- otherwise the fact that it's now off would
        # only reach disk on some later externally-triggered save. Shared
        # by both the settings-window debounce handler and the IPC "set"
        # command, which can independently flip auto_save.
        if now_auto_save or (was_auto_save and not now_auto_save):
            _save_cfg()

    # Acquire the single-instance lock and bind the IPC socket now -- BEFORE
    # building any windows, the settings UI, or the tray. A duplicate launch
    # must fail fast here without doing GTK/tray/D-Bus work (tray setup can
    # block for seconds, or hang entirely, on a system with no session bus).
    # handle_ipc references cfg/apply_config/settings_window/etc. that don't
    # exist yet -- that's fine, since IpcServer only calls the dispatcher
    # once a real request arrives, and _ipc_handler_ref is filled in once
    # handle_ipc is fully defined further down, well before Gtk.main() runs.
    _ipc_handler_ref = {"fn": None}

    def _dispatch_ipc(req):
        fn = _ipc_handler_ref["fn"]
        if fn is None:
            return {"ok": False, "message": "daemon is still starting up"}
        return fn(req)

    server = core.IpcServer(paths, _dispatch_ipc, log)
    try:
        server.start()
    except core.AlreadyRunning:
        msg = f"{APP_NAME} daemon is already running (lock held: {paths.lock_file})."
        print(msg, file=sys.stderr)
        log.error(msg)
        return 3
    except core.CoreError as exc:
        print(str(exc), file=sys.stderr)
        log.error(str(exc))
        return 2

    def _wrap_scrollable(widget):
        scrolled = Gtk.ScrolledWindow()
        scrolled.set_policy(Gtk.PolicyType.NEVER, Gtk.PolicyType.AUTOMATIC)
        scrolled.set_min_content_height(260)
        scrolled.add(widget)
        return scrolled

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
                cr.move_to(cx+dx,   cy+dy-g)
                cr.line_to(cx+dx,   cy+dy-(g+size))
                cr.move_to(cx+dx,   cy+dy+g)
                cr.line_to(cx+dx,   cy+dy+(g+size))
                cr.move_to(cx+dx-g, cy+dy)
                cr.line_to(cx+dx-(g+size), cy+dy)
                cr.move_to(cx+dx+g, cy+dy)
                cr.line_to(cx+dx+(g+size), cy+dy)
                cr.stroke()
            if shape in ("x", "x_dot"):
                cr.move_to(cx+dx-gap, cy+dy-gap)
                cr.line_to(cx+dx-(gap+size), cy+dy-(gap+size))
                cr.move_to(cx+dx+gap, cy+dy+gap)
                cr.line_to(cx+dx+(gap+size), cy+dy+(gap+size))
                cr.move_to(cx+dx-gap, cy+dy+gap)
                cr.line_to(cx+dx-(gap+size), cy+dy+(gap+size))
                cr.move_to(cx+dx+gap, cy+dy-gap)
                cr.line_to(cx+dx+(gap+size), cy+dy-(gap+size))
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
            self.connect("configure-event", self._on_configure)
            self._apply_size_and_position()
            self.show_all()

        def _apply_window_rules(self):
            if self.use_layershell:
                GtkLayerShell.init_for_window(self)
                GtkLayerShell.set_layer(self, GtkLayerShell.Layer.OVERLAY)
                try:
                    GtkLayerShell.set_keyboard_mode(self, GtkLayerShell.KeyboardMode.NONE)
                except AttributeError:
                    with contextlib.suppress(Exception):
                        GtkLayerShell.set_keyboard_interactivity(self, False)
                GtkLayerShell.set_monitor(self, self.monitor)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.TOP, True)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.LEFT, True)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.RIGHT, False)
                GtkLayerShell.set_anchor(self, GtkLayerShell.Edge.BOTTOM, False)
                GtkLayerShell.set_exclusive_zone(self, -1)
            else:
                # X11: UTILITY type floats on top without full WM management
                with contextlib.suppress(Exception):
                    self.set_type_hint(Gdk.WindowTypeHint.UTILITY)
                with contextlib.suppress(Exception):
                    self.set_keep_above(True)

            # pass all mouse events through the canvas to whatever is below.
            # Applied at realize AND after map -- some compositors reset the
            # input region on the first map, so a single realize-time call
            # isn't always enough (H item: click-through must survive map).
            self.connect("realize", self._on_realize_passthrough)
            self.connect("map-event", self._on_realize_passthrough)

        def _on_realize_passthrough(self, *_):
            try:
                self.input_shape_combine_region(cairo.Region())
                log.debug("Applied empty input region (click-through) for %s", self)
            except Exception:
                log.exception("Failed to apply empty input region -- clicks may be captured")
            return False

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

        def _recenter(self, w, h):
            try:
                geo = self.monitor.get_geometry()
                x_margin = int((geo.width - w) / 2)
                y_margin = int((geo.height - h) / 2)
                if self.use_layershell:
                    GtkLayerShell.set_margin(self, GtkLayerShell.Edge.LEFT, max(0, x_margin))
                    GtkLayerShell.set_margin(self, GtkLayerShell.Edge.TOP, max(0, y_margin))
                else:
                    self.move(geo.x + x_margin, geo.y + y_margin)
            except Exception:
                log.exception("Failed to recenter overlay window")

        def _apply_size_and_position(self):
            canvas = self._canvas_size_px()
            self.set_default_size(canvas, canvas)
            self.darea.set_size_request(canvas, canvas)
            try:
                if self.get_realized():
                    self.resize(canvas, canvas)
            except Exception:
                log.exception("resize() failed for overlay window")
            # Recenter immediately from the requested size; the
            # configure-event handler below will correct this again once
            # the compositor reports the *actual* allocation (H-12: don't
            # trust set_default_size as a guaranteed live resize).
            self._recenter(canvas, canvas)
            self.queue_draw()

        def _on_configure(self, widget, event):
            alloc = self.get_allocation()
            if alloc.width > 1 and alloc.height > 1:
                self._recenter(alloc.width, alloc.height)
            return False

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
            with contextlib.suppress(Exception):
                mons.append(display.get_monitor(i))
        return mons

    def _monitor_geo_key(m):
        # Stable left-to-right, top-to-bottom ordering for numbering monitors
        # in the UI and for disambiguating duplicate model names below.
        try:
            g = m.get_geometry()
            return (g.x, g.y)
        except Exception:
            return (0, 0)

    def _resolve_by_name(mons, needle, ordinal):
        # Resolve a saved (name, ordinal) pair back to a live monitor object.
        # Exact case-insensitive match first; falls back to substring match
        # for configs saved before this was tightened. When multiple
        # monitors share an identical name (e.g. two of the same model),
        # `ordinal` picks the Nth one in stable left-to-right order -- this
        # is what makes "monitor 3" keep meaning the same physical screen
        # even if the OS re-enumerates monitors after a hotplug/reconnect.
        needle = str(needle or "").strip()
        if not needle:
            return None
        low = needle.lower()
        exact = [m for m in mons if monitor_pretty_name(m).strip().lower() == low]
        candidates = exact if exact else [
            m for m in mons if low in monitor_pretty_name(m).lower()
        ]
        if not candidates:
            return None
        candidates.sort(key=_monitor_geo_key)
        idx = _clamp(int(ordinal or 0), 0, len(candidates) - 1)
        return candidates[idx]

    def get_monitor_choices_for_ui():
        # "active" follows the cursor -- useful when swapping between displays.
        # Specific-monitor picks are identified by NAME (+ ordinal for
        # duplicate model names), not raw enumeration index -- index order
        # can reshuffle on hotplug/wake, silently pointing "monitor 3" at a
        # different physical screen. Name (+ordinal) survives reconnects.
        choices = [
            ("all",                  "all",    None),
            ("primary",              "primary", None),
            ("active (cursor mon.)", "active",  None),
        ]
        mons = sorted(get_all_monitors(), key=_monitor_geo_key)
        seen = {}
        for pos, mon in enumerate(mons):
            name = monitor_pretty_name(mon)
            ordinal = seen.get(name, 0)
            seen[name] = ordinal + 1
            label = f"{pos + 1}: {name}"
            if ordinal > 0:
                label += f" (dup #{ordinal + 1})"
            choices.append((label, "name", (name, ordinal)))
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
            picked = _resolve_by_name(
                mons, cfg.get("monitor_name", ""), cfg.get("monitor_name_ordinal", 0)
            )
            if picked is not None:
                return [picked]
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
            with contextlib.suppress(Exception):
                w.destroy()
        windows = []
        for mon in get_monitors():
            windows.append(OverlayWindow(mon, is_wayland=is_wayland,
                                         use_layershell=use_layershell))

    rebuild_windows()

    # defined early so SettingsWindow and tray can reference it;
    # real implementation added via _quit_callbacks below
    _quit_callbacks = []

    def request_quit():
        for cb in list(_quit_callbacks):
            with contextlib.suppress(Exception):
                cb()

    # holds the restart hand-off guard fd between _restart() and the
    # cleanup block after Gtk.main() returns (item 18: pipe-EOF handoff)
    _restart_guard = {"fd": None}

    def _reload_cfg_and_sync():
        r = core.load_config(paths.config_file, log)
        cfg.clear()
        cfg.update(r.config)
        _cfg_fingerprint[0] = core.file_fingerprint(paths.config_file)
        apply_config(rebuild=True)
        settings_window._sync_ui_from_cfg()
        if r.recovered:
            log.warning("Reload recovered from invalid config: %s", r.error)

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
                "Saves config, then hands off to a fresh daemon once this one\n"
                "has fully exited (no other running instances are touched)."
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
            nb.append_page(_wrap_scrollable(tab_gen), Gtk.Label(label="General"))

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
            _cur_shape = cfg.get("shape", "dot")
            active_idx = SHAPES.index(_cur_shape) if _cur_shape in SHAPES else 0
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
            self._mon_choice_name_ordinal = int(cfg.get("monitor_name_ordinal", 0) or 0)

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
            nb.append_page(_wrap_scrollable(tab_app), Gtk.Label(label="Appearance"))

            self.scale_opacity = self._add_scale(tab_app, "Opacity", 0.05, 1.0,
                                                  cfg.get("opacity", 0.95), step=0.01)
            self.scale_size = self._add_scale(tab_app, "Size", 1, 200, cfg.get("size", 5))
            self.scale_thick = self._add_scale(
                tab_app, "Thickness", 1, 100, cfg.get("thickness", 2)
            )
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
            nb.append_page(_wrap_scrollable(tab_prev), Gtk.Label(label="Preview"))

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
            nb.append_page(_wrap_scrollable(tab_hot), Gtk.Label(label="Hotkeys & Autostart"))

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
            nb.append_page(_wrap_scrollable(tab_sys), Gtk.Label(label="System"))

            btn_cfg_folder = Gtk.Button(label="📁 Open Config Folder")
            btn_cfg_folder.set_tooltip_text(str(paths.config_dir))
            btn_cfg_folder.connect("clicked", lambda *_: self._open_folder(str(paths.config_dir)))
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

            btn_uninstall_app = Gtk.Button(label="🗑 Uninstall App")
            btn_uninstall_app.set_tooltip_text(
                "Removes the installed script, launcher, app-menu entry, and\n"
                "autostart entry. Your config file is kept."
            )
            btn_uninstall_app.connect("clicked", lambda *_: self._uninstall_app())
            tab_sys.pack_start(btn_uninstall_app, False, False, 0)

            sep = Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL)
            tab_sys.pack_start(sep, False, False, 0)

            info = Gtk.Label()
            info.set_xalign(0)
            info.set_line_wrap(True)
            info.set_markup(
                f"<b>Config:</b> <tt>{paths.config_file}</tt>\n"
                f"<b>Socket:</b> <tt>{paths.socket_file}</tt>\n"
                f"<b>Lock:</b> <tt>{paths.lock_file}</tt>\n"
                f"<b>Log:</b> <tt>{paths.log_file}</tt>\n"
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
                backend_info = f"{backend}{'+layershell' if use_layershell else ''}"
                self.statusbar.set_text(
                    f"Crosshair: {enabled}  |  {backend_info}  |  "
                    f"{len(mons)} monitor(s): {mon_info}"
                )
            except Exception:
                pass

        def _launcher_hint(self):
            if paths.bin_file.exists():
                return str(paths.bin_file)
            return f'python3 "{Path(__file__).resolve()}"'

        def _copy_text(self, text):
            try:
                cb = Gtk.Clipboard.get(Gdk.SELECTION_CLIPBOARD)
                cb.set_text(text, -1)
                cb.store()
            except Exception:
                pass

        def _open_folder(self, path):
            with contextlib.suppress(Exception):
                subprocess.Popen(["xdg-open", path])

        def _edit_config(self):
            _save_cfg()
            with contextlib.suppress(Exception):
                subprocess.Popen(["xdg-open", str(paths.config_file)])

        def _install_autostart(self):
            try:
                bin_path, autostart_path = install_autostart(paths, log)
            except core.CoreError as exc:
                self._info_dialog("Install failed", str(exc))
                return
            msg = (f"Installed:\n  {bin_path}\n  {autostart_path}\n\n"
                   "Autostart: XDG autostart entry enabled (no systemd unit used).")
            self._info_dialog("Install complete", msg)

        def _remove_autostart(self):
            removed = []
            try:
                if paths.autostart_entry.exists() or paths.autostart_entry.is_symlink():
                    paths.autostart_entry.unlink()
                    removed.append(str(paths.autostart_entry))
            except OSError:
                pass
            removed += _remove_legacy_systemd_unit()
            msg = "Removed:\n" + ("\n".join(removed) if removed else "(nothing found)")
            self._info_dialog("Autostart removed", msg)

        def _uninstall_app(self):
            dlg = Gtk.MessageDialog(
                parent=self, modal=True, message_type=Gtk.MessageType.QUESTION,
                buttons=Gtk.ButtonsType.YES_NO, text="Uninstall Crosshair?",
            )
            dlg.format_secondary_text(
                "This removes the installed script, launcher, app-menu entry, and "
                "autostart entry. Your configuration file is kept.\n"
                "The currently running overlay keeps running until you quit it."
            )
            resp = dlg.run()
            dlg.destroy()
            if resp == Gtk.ResponseType.YES:
                removed = uninstall_app(paths)
                msg = "Removed:\n" + ("\n".join(removed) if removed else "(nothing found)")
                self._info_dialog("Uninstalled", msg)

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
                cfg.clear()
                cfg.update(core.default_config())
                _save_cfg()
                apply_config(rebuild=True)
                self._sync_ui_from_cfg()

        def _reload_config(self):
            _reload_cfg_and_sync()

        def _preview_bg_changed(self):
            c = self.btn_preview_bg.get_rgba()
            cfg["preview_bg"] = _rgba_to_hex((c.red, c.green, c.blue, 1.0))
            self.preview.queue_draw()
            if cfg.get("auto_save", True):
                _save_cfg()

        def _restart(self):
            """Save config; hand off to a fresh daemon once this one exits cleanly.

            Uses the pipe-EOF handoff in crosshair_core -- the replacement
            process is only spawned after this process has released the lock
            and socket, so it never races itself for ownership (H-11).
            """
            self._read_ui_to_cfg()
            _save_cfg()
            _restart_guard["fd"] = core.schedule_restart(
                Path(__file__).resolve(), ["--daemon", "--no-ui"], paths.log_file
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
            for lbl, mode, extra in choices:
                if self._mon_choice_mode != mode:
                    continue
                if mode == "name" and extra is not None:
                    name, ordinal = extra
                    if name == self._mon_choice_name and ordinal == self._mon_choice_name_ordinal:
                        active_label = lbl
                        break
                else:
                    active_label = lbl
                    break

            if active_label is None:
                # Nothing matched -- likely a pre-upgrade config that used a
                # raw index, or the previously-chosen screen is unplugged.
                # Fall back to "all" rather than guessing at a possibly
                # wrong physical monitor.
                self._mon_choice_mode = "all"
                self._mon_choice_name = ""
                self._mon_choice_name_ordinal = 0
                active_label = "all"

            self.mon_btn_label.set_text(active_label)

            def add_item(lbl, mode, extra):
                btn = Gtk.ModelButton(label=lbl)
                btn.set_hexpand(True)
                btn.set_halign(Gtk.Align.FILL)

                def _pick(*_):
                    self._mon_choice_mode = mode
                    if mode == "name" and extra is not None:
                        self._mon_choice_name, self._mon_choice_name_ordinal = extra
                    self.mon_btn_label.set_text(lbl)
                    try:
                        self.mon_pop.popdown()
                    except Exception:
                        with contextlib.suppress(Exception):
                            self.mon_pop.hide()
                    self._changed(rebuild=True)

                btn.connect("clicked", _pick)
                self.mon_pop_box.pack_start(btn, False, False, 0)

            for lbl, mode, extra in choices:
                add_item(lbl, mode, extra)

            self.mon_pop_box.show_all()

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
                self.btn_preview_bg.set_rgba(
                    self._gdk_rgba_from_hex(cfg.get("preview_bg", "#222222"))
                )
                self._mon_choice_mode = str(cfg.get("monitor_mode", "all"))
                self._mon_choice_index = int(cfg.get("monitor_index", 0) or 0)
                self._mon_choice_name = str(cfg.get("monitor_name", ""))
                self._mon_choice_name_ordinal = int(cfg.get("monitor_name_ordinal", 0) or 0)
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
            _sanitize_inplace(cfg)

        def _changed(self, rebuild=False):
            if self._suppress_changes:
                return
            if rebuild:
                self._pending_rebuild = True
            if self._debounce_id is not None:
                with contextlib.suppress(Exception):
                    GLib.source_remove(self._debounce_id)
                self._debounce_id = None

            def do_apply():
                self._debounce_id = None
                do_rebuild = self._pending_rebuild
                self._pending_rebuild = False
                was_auto_save = bool(cfg.get("auto_save", True))
                self._read_ui_to_cfg()
                apply_config(rebuild=do_rebuild)
                now_auto_save = bool(cfg.get("auto_save", True))
                _save_if_needed(was_auto_save, now_auto_save)
                self.preview.queue_draw()
                self._update_statusbar()
                return False

            self._debounce_id = GLib.timeout_add(60, do_apply)

        def _save(self):
            self._read_ui_to_cfg()
            _save_cfg()
            self._update_statusbar()
            self._info_dialog("Saved", f"Config saved to:\n{paths.config_file}")

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

    if result.recovered and open_config:
        notice = (
            "Your Crosshair config file was invalid and has been reset to defaults.\n"
            f"A backup of the broken file was saved to:\n{result.corrupt_copy}"
        )
        GLib.idle_add(lambda: (settings_window._info_dialog("Config recovered", notice), False)[-1])

    # apply config
    def apply_config(rebuild=False):
        _sanitize_inplace(cfg)
        if rebuild:
            rebuild_windows()
        for w in windows:
            w.apply_config()
        sync_fn = _tray_sync_ref["fn"]
        if sync_fn is not None:
            with contextlib.suppress(Exception):
                sync_fn()

    # tray -- one shared menu builder used by both AppIndicator3 and the
    # Gtk.StatusIcon fallback, rebuilt fresh from cfg on every state change
    # (see the sync_fn call in apply_config above) rather than mutated in
    # place. That keeps the "Screen" list and ON/OFF indicator honest no
    # matter whether the change came from the tray itself, the Settings
    # window, a CLI --set/--toggle call, or a monitor hotplug event.
    _tray_sync_ref = {"fn": None}

    def _current_monitor_selection_matches(mode, extra):
        if str(cfg.get("monitor_mode", "all")) != mode:
            return False
        if mode == "name" and extra is not None:
            name, ordinal = extra
            return (str(cfg.get("monitor_name", "")) == name and
                    int(cfg.get("monitor_name_ordinal", 0) or 0) == ordinal)
        return True

    def _apply_monitor_choice(mode, extra):
        cfg["monitor_mode"] = mode
        if mode == "name" and extra is not None:
            name, ordinal = extra
            cfg["monitor_name"] = name
            cfg["monitor_name_ordinal"] = ordinal
        apply_config(rebuild=True)
        if cfg.get("auto_save", True):
            _save_cfg()
        if settings_window.get_visible():
            settings_window._sync_ui_from_cfg()
        else:
            settings_window._update_statusbar()

    def _build_tray_menu():
        menu = Gtk.Menu()
        enabled = bool(cfg.get("enabled", True))

        status_item = Gtk.MenuItem(label=f"\u25cf Crosshair: {'ON' if enabled else 'OFF'}")
        status_item.set_sensitive(False)
        menu.append(status_item)
        menu.append(Gtk.SeparatorMenuItem())

        screen_item = Gtk.MenuItem(label="Screen")
        screen_menu = Gtk.Menu()
        radio_items = []
        for lbl, mode, extra in get_monitor_choices_for_ui():
            mi = Gtk.RadioMenuItem(label=lbl)
            if radio_items:
                mi.join_group(radio_items[0])
            radio_items.append(mi)
            # set_active BEFORE connecting -- avoids a spurious "toggled"
            # firing from this programmatic init and re-triggering a rebuild
            mi.set_active(_current_monitor_selection_matches(mode, extra))

            def _mk(mode=mode, extra=extra):
                def _cb(item):
                    if item.get_active():
                        _apply_monitor_choice(mode, extra)
                return _cb

            mi.connect("toggled", _mk())
            screen_menu.append(mi)
        screen_menu.show_all()
        screen_item.set_submenu(screen_menu)
        menu.append(screen_item)
        menu.append(Gtk.SeparatorMenuItem())

        def _open_settings():
            settings_window.show_all()
            settings_window.present()
            return False

        settings_item = Gtk.MenuItem(label=f"{APP_NAME} Settings")
        settings_item.connect("activate", lambda *_: GLib.idle_add(_open_settings))
        menu.append(settings_item)

        toggle_item = Gtk.CheckMenuItem(label="Enabled")
        toggle_item.set_active(enabled)

        def _toggle_cb(item):
            new_val = bool(item.get_active())
            if new_val == bool(cfg.get("enabled", True)):
                return
            cfg["enabled"] = new_val
            apply_config()
            if cfg.get("auto_save", True):
                _save_cfg()
            if settings_window.get_visible():
                settings_window._sync_ui_from_cfg()
            else:
                settings_window._update_statusbar()

        toggle_item.connect("toggled", _toggle_cb)
        menu.append(toggle_item)

        menu.append(Gtk.SeparatorMenuItem())
        restart_item = Gtk.MenuItem(label="\u21ba Restart")
        restart_item.connect("activate", lambda *_: settings_window._restart())
        menu.append(restart_item)
        quit_item = Gtk.MenuItem(label="\u2715 Quit")
        quit_item.connect("activate", lambda *_: request_quit())
        menu.append(quit_item)

        menu.show_all()
        return menu

    _tray_state = {"indicator": None, "status_icon": None}

    def _sync_tray():
        enabled = bool(cfg.get("enabled", True))
        state_text = "ON" if enabled else "OFF"
        indicator = _tray_state.get("indicator")
        if indicator is not None:
            with contextlib.suppress(Exception):
                indicator.set_menu(_build_tray_menu())
            # set_label puts short text next to the tray icon on DEs that
            # support it (Unity/some Plasma configs) -- an at-a-glance
            # ON/OFF without opening the menu at all. Harmless no-op
            # elsewhere. set_title shows up in some Plasma hover tooltips.
            with contextlib.suppress(Exception):
                indicator.set_label(state_text, "OFF")
            with contextlib.suppress(Exception):
                indicator.set_title(f"{APP_NAME} \u2014 {state_text}")
        icon = _tray_state.get("status_icon")
        if icon is not None:
            with contextlib.suppress(Exception):
                icon.set_tooltip_text(f"{APP_NAME} \u2014 {state_text}")

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
            _tray_state["indicator"] = indicator
            _tray_sync_ref["fn"] = _sync_tray
            _sync_tray()
            return indicator

        else:
            # Gtk.StatusIcon fallback (deprecated but available everywhere)
            try:
                icon = Gtk.StatusIcon()
                icon.set_from_icon_name("input-mouse")

                def _open_settings():
                    settings_window.show_all()
                    settings_window.present()
                    return False

                icon.connect("activate", lambda *_: GLib.idle_add(_open_settings))

                def _tray_popup(icon, btn, t):
                    _build_tray_menu().popup(None, None, None, None, btn, t)

                icon.connect("popup-menu", _tray_popup)
                icon.set_visible(True)
                _tray_state["status_icon"] = icon
                _tray_sync_ref["fn"] = _sync_tray
                _sync_tray()
                return icon
            except Exception:
                return None

    tray = _setup_tray()  # noqa: F841 -- keep a reference so GC doesn't drop the indicator

    # ipc command handling
    should_quit = {"value": False}

    def _do_quit():
        should_quit["value"] = True
        Gtk.main_quit()

    _quit_callbacks.append(_do_quit)

    def handle_ipc(req):
        cmd = str(req.get("cmd", "")).lower()
        payload = req.get("payload", None)

        def _gl(fn, *args):
            GLib.idle_add(fn, *args, priority=GLib.PRIORITY_DEFAULT)

        if cmd == "status":
            return {
                "ok": True,
                "version": APP_VERSION,
                "pid": os.getpid(),
                "enabled": bool(cfg.get("enabled", True)),
                "shape": cfg.get("shape", "dot"),
                "color": cfg.get("color", "#a000ff"),
                "monitor_mode": cfg.get("monitor_mode", "all"),
                "backend": backend,
                "layershell": use_layershell,
                "monitors": len(get_all_monitors()),
            }
        if cmd == "toggle":
            def _t():
                cfg["enabled"] = not bool(cfg.get("enabled", True))
                apply_config()
                if cfg.get("auto_save", True):
                    _save_cfg()
                if settings_window.get_visible():
                    settings_window._sync_ui_from_cfg()
                else:
                    settings_window._update_statusbar()
            _gl(_t)
            return {"ok": True}
        if cmd == "show":
            def _s():
                cfg["enabled"] = True
                apply_config()
                if cfg.get("auto_save", True):
                    _save_cfg()
            _gl(_s)
            return {"ok": True}
        if cmd == "hide":
            def _h():
                cfg["enabled"] = False
                apply_config()
                if cfg.get("auto_save", True):
                    _save_cfg()
            _gl(_h)
            return {"ok": True}
        if cmd == "config":
            def _c():
                settings_window.show_all()
                settings_window.present()
            _gl(_c)
            return {"ok": True}
        if cmd == "quit":
            _gl(request_quit)
            return {"ok": True}
        if cmd == "reload":
            _gl(_reload_cfg_and_sync)
            return {"ok": True}
        if cmd == "restart":
            _gl(settings_window._restart)
            return {"ok": True}
        if cmd == "set":
            try:
                patched = core.apply_patch(cfg, payload)
            except core.ConfigError as exc:
                return {"ok": False, "message": str(exc)}
            def _set():
                was_auto_save = bool(cfg.get("auto_save", True))
                cfg.clear()
                cfg.update(patched)
                apply_config(rebuild=True)
                now_auto_save = bool(cfg.get("auto_save", True))
                _save_if_needed(was_auto_save, now_auto_save)
            _gl(_set)
            return {"ok": True}
        return {"ok": False, "message": f"unknown command: {cmd}"}

    # server was already started (lock + socket acquired) earlier, before any
    # window/tray construction -- now that handle_ipc exists, wire it in.
    _ipc_handler_ref["fn"] = handle_ipc

    # config file watcher -- fingerprint-based so our own saves never
    # self-trigger a reload (H-02 / item 9)
    def _watch_config_file():
        try:
            fp = core.file_fingerprint(paths.config_file)
            if fp != _cfg_fingerprint[0]:
                _cfg_fingerprint[0] = fp
                _reload_cfg_and_sync()
        except Exception:
            log.exception("Config watcher failed")
        return True

    GLib.timeout_add(2000, _watch_config_file)

    # monitor change detection (signal + polling fallback). Fingerprints
    # geometry/scale/primary/connector, not just count (item 12), and polls
    # more usefully for "active" mode so it actually follows the cursor
    # output (H-04) without needless full rebuilds.
    def _monitor_fingerprint():
        mons = get_all_monitors()
        try:
            primary = display.get_primary_monitor()
        except Exception:
            primary = None
        parts = []
        for m in mons:
            try:
                g = m.get_geometry()
                parts.append((
                    monitor_pretty_name(m), g.x, g.y, g.width, g.height,
                    round(m.get_scale_factor() or 1), m is primary,
                ))
            except Exception:
                parts.append((id(m),))
        return tuple(parts)

    _last_mon_fp = [_monitor_fingerprint()]
    _last_active_mon = [None]

    def on_mon_change(*_):
        apply_config(rebuild=True)
        try:
            settings_window.refresh_monitor_options()
        except Exception:
            log.exception("Failed to refresh monitor options")

    try:
        display.connect("monitor-added", on_mon_change)
        display.connect("monitor-removed", on_mon_change)
    except Exception:
        pass

    def _poll_monitors():
        fp = _monitor_fingerprint()
        if fp != _last_mon_fp[0]:
            _last_mon_fp[0] = fp
            on_mon_change()
            return True
        if str(cfg.get("monitor_mode", "all")).strip().lower() == "active":
            mons = get_monitors()
            current = id(mons[0]) if mons else None
            if current != _last_active_mon[0]:
                _last_active_mon[0] = current
                apply_config(rebuild=True)
        return True

    GLib.timeout_add(400, _poll_monitors)

    # graceful shutdown on SIGINT/SIGTERM (item 17). A second signal falls
    # through to the default Python/OS handler as a force-quit escape hatch.
    def _on_unix_signal(sig_num):
        log.info("Received signal %s; shutting down", sig_num)
        request_quit()
        return GLib.SOURCE_REMOVE

    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGINT, _on_unix_signal, signal.SIGINT)
    GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, signal.SIGTERM, _on_unix_signal, signal.SIGTERM)

    if open_config:
        GLib.idle_add(lambda: (settings_window.show_all(), settings_window.present(), False)[-1])

    try:
        Gtk.main()
    finally:
        server.stop()
        core.release_restart_guard(_restart_guard["fd"])
        log.info("Stopped %s (pid %s)", APP_NAME, os.getpid())

    return 0

# --- CLI --------------------------------------------------------------

def main():
    parser = argparse.ArgumentParser(prog=APP_ID, add_help=True,
                                     description=f"{APP_NAME} v{APP_VERSION} -- crosshair overlay")
    parser.add_argument("--daemon", action="store_true", help="Run overlay daemon")
    parser.add_argument("--config", action="store_true", help="Open config UI (talks to daemon)")
    parser.add_argument("--toggle", action="store_true", help="Toggle enabled")
    parser.add_argument("--show", action="store_true", help="Enable/show")
    parser.add_argument("--hide", action="store_true", help="Disable/hide")
    parser.add_argument("--quit", action="store_true", help="Quit daemon (idempotent)")
    parser.add_argument("--restart", action="store_true", help="Restart daemon")
    parser.add_argument("--reload", action="store_true", help="Reload config from disk")
    parser.add_argument("--status", action="store_true", help="Print live daemon status")
    parser.add_argument("--set", type=str, help="Set config keys via JSON")
    parser.add_argument("--install", action="store_true",
                        help="Install into ~/.local/bin + app menu")
    parser.add_argument("--autostart", action="store_true",
                        help="Install + enable XDG autostart")
    parser.add_argument("--desktop-shortcut", action="store_true",
                        help="Create a double-clickable .desktop icon on the Desktop")
    parser.add_argument("--no-ui", action="store_true",
                        help="Daemon: do not open config UI on start")
    parser.add_argument("--doctor", action="store_true",
                        help="Print dependency/backend diagnostics")
    parser.add_argument("--uninstall", action="store_true",
                        help="Remove installed app/autostart (keeps config)")
    parser.add_argument("--purge-config", action="store_true",
                        help="Explicitly remove retained user config")
    parser.add_argument("--verbose", action="store_true", help="Verbose daemon logging")
    args = parser.parse_args()

    paths = core.AppPaths.from_env()

    if args.doctor:
        return doctor(paths)

    if args.status:
        try:
            response = core.ipc_call(paths, {"cmd": "status"})
            print(f"daemon:  running (pid {response.get('pid', '?')})")
            print(f"config:  {paths.config_file}")
            print(f"enabled: {response.get('enabled')}")
            print(f"shape:   {response.get('shape')}")
            print(f"color:   {response.get('color')}")
            print(f"monitor: {response.get('monitor_mode')}")
            print(f"backend: {response.get('backend')} (layershell={response.get('layershell')})")
        except core.IpcUnavailable:
            print("daemon:  not running")
            result = core.load_config(paths.config_file)
            print(f"config (on disk): {paths.config_file}")
            print(f"enabled: {result.config.get('enabled', True)}")
            print(f"shape:   {result.config.get('shape', 'dot')}")
            print(f"color:   {result.config.get('color', '#a000ff')}")
            print(f"monitor: {result.config.get('monitor_mode', 'all')}")
            if result.recovered:
                print(f"NOTE: on-disk config was invalid; defaults shown ({result.error})")
        return 0

    if args.install:
        try:
            py_path, bin_path = install_self(paths)
        except core.CoreError as exc:
            print(f"Install failed: {exc}", file=sys.stderr)
            return 1
        print(f"Script:  {py_path}")
        print(f"Command: {bin_path}")
        print(f"Menu:    {paths.application_entry}")
        return 0

    if args.autostart:
        try:
            bin_path, autostart_path = install_autostart(paths)
        except core.CoreError as exc:
            print(f"Autostart install failed: {exc}", file=sys.stderr)
            return 1
        print(f"Command:   {bin_path}")
        print(f"Autostart: {autostart_path}")
        return 0

    if args.desktop_shortcut:
        try:
            path = install_desktop_shortcut(paths)
        except core.CoreError as exc:
            print(f"Desktop shortcut failed: {exc}", file=sys.stderr)
            return 1
        print(f"Desktop shortcut created: {path}")
        print("Double-click it in your file manager to launch.")
        print("Do NOT run it from terminal -- it is a launcher, not a shell script.")
        return 0

    did_something = False
    if args.uninstall:
        removed = uninstall_app(paths)
        print("Removed:" if removed else "Nothing installed to remove.")
        for r in removed:
            print(f"  {r}")
        did_something = True
    if args.purge_config:
        removed = purge_config(paths)
        print("Config purged:" if removed else "No config to purge.")
        for r in removed:
            print(f"  {r}")
        did_something = True
    if did_something:
        return 0

    if args.daemon:
        return run_daemon(paths, open_config=(not args.no_ui), verbose=args.verbose)

    if args.restart:
        try:
            core.ipc_call(paths, {"cmd": "restart"})
        except core.IpcUnavailable:
            start_daemon_background(paths)
        return 0

    explicit = (args.toggle or args.show or args.hide or args.config or
                args.quit or args.reload or (args.set is not None))

    if explicit:
        cmd = None
        payload = None
        if args.toggle:
            cmd = "toggle"
        elif args.show:
            cmd = "show"
        elif args.hide:
            cmd = "hide"
        elif args.config:
            cmd = "config"
        elif args.quit:
            cmd = "quit"
        elif args.reload:
            cmd = "reload"
        elif args.set is not None:
            cmd = "set"
            try:
                payload = json.loads(args.set)
                if not isinstance(payload, dict):
                    raise ValueError("JSON must be an object")
            except (json.JSONDecodeError, ValueError) as exc:
                print('--set expects a JSON object, e.g. --set \'{"enabled": false}\'',
                      file=sys.stderr)
                print(str(exc), file=sys.stderr)
                return 2

        request = {"cmd": cmd}
        if payload is not None:
            request["payload"] = payload

        response = None
        try:
            response = core.ipc_call(paths, request)
        except core.IpcUnavailable:
            if cmd == "quit":
                print("Daemon not running.")
                return 0
            if not start_daemon_background(paths):
                print("Could not start daemon.", file=sys.stderr)
                return 2
            for _ in range(80):
                time.sleep(0.05)
                try:
                    response = core.ipc_call(paths, request)
                    break
                except core.IpcUnavailable:
                    continue

        if response is None:
            print("Could not contact daemon.", file=sys.stderr)
            return 2
        if not response.get("ok", False):
            print(f"Command failed: {response.get('message', 'unknown error')}", file=sys.stderr)
            return 2
        return 0

    # no args: start daemon, or bring up config if already running
    try:
        response = core.ipc_call(paths, {"cmd": "config"})
        if response.get("ok", False):
            return 0
    except core.IpcUnavailable:
        pass
    return run_daemon(paths, open_config=True, verbose=args.verbose)


if __name__ == "__main__":
    raise SystemExit(main())
