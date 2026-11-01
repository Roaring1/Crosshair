# Changelog

## Unreleased — 31/10/2026

- **Hardened:** the daemon lock opens nonblocking with `O_NOFOLLOW` and accepts only regular files; FIFO and device-node paths are rejected before flock or file mutation. Lock descriptors are closed on every validation failure.
- **Fixed dependency advisory:** the hosted Python 3.10 dependency audit identified 17 advisories affecting transitive `filelock==4.1.0`; development requirements now constrain `filelock==4.2.1`, the fix version reported by the advisory database.
- **Hardened:** daemon shutdown removes only the exact socket it created, matching device, inode and socket type. A replacement file at the socket path is retained.
- **Added:** a FIFO lock-path regression test verifies this rejection does not block daemon startup.

- **Hardened:** config and IPC JSON parsing now reject duplicate object keys at every nesting level and reject non-standard `NaN`/`Infinity` constants. Deeply nested config input is recovered as invalid data rather than escaping as an uncaught recursion error; the exact original bytes are preserved within the bounded size limit.
- **Added:** regression tests cover duplicate config/IPC keys, nested duplicate keys, non-standard numeric constants, IPC encoder rejection, and deeply nested config recovery.

- **Hardened:** atomic file writes now pin the parent directory with `O_NOFOLLOW`, create unpredictable `O_EXCL` temporary files, and perform rename/cleanup relative to the pinned directory descriptor. Parent-directory replacement is detected and fails closed; non-regular destinations are refused.
- **Hardened:** log files are opened with `O_NOFOLLOW|O_NONBLOCK`, verified as regular files, and permissioned through their open descriptor. The IPC socket is created owner-only at bind time and startup refuses to delete a regular file or other non-socket entry at its pathname.
- **Hardened:** config reads are capped at 1 MiB. Oversized files remain untouched, are not copied into a second large backup, and Crosshair continues with in-memory defaults while reporting the issue.
- **Added:** parent-directory replacement, FIFO log, non-socket IPC entry and oversized-config tests exercise these additional denial-of-service and path-redirection boundaries.

- **Hardened:** system helper programs are resolved only from root-owned system directories and must not be writable by the current user (including through supplementary groups or ACLs). User-writable PATH lookalikes are never executed. Legacy systemd unit files are retained unless both trusted disable and daemon-reload operations succeed.
- **Hardened:** application directories are opened with `O_NOFOLLOW` and permissioned through verified directory descriptors. Log files now open with `O_NOFOLLOW` and receive mode 0600 through their open descriptor. The local IPC socket is created with mode 0600 via the bind-time umask, without a post-bind pathname chmod.
- **Added:** adversarial tests cover directory chmod and log-open symlink races, trusted system executable resolution, systemd cleanup failures, and hostile desktop command paths. A real `desktop-file-validate` run accepts the reserved-character Exec test case.

- **Hardened:** config files are opened with `O_NOFOLLOW`; symlink and non-regular destinations are refused at open time. Invalid non-object JSON is preserved as a corrupt backup rather than silently replaced with defaults.
- **Hardened:** atomic writes no longer chmod the destination after rename, closing a symlink-swap window in which a raced destination could redirect the permission change to another file.
- **Hardened:** installed POSIX launchers quote both the interpreter and script path with shell-safe argument quoting. Desktop `Exec` paths now escape quotes, backslashes, dollar signs, backticks and literal field-code percent signs, and reject control characters and unsupported executable paths.
- **Hardened:** local IPC fails closed when peer credentials cannot be verified.
- **Added:** exploit-oriented tests cover symlink reads (including dangling links), non-regular config files, atomic-write symlink swaps, desktop field-code escaping, shell injection through hostile install paths and missing peer credentials.
- **Added:** Bandit medium/high severity analysis and `pip-audit` dependency vulnerability scanning to CI, pinned development tool versions, immutable GitHub Actions references, and weekly Dependabot updates. The current pinned development dependencies audit with no known vulnerabilities.

- **Fixed:** legacy `purplecrosshair` config migration now creates or verifies
  the destination config directory with mode 0700 before writing the mode-0600
  file. A symlinked destination is refused and logged rather than followed.
- **Added:** adversarial IPC tests cover oversized frames, symlinked lock and
  socket paths, lock release after failed server startup, and peer-UID
  rejection. Migration tests verify private directory permissions and
  symlink refusal.

- **Fixed:** generated app-menu and desktop-shortcut entries no longer use
  `--daemon --no-ui`. Normal clicks now route through the default CLI
  activation path, which opens settings for an existing daemon or starts it
  with the settings window visible. XDG login autostart remains headless.
- **Fixed:** the first CLI command after a stopped daemon no longer mistakes
  the temporary "daemon is still starting up" IPC response for a failed
  command. It retries until the daemon finishes wiring its GTK dispatcher,
  which makes first-use `--toggle` reliable as well.
