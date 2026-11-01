# Runbook: validating v3.2.0 on your machine

This lists what's already been verified for you, and what still needs
your actual Nobara/Plasma session, GPU, and games to check.

## Already verified (sandboxed headless X11 + isolated XDG dirs)

Everything below was actually *run*, not just read — a real GTK3 app
against a virtual X11 display (Xvfb), with `HOME`/`XDG_*` pointed at a
throwaway directory so it never touched a real config.

- `crosshair_core.py`: full pytest suite (18 tests) — atomic writes,
  file permissions, symlink refusal, corrupt-config preservation,
  single-lock-owner under real multiprocess contention, IPC round-trip,
  malformed-input resilience, oversized-message rejection.
- `ruff check .` — zero findings on both files.
- `--help`, `--doctor` (exit 0 with all deps present).
- Daemon start/stop, `--status` (live), `--toggle`.
- **10 concurrent duplicate `--daemon` launches**: exactly one daemon
  survives, the other 10 fail fast with "already running" (exit 3)
  *before* touching GTK/tray/D-Bus.
- **Decoy-process safety**: a process named `crosshair.py` (via
  `exec -a`) survives both `--restart` and `--quit` untouched — this is
  the core claim of the C-01/C-02 fixes, proven empirically, not just by
  reading the code.
- **Restart handoff**: old daemon exits, exactly one replacement comes
  up automatically via the pipe-EOF mechanism.
- **`--quit` idempotency**: second `--quit` after the daemon already
  exited reports "not running" cleanly, no error.
- **`--set` validation**: valid payload applies live; out-of-range,
  unknown-key, and malformed-JSON payloads are all rejected with a
  specific message and non-zero exit, and the daemon stays healthy
  throughout.
- **Corrupt-config recovery**: a truncated `config.json` is preserved as
  a timestamped `.corrupt-*` backup (mode 0600) and defaults are used;
  both `--doctor` and `--status` report this clearly.
- **Config watcher**: an external edit to `config.json` while the daemon
  is running is picked up within ~2 seconds and reflected in
  `--status`.
- **Auto-save-off transition**: confirmed it persists via both the
  settings-window checkbox and `--set`, and that further changes
  correctly stay in-memory-only afterward.
- **Install lifecycle**: `--install`, `--autostart` (XDG entry passes
  `desktop-file-validate`, confirmed **no** systemd unit is created),
  `--desktop-shortcut`, `--uninstall` (idempotent, config kept),
  `--purge-config`.

## Target-machine checks performed — 31/10/2026

On the user's Nobara/Plasma Wayland session:

- Confirmed the real backend is Wayland with GTK Layer Shell and three
  monitors; `--doctor` reports all required GTK/GDK/Cairo/layer-shell/tray
  dependencies healthy.
- Reproduced the old installed daemon at roughly 24–26% of one CPU core,
  with tens of thousands of Wayland socket round-trips/context switches per
  second. The installed copy was older than the repository. A fresh daemon
  from current source idled at 0%; the refreshed live daemon also sampled
  at 0% after restart.
- Verified the identical `--toggle` command against the live daemon flips
  OFF → ON → OFF. Also tested cold start with no daemon: first `--toggle`
  starts the daemon and flips OFF → ON; the second flips ON → OFF.
- Corrected the generated app-menu entry and the existing
  `~/Desktop/crosshair.desktop` shortcut to launch the interactive settings
  path (no `--daemon --no-ui`). Kept login autostart headless. All three
  desktop files pass `desktop-file-validate`, and KDE's service cache was
  rebuilt.
- The actual KDE click-to-open/settings-window presentation, pointer
  passthrough, multi-monitor visual placement, fullscreen/game compatibility,
  sleep/resume and long-duration stability were not visually exercised.

## Needs your real machine

These cannot be fully verified without the real compositor, GPU driver and
games — Xvfb has no layer-shell protocol, no KWin, and no fullscreen scanout
path. The launcher metadata is corrected, but the actual visible
click-to-open result still needs a quick desktop check.

### 1. Native Plasma Wayland

```bash
crosshair --doctor          # confirm backend=wayland, layershell=true
```

- [ ] Exact full-output center on every monitor
- [ ] Click/scroll/drag/game-mouse-look pass through
- [ ] No keyboard focus, no task-switcher/pager entry, doesn't move
      panels or the work area
- [ ] `active` monitor mode follows the cursor without oscillating
- [ ] Geometry/scale/rotation/primary/hotplug changes trigger one clean
      rebuild, not a flicker loop
- [ ] Maximizing/restoring the canvas doesn't move the visual center
- [ ] Windowed, borderless, fullscreen, Gamescope, HDR — record per game

```bash
G_MESSAGES_DEBUG=all PYTHONFAULTHANDLER=1 crosshair --daemon --no-ui \
    |& tee WAYLAND-GTK.log
```

### 2. X11 / XWayland

- [ ] RGBA transparency has no black box behind it
- [ ] Pointer input passes through
- [ ] Centering is correct with negative monitor coordinates
      (multi-monitor layouts with a monitor left/above the origin)
- [ ] `keep-above` holds for ordinary and borderless windows
- [ ] Document which games' exclusive-fullscreen modes bypass the
      compositor entirely (expected — no overlay is going to beat true
      exclusive fullscreen; that's a compositor limitation, not a bug)

### 3. Eight-hour stability

Run with normal toggling, a few config changes, a sleep/resume cycle,
and actual games. Sample RSS/CPU/fd count/log size periodically. Reject
unbounded growth, repeated self-reload loops, zombie processes, or a
flood of warnings in the log.

### 4. Fresh-machine install path

```bash
sudo dnf install python3-gobject python3-cairo gtk3 gtk-layer-shell \
    libayatana-appindicator-gtk3 xdg-utils desktop-file-utils
python3 crosshair.py --doctor       # should be all [OK] on Nobara/Fedora
python3 crosshair.py --install
python3 crosshair.py --autostart
# log out / log back in, or reboot -- confirm it actually autostarts
```

## Recovery, if something ever gets stuck

Never signal a process just because its name contains `crosshair`.

```bash
crosshair --quit                                  # try this first
lslocks | grep daemon.lock                         # who holds the lock?
ps -o pid,cmd -p <pid-from-above>                   # confirm it's really ours
cat /proc/<pid>/cmdline | tr '\0' ' '               # double-check
```

Only if you've confirmed via `/proc/<pid>/cmdline` that it's genuinely
the crosshair daemon should you signal it manually. `SIGTERM` is routed
through `GLib.unix_signal_add`, so it still depends on the GTK main loop
being able to dispatch the signal callback. After trying `--quit`, send
`SIGTERM` and wait a few seconds; if the exact same verified process is
still present and remains wedged, `SIGKILL` may be necessary. Re-check
`/proc/<pid>/cmdline` immediately before any force signal.

## Evidence to keep

```
ORIGINAL.SHA256           # hash of the imported v3.1 baseline
pytest -q  output
ruff check .  output
--doctor output on the target machine
Wayland/X11 logs from the checklist above
monitor layout, GPU/driver, and game compatibility notes
```