- **Fixed:** `--quit` can no longer silently report success while the
  daemon remains alive if the GTK main loop is wedged. The IPC server queues
  the quit callback through `GLib.idle_add`; a bounded (~3s) watchdog now
  self-terminates this process if the main loop never confirms shutdown.
  This never signals another process by PID or name, preserving the
  C-01/C-02 decoy-process safety. Normal quit remains unaffected (<1s).
- **Fixed:** status reads and GUI-mutating IPC commands now execute on
  the GTK thread and wait for a bounded completion acknowledgement. Commands
  that have not started before the deadline are cancelled instead of being
  left queued to execute unexpectedly after the UI recovers. A callback that
  has started but cannot finish in time is reported as incomplete.
- **Fixed:** restart prepares the pipe-EOF replacement handoff before asking
  GTK to save/quit. A watchdog releases the handoff if the GTK main loop is
  wedged, so a restart no longer silently stalls behind an unprocessed idle
  callback.
- **Fixed:** `--restart` now reports daemon rejection and cold-start failures
  with a nonzero exit status instead of always returning success.
- **Fixed:** same-sized monitors no longer share a cached display name.
  Monitor cache identity includes logical geometry while remaining stable
  across GDK wrapper recreation.
- **Fixed:** the settings dropdown now persists `monitor_name_ordinal`, so
  choosing the second display of a duplicate model survives save/restart.
- **Fixed:** monitor-added/removed signals and the 400ms polling fallback now
  coalesce the same topology change into one overlay rebuild. Active-monitor
  state is initialized from the first build, avoiding an unnecessary rebuild
  shortly after startup; stale monitor-name cache entries are pruned.
- **Fixed:** large diagonal `X` and `X-dot` crosshairs now allocate enough
  window space for their radial endpoints. The settings preview scales large
  designs to fit its preview area without changing the actual overlay size.
- **Fixed:** the installer now deploys `crosshair_core.py` beside
  `crosshair.py`. Previously it updated only the frontend, leaving an
  older installed core module behind; the new frontend then crashed at
  startup when it imported helpers missing from that stale module. Core is
  installed before the frontend, and both modules are removed on uninstall.
- **Fixed:** uninstall now removes the generated Desktop shortcut. Purging
  configuration is recursive, refuses a symlinked config root, and never
  follows symlinks inside the config tree.
- **Fixed:** installer source symlink detection now checks the original path
  before resolving it. Relative `xdg-user-dir DESKTOP` output is rejected in
  favor of the home Desktop fallback; dangling legacy systemd unit symlinks
  are cleaned up.
- **Fixed:** `--doctor` determines the actual GDK display backend before
  consulting environment hints, so GTK Layer Shell is mandatory on native
  Wayland even when `XDG_SESSION_TYPE` is unset. `--status` and default
  settings activation now report an unresponsive daemon accurately.
- **Fixed:** CLI output piped to an early-closing consumer no longer emits an
  uncaught `BrokenPipeError` traceback.
- **Added:** GitHub Actions checks on Python 3.10–3.14, running Ruff,
  byte-compilation and the full headless regression suite.

## v3.2.0 — Nobara/KDE hardening pass

Rewrite of the lifecycle, configuration, IPC, and installer layers on top
of the original v3.1 drawing/UI code. Findings referenced below (C-xx,
H-xx) are from the static audit that preceded this pass.

### Critical

- **Removed all `pgrep`/PID-file-based process termination (C-01, C-02).**
  There is no code path left that signals a process based on its name or
  a PID read from disk. Single-instance enforcement is now an advisory
  `flock` on `$XDG_RUNTIME_DIR/crosshair/daemon.lock`, owned only by the
  process that holds the file descriptor. Verified: a decoy process named
  `crosshair.py` survives both `--restart` and `--quit`.
- **Autostart is XDG-only (C-03).** `--autostart` no longer creates a
  systemd user service; it writes one `.desktop` file to
  `$XDG_CONFIG_HOME/autostart/`. If a systemd unit from a pre-3.2 install
  is found, it's disabled and removed as an explicit, logged migration
  step during `--autostart` or `--uninstall`.
- **`--install`/`--autostart` report real success or failure (C-04).**
  Desktop entries are validated with `desktop-file-validate` when
  available; a failed validation removes the just-written file and
  raises, instead of reporting success regardless of exit code.
- **Config writes are atomic (C-05).** Every write goes through a sibling
  temp file, `fsync`, `os.replace`, and a directory `fsync` where
  supported. Symlinked destinations are refused. An invalid file is
  copied to a timestamped `.corrupt-*` backup before defaults are used —
  it is never silently overwritten.
- **Real backend detection (C-06).** The GDK backend is read from the
  live `Gdk.Display`'s GObject type name, not `$XDG_SESSION_TYPE` (which
  can disagree under XWayland).
- **No fake-Wayland fallback (C-07).** On native Wayland without GTK
  Layer Shell, the daemon now exits with a clear error (exit 2) instead
  of drawing an ordinary window that can't guarantee placement/stacking.

### High

- **H-02 / H-03 (watcher self-trigger, auto-save-off not persisted).**
  The config-file watcher compares an inode/size/mtime fingerprint, and
  every internal save updates that fingerprint — the daemon never
  reloads its own writes. Turning auto-save off is itself force-saved,
  through *both* the settings-window checkbox and the IPC `set` command
  (the latter was a gap found during testing of this same fix — see
  below).
- **H-04 (active-monitor mode only sampled once).** Monitor state is
  polled every 400 ms; in `active` mode this actually follows the
  monitor under the cursor instead of freezing at startup.
- **H-05 / H-06 (IPC desync, size, transport-vs-command success).** The
  IPC protocol is a bounded (64 KiB), newline-delimited JSON protocol
  with a mandatory boolean `ok` on every response, so callers can tell a
  transport failure from an application-level rejection.
- **H-07 (status trusted a PID file).** `--status` makes a live IPC call
  and reports "not running" (falling back to on-disk config) when the
  socket is unreachable — it never trusts a PID file.
- **H-08 (SIGTERM skipped cleanup).** `SIGINT`/`SIGTERM` are handled via
  `GLib.unix_signal_add` and run the same clean-shutdown path as `--quit`
  when the GTK main loop can dispatch them (stop IPC, release lock, exit).
  A wedged main loop can starve these signal sources too; the CLI `--quit`
  watchdog is the preferred recovery path in that situation.
- **H-09 (tray backend order).** Ayatana `AppIndicator3` is tried first,
  then legacy `AppIndicator3`, then `Gtk.StatusIcon`. Absence of any of
  them is a warning, never a startup failure.
- **H-10 (tall tabs don't scroll).** Every settings tab is wrapped in a
  `Gtk.ScrolledWindow`.
- **H-11 (racy restart).** `--restart` uses a pipe-EOF handoff: a helper
  process waits for the outgoing daemon to close a pipe (which happens
  only after the lock and socket are released) before spawning the
  replacement. No arbitrary sleep-and-hope delay.
- **H-12 (size/position drift).** Resizing now calls `resize()` in
  addition to `set_default_size()`, and a `configure-event` handler
  recenters from the *actual* allocation once the compositor reports it.
- **H-13 (runtime files in cache instead of `$XDG_RUNTIME_DIR`).** Socket
  and lock live under `$XDG_RUNTIME_DIR/crosshair`; a private fallback is
  used only if the runtime dir is unset/invalid, and `--doctor` warns
  about it.
- **H-14 (`--doctor` always exits 0).** Now returns non-zero if a
  mandatory dependency is missing.
- **H-16 (IPC `set` accepted anything).** Patches are validated strictly:
  unknown keys, wrong types, and out-of-range values are rejected with a
  specific message, not silently clamped.

### Medium (selected)

- Desktop-folder path uses `xdg-user-dir DESKTOP` when available instead
  of assuming `~/Desktop`.
- `--install` now also writes an application-menu entry, matching what
  the old README claimed but the old code didn't actually do.
- Click-through (`input_shape_combine_region`) is (re)applied on both
  `realize` and `map-event`, since some compositors reset the input
  region on first map.
- Legacy `purplecrosshair` config migration is now an explicit, logged
  step inside `--daemon` startup — never at import time, so `--help` and
  `--doctor` never touch the filesystem for it.
- `--uninstall` and `--purge-config` are separate, idempotent commands;
  config is only ever removed by an explicit `--purge-config`.

### Removed

- `--kill-all` / "Kill All & Restart Fresh" is gone. There is no safe way
  to keep that feature without a name-pattern kill, which is exactly what
  C-01 says not to do. The System tab now has an "Uninstall App" button
  (confirmation dialog, config kept) in its place.

### Behavior changes worth knowing about

- **`--set` is now strict**, not lenient. Previously out-of-range or
  wrong-typed values were clamped/coerced; now they're rejected with a
  non-zero exit and a message explaining why.
- **Native Wayland without GTK Layer Shell now refuses to start**
  (exit 2) instead of falling back to an unmanaged window.
- **A second concurrent launch fails fast** (exit 3, "already running")
  before doing any GTK/tray/D-Bus work, instead of building the full UI
  first and only then discovering another instance owns the lock.

### Fixed during integration testing (not in the original static audit)

- The IPC `set` handler had the same auto-save-off gap as the
  settings-window checkbox (H-03): setting `auto_save: false` via
  `--set` didn't itself get persisted, because the save was gated on the
  *new* (now-false) value. Both paths now share a `_save_if_needed()`
  helper that forces exactly one save on the on→off transition.
- Duplicate daemon launches originally acquired the lock *after*
  building all windows, the settings UI, and the tray — meaning a
  second launch did a full GTK/AppIndicator/D-Bus startup (which can
  block for seconds, or hang entirely without a session bus) before
  ever finding out another instance was running. The lock/socket are
  now acquired immediately after the mandatory display/backend checks,
  before any window is created.
