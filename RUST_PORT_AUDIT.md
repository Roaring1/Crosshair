# Rust recreation audit

Audit baseline: a5ffb811a71d90889b15bf8e7537d937b5776d61
Frozen refs: working-python, working-python-2026-10-31
Implementation branch: rust-port

## Objective

Recreate Crosshair as a native Rust desktop utility with a strict on-demand lifecycle. The normal OFF/closed state must have zero Crosshair processes. No resident tray provider, polling service, login daemon, telemetry, updater, or background helper is part of the target design. Padfire or a desktop shortcut invokes a short-lived CLI/controller when needed.

The active overlay should be a tiny, static, transparent, topmost surface. It must not take keyboard focus or receive pointer input. Settings code and rendering preview code should be loaded only when the user explicitly opens Settings.

## Baseline implementation map

The current Python implementation is split into:

- crosshair.py (2,816 lines): CLI entry point, installer, GTK3 application, Cairo renderer, overlay windows, monitor discovery/selection, tray, Settings UI and GTK main loop.
- crosshair_core.py (1,084 lines): XDG path resolution, config schema and validation, safe file I/O, log handling, single-instance lock, Unix-domain JSON IPC, GTK callback handoff and restart helper.
- tests/test_cli.py (626 lines) and tests/test_core.py (727 lines): 99 regression tests.
- README, CHANGELOG, RUNBOOK and SECURITY document operation, security boundaries and target-machine checks.

Validation at the frozen head: Python 3.14.4; 99 tests pass; Ruff clean; Bandit medium/high scan clean; pip-audit reports no known vulnerabilities; hosted GitHub Actions has six successful jobs (Python 3.10–3.14 plus a separate dependency audit).

The machine was checked OFF before the baseline was frozen: crosshair --status reported no daemon and enabled=False.

## Existing behavior to preserve

### Overlay and renderer

- Nine persisted shape values: dot, cross, x, plus, ring, circle (normalized to ring), dot_ring, cross_dot, x_dot.
- Main color and opacity, size, thickness and gap.
- Optional outline: enabled, color, opacity and thickness.
- Optional shadow: enabled, color, opacity and X/Y offset.
- Transparent pixels everywhere outside the drawing; rounded line caps/joins.
- A small per-monitor drawing surface sized only to the shape's true bounds; no full-output backing bitmap.
- Settings preview scales an oversized drawing to fit while leaving the real overlay at native configured size.

### Placement and input behavior

- One overlay on all outputs, primary output, cursor-active output, or one named output.
- Specific-monitor identity is connector/model name plus an ordinal for duplicate display names; raw enumeration index is not trusted for this selection.
- Monitor add/remove, geometry, workarea, scale, refresh rate and identity changes cause a coalesced rebuild.
- Cursor-active output selection currently works on X11 and samples the pointer every 400 ms only in that optional mode. The current Wayland backend does not implement it: normal Wayland clients cannot query global pointer position. Wayland primary selection has no standard `wl_output` flag; the implementation uses the top-left-most output as a deterministic fallback. These are known backend limitations, not parity claims.
- Wayland uses the overlay layer, keyboard interactivity NONE, top/left anchoring and a negative exclusive zone. A compositor without layer-shell must fail clearly rather than claim an ordinary window is equivalent.
- X11 uses a transparent topmost utility-style window.
- Overlay has an empty input region; pointer/touch pass through. It never activates or gets keyboard focus.
- True exclusive fullscreen/direct-scanout behavior is compositor/game-dependent and cannot be promised.

### Lifecycle and CLI contract

- Default install is on-demand; login autostart is a separate, explicit opt-in.
- crosshair --toggle: cold start means show; a live daemon means toggle.
- --show, --hide, --status, --config, --set JSON, --reload, --restart, --quit, --doctor.
- Default no-argument app launch opens Settings.
- When overlay is OFF and Settings is closed, the daemon exits after a short grace period and removes its IPC socket/lock ownership. OFF is not represented by an invisible resident window.
- A visible Settings window is an intentional foreground workload.
- Restart is an orderly, one-instance handoff. The old process must release the lock/socket before the replacement takes ownership.
- IPC is bounded newline-delimited JSON (64 KiB), has explicit application-level success/error, and rejects unknown/out-of-range configuration patches.

### Settings UI: end-user inventory

The existing GTK Settings window is 540 x 640 with five tabs and a top action strip (Toggle, Show, Hide, Restart, Save, Quit), plus a bottom status line.

**General**
- Enabled checkbox.
- Auto-save on change checkbox; turning auto-save off is itself persisted.
- Shape selector.
- Monitor selector: all, primary, cursor-active, or a named output with duplicate-name disambiguation.
- Reset to Defaults confirmation.

**Appearance**
- Opacity slider (0.05–1.00, 0.01 steps).
- Size (1–200), thickness (1–100), gap (0–200).
- Main color.
- Outline enabled/color/opacity (0–1)/thickness (0–100).
- Shadow enabled/color/opacity (0–1)/X/Y offsets (-200–200).

**Preview**
- 240 x 240 live drawing preview.
- Custom preview-background color.

**Hotkeys & Autostart**
- Explains that Wayland blocks application-wide key grabs.
- Displays copyable Toggle/Show/Hide command strings for the desktop shortcut manager.
- Copy toggle command.
- Explicit install/remove autostart actions.
- Fullscreen/DRM direct-scanout troubleshooting tip.

**System**
- Open config folder.
- Open source/install location.
- Open config in text editor.
- Reload config from disk.
- Uninstall app while retaining config.
- Displays config/socket/lock/log paths and version.

The requested Rust Settings layout is intentionally smaller than the legacy five-tab GTK window:

- **General:** enable/disable, auto-save, monitor selection and Reset remain immediately visible. Shortcut instructions and optional startup controls are folded into a collapsed section. The System actions sit in a small top menu.
- **Appearance:** the main shape/color/opacity/size controls share a page with the 240 x 240 preview. Outline, shadow and the second reference dot can be expanded when needed. Global X/Y offsets and preview-background color are also available.
- **Profile sharing:** appearance settings export to a short, versioned, checksummed human-copyable code. The current code is exactly 52 characters (`CH1-` plus a 48-character URL-safe Base64 encoding of a fixed 36-byte binary payload with CRC-16). Monitor and machine-specific settings are intentionally excluded; a regression test locks the code length.
- No in-app title/header, footer, permanent status strip, dashboard cards, AI artwork, or resident tray process. General uses a compact window height; Appearance expands to fit its controls. The window keeps a short 140 ms height transition; managed WM_DELETE close stops the pending debounce timer and lets Slint unwind before hiding the native surface.
- Closing Settings with the window manager's X ends the Settings process without stopping an already-enabled overlay. The close path must flush the final debounced state when auto-save is enabled and restore the saved state when auto-save is off.

The Rust Settings UI is built in Slint (software renderer), not GTK, egui or a webview. It has General/Appearance navigation and a System popup. Optional Linux XDG login startup is a real explicit opt-in: Install calls the CLI's --autostart, Remove calls --remove-autostart, and ordinary --install always removes a stale login entry. The entry runs the hidden daemon with --daemon --no-ui; it does not change the Padfire-only default unless the user explicitly enables it. Linux app installation now reads source executables with final-component no-follow, nonblocking and bounded reads; preflights all three destinations before writing; writes CLI/Settings binaries with owner-only 0700 mode through pinned-directory atomic replacement; and writes the app-menu entry and autostart through the same core atomic writer. Replacement refuses symlinks, non-regular files, foreign-owned files and multiply-linked targets. Uninstall preflights the CLI, Settings, autostart and app-menu targets as a batch; a suspicious target aborts before any artifact is removed. The System-menu Uninstall item asks for confirmation, calls the sibling CLI's --uninstall, and retains config. Settings IPC and installer CLI actions run on one serial background worker; config updates use request generations to discard stale previews, and results return to the UI through Slint's event loop. Autostart enable/remove does not invalidate a pending preview or auto-save; uninstall alone invalidates it before replacing/removing the executable pair. Closing Settings joins the worker before the final persistence/overlay flush. These paths are tested with isolated XDG directories, special characters in install paths, symlink refusal, failed-install/no-partial-write and failed-uninstall/no-partial-removal assertions, config retention, Xvfb button interactions, and a delayed fake IPC peer that proves the Appearance tab resizes before a withheld reply arrives. Windows/macOS login startup and native installation still require platform-specific work.

## Reusable data versus OS-specific work

| Existing part | Windows reuse | Required Rust work |
| --- | --- | --- |
| JSON config schema and values | Data is reusable | Implement typed Rust schema; continue reading current Linux JSON unchanged |
| Shapes, geometry, outline/shadow math | Behavior is reusable | Port renderer and test output; Python source itself is not reused |
| CLI command names and semantics | Contract is reusable | Native Rust CLI/controller and Windows process launch |
| IPC command payload schema | Contract is reusable | Unix socket + peer UID on Linux; named pipe with current-user ACL on Windows |
| Single-instance/lifecycle semantics | Concept is reusable | Linux file lock; Windows named mutex/lock; no PID-name guessing |
| Config/log paths | Not portable verbatim | XDG on Linux; APPDATA/LOCALAPPDATA on Windows |
| GTK3 Settings widgets | No | Rebuild the controls in a portable Rust GUI |
| GDK monitor enumeration | No | Wayland/XRandR and Win32 monitor APIs |
| GTK Layer Shell overlay | No | Native Wayland layer-shell backend; separate X11 backend; Win32 layered window |
| Cairo drawing | Geometry and layer order are reusable | Small software rasterizer is compared against the Cairo reference using shape bounds and antialiasing-tolerant coverage metrics |
| .desktop / XDG autostart | No | Windows Start Menu/Startup registration and uninstall path |
| Tray/AppIndicator | Intentionally dropped | No Linux or Windows tray process in the target design |

A Rust source tree can share the config, renderer, UI view-model, CLI contract and much of the IPC protocol. It cannot copy a Linux Wayland surface or X11 window onto Windows. The Windows executable must be compiled for a Windows target and use Windows windowing, monitor, storage, startup and IPC primitives.

## Proposed Rust architecture

1. **Small controller/CLI**: short-lived command parser. When OFF, a Padfire toggle starts the daemon directly ON. The CLI exits immediately after the command is acknowledged.
2. **Small overlay daemon**: no GTK, egui, webview, tray or Settings widgets. Holds one small surface per selected output while visible. Blocks on Wayland/X11/Win32 events and IPC; no periodic redraw loop.
3. **Settings executable**: separate process launched only for --config or the app-menu entry. Uses Slint 1.18.1 with the software renderer and the agreed compact General/Appearance/System-popup layout. The same Rust config and rasterizer provide the live preview. When the overlay is ON, Settings sends config patches through IPC; when OFF, it edits/saves config without needing an overlay daemon.
4. **Shared core library**: typed config, validation/migration, shape rasterizer, paths, atomic persistence, command protocol, logging and platform-neutral tests.
5. **Platform adapters**:
   - Linux Wayland: Smithay client toolkit + wlr layer-shell when advertised, SHM buffer, keyboard NONE, empty input region.
   - Linux X11: X11 connection, ARGB visual, EWMH topmost/skip-taskbar hints and Shape input region.
   - Windows: Win32 layered transparent no-activate topmost window; EnumDisplayMonitors/DPI-aware geometry; named-pipe IPC and a per-user mutex.
6. **No resident tray**. No login autostart by default. No Crosshair process when OFF and Settings is closed.

The renderer should use a tiny software buffer around the crosshair, not a framebuffer-sized surface. The daemon should unmap/destroy the surface when disabled, use event-driven monitor changes, and avoid frame callbacks/animation entirely.

## Performance acceptance gates

- OFF + Settings closed: no Crosshair process, service, tray registration, open socket, timer or monitor polling.
- ON, fixed monitor: one daemon process; idle CPU effectively zero after first present; no fixed-interval timer; no redraw until a genuine config/output event; tiny per-monitor pixel buffer.
- ON, cursor-active monitor: only this optional mode uses a 400 ms pointer sample (or a platform event that provides equivalent reliable active-output tracking).
- OFF + Settings open: only the visible Settings process; no renderer/overlay daemon unless preview explicitly requires it.
- Settings closed after hiding: Settings exits; no Crosshair process remains.
- Report RSS and PSS/private memory separately on Linux; RSS alone is not a fair measure of shared runtime libraries. On Windows report private bytes/working set separately.
- Compare like-for-like on the same hardware, monitor count, compositor and display scale. Do not claim a memory target until measured.

## Portability and release targets

- Linux x86_64: native Wayland and X11; Wayland overlay requires a compositor implementing wlr-layer-shell.
- Windows x86_64: Win32 layered overlay and portable Settings UI.
- Linux ARM64 / Windows ARM64 can be evaluated after x86_64 feature parity.

The Windows and Linux builds are separate artifacts. Rust target triples support both Linux GNU and Windows MSVC, but they are not one universal executable. CI must compile/test each target, and Windows overlay behavior must be exercised on a real Windows session rather than inferred from Linux tests.

## Porting exit criteria

- [ ] Rust config reads current Python-generated JSON without destructive migration; unknown and invalid fields are handled deterministically.
- [x] Shared Python/Rust renderer parity: all nine persisted shape values (including the `circle` alias), main/outline/shadow layers, colors and opacity passed 20 representative Cairo comparisons with exact alpha bounds, at most 7.6% alpha-mass difference and at most 3.77/255 mean premultiplied-channel error. Rust-only reference-dot color, opacity and offset have separate unit coverage. The parity matrix is representative, not exhaustive.
- [ ] Multi-monitor targeting and hotplug behavior verified on real OS APIs.
- [ ] Input pass-through, no-focus and topmost behavior verified per backend.
- [ ] All CLI operations and cold/live toggle semantics retained.
- [ ] One-instance locking and bounded IPC tested under concurrent starts and malformed input.
- [ ] Atomic config writes, symlink/path defenses and orderly restart/shutdown have regression coverage.
- [ ] Settings exposes every user-relevant control and does not keep the daemon alive when OFF.
- [ ] Linux Wayland, Linux X11 and Windows x86_64 have real integration checks.
- [ ] Idle RSS/PSS/private bytes, CPU, file descriptors/handles and cold-start latency measured on the poor-device test target.
- [x] Python working branch/tag remains available; the Rust work is isolated on `rust-port` and the default branch has not been replaced.

## Verified implementation snapshot (2026-11-01)

This section records what was actually built and exercised on the Nobara x86_64 development machine. It is not a release-readiness declaration.

### Current verification

- `cargo check --features settings-ui --bins`: passed.
- `cargo test --locked --features settings-ui --all-targets`: **51 passed** (27 shared-core/renderer/filesystem tests, four shared IPC-framing tests, five Wayland output-layout tests, six Wayland output-selection tests, five shared Linux lock/socket security tests, two Wayland shared-memory canvas tests and two Settings-worker/preview regressions).
- Debug and optimized Linux builds of both executables: passed.
- HiDPI renderer regression: passed. Logical config values are normalized first, then scaled for physical pixels without being normalized back to the lower UI limits. Scale is bounded to 1..=4; tests cover all scaled geometry fields, maximum logical reticle size at scale 2, zero/oversized scale handling, and Wayland output-layout size selection/fallback/invalid-dimension cases. If an extreme output scale exceeds 4x and logical dimensions are unavailable, the layout now fails closed rather than mixing an unbounded mode divisor with a capped buffer scale.
- Wayland monitor-selection unit coverage: all/index/primary fallback/named connector plus duplicate-name ordinal/missing named output/unsupported cursor-active behavior passed using synthetic output metadata. Primary is currently a documented top-left-most fallback, not a compositor-reported primary designation.
- `cargo build --locked --example render_probe` and `python3 tests/render-parity.py`: passed; 20 representative Python/Cairo-vs-Rust cases with exact alpha bounds, maximum 7.6% alpha-mass difference and maximum 3.77/255 mean premultiplied-channel error.
- `cargo check --locked --target x86_64-pc-windows-msvc --features settings-ui --bins`: passed from the Linux host using the isolated Rust 1.96.0 MSVC standard-library target. This proves compile compatibility only; it does not test the Win32 overlay, window behavior or named-pipe security.
- `git diff --check`: passed.
- Python fallback: **99 tests passed**, Ruff clean, the repository's Bandit medium/high gate passed, and `pip-audit` found no known vulnerabilities.
- X11 runtime smoke test under an isolated Xvfb display: passed. It verified 32-bit ARGB, no keyboard focus, an empty input shape, pointer click-through, cold-toggle startup and daemon exit after hide.
- Settings-close smoke test under isolated Xvfb: passed. Closing the Settings window exited its process and left the enabled overlay daemon alive; hiding the overlay then exited that daemon.
- Appearance preview regression: passed. The preview remains rendered when the real overlay is disabled.
- Install/autostart/uninstall smoke test under isolated temporary XDG directories: passed against both debug and optimized release executable pairs. It verified quoting for paths containing spaces, percent signs and dollar signs; owner-only executable mode 0700; private app-menu/autostart file and directory modes 0600/0700; explicit autostart enable/remove; cleanup by ordinary on-demand installation; fail-closed install on symlinked autostart or app-menu destinations before replacing binaries; uninstall preflight abort without partial deletion; application-file/menu cleanup; and configuration retention.
- Settings autostart UI smoke test under Xvfb: passed. It opened Settings, expanded the General startup section, clicked Install and Remove, verified the XDG entry transitions, closed Settings while the overlay stayed enabled, then hid the overlay and verified daemon shutdown.
- Settings delayed-IPC/autostart/shutdown test under Xvfb: passed three consecutive times. A fake Unix socket accepts an overlay update and delays its reply for 1.5 seconds; Settings still processes the Appearance tab click and resizes to 420 px. The test queues Install autostart behind the in-flight preview, then immediately sends a direct X11 `WM_DELETE_WINDOW` message before either operation has completed. Worker shutdown drains the queued CLI action after the IPC task, the autostart entry exists after exit, Settings closes cleanly, and the final close flush persists `enabled=true`.
- Settings IPC drop/recovery smoke under Xvfb: passed three consecutive times. The fake peer accepts the first enabled-state update and closes without acknowledging it; the test confirms the failed update was not persisted. A later user update succeeds against the recovered peer, persists `enabled=true`, and Settings closes cleanly.
- Settings rapid-edit/socket-replacement smoke under Xvfb: passed three consecutive times. The fake old listener accepts the first preview and withholds its response while four more setting edits are made. The test atomically replaces the socket path with a ready listener, then verifies stale queued previews are skipped, only the final `enabled=true` update reaches the replacement listener and persists, and Settings closes cleanly. This is a controlled IPC handoff test, not a real compositor or overlay-process crash/restart test.
- Rust/Cairo renderer parity: passed 20 representative cases across all nine persisted shape values, including the `circle` alias, outline/shadow layering, opacity and edge geometry. Alpha bounds were exact in every case; the largest alpha-mass difference was 7.6% and largest mean premultiplied-channel error was 3.77/255. The harness uses the frozen Python drawing procedure and allows for Cairo-versus-2x2-sampler antialiasing differences. A thick zero-gap cross exposed repeated source-over blending at arm intersections; Rust now evaluates combined stroke-path coverage as a union before blending. Rust-only second-dot behavior is unit-tested separately.
- Wayland monitor registry query: connected through the read-only `--list-monitors` path on the active KDE Wayland session and enumerated three outputs. This did not create surfaces or launch the overlay; it verified registry enumeration only.
- Isolated Wayland layer-shell smoke: passed against a separate KWin virtual-framebuffer compositor inside a private DBus session with temporary HOME/XDG directories. It verified the `wlr-layer-shell` global, output discovery, initial surface configure/render, a live appearance IPC update, and graceful daemon shutdown. This is a real layer-shell client/compositor lifecycle, but KWin exposes only one integer-scale virtual output; it does not validate physical multi-monitor placement, fractional scaling, hotplug, fullscreen behavior or pointer passthrough on the active desktop.
- Shared IPC framing: both Linux servers and the CLI/Settings clients use one bounded newline-delimited reader. It reads at most 64 KiB plus one sentinel byte, rejects oversized messages, and requires the newline delimiter. Four regression tests cover a normal request, the exact 64 KiB boundary, an oversized request with an otherwise valid JSON prefix, and EOF without the delimiter. Request writers use the same shared limit and reserve space for the delimiter.
- Linux daemon/socket security: five additional tests verify exactly one winner across eight concurrent daemon-lock attempts, refusal of symlinked and hard-linked lock files, replacement of a stale owned Unix socket, private 0600 socket permissions and current-user peer credentials, refusal of symlink/regular-file socket paths without modifying their targets, and safe cleanup when a socket path is replaced by a regular file.
- The compact General, Appearance and System-popup layouts were captured and visually inspected under Xvfb. The separate Settings-close regression passed; a later multi-page screenshot harness did not close its window after opening the System popup, so popup-open close behavior is not counted as verified.
- The system toolchain did not ship `rustfmt`; to avoid changing the Nobara package set, Rust 1.96.0 and its matching formatter were installed under `/tmp`. `cargo fmt --all -- --check` passes locally. The CI job independently installs stable Rust plus rustfmt and runs the same formatter gate.

### Measured Linux overlay footprint

Optimized Linux x86_64 artifacts after the current build:

| Artifact | File size |
| --- | ---: |
| `crosshair` CLI + overlay | 1,186,048 bytes (about 1.13 MiB) |
| `crosshair-settings` | 15,229,952 bytes (about 14.52 MiB) |

The overlay binary's dynamic dependencies on this machine are limited to the C runtime and libgcc. The Settings binary also uses the installed font and text-rendering stack (fontconfig, FreeType, HarfBuzz, GLib and related libraries); the app is not yet a fully static, zero-system-dependency bundle.

An isolated Xvfb/X11 measurement of one uniquely identified optimized overlay process gave:

- RSS: 2,968 KiB
- PSS: 1,202 KiB
- PSS anonymous: 196 KiB
- CPU: 0.0% at initial sample and after two seconds idle
- After `--hide`: no overlay daemon remained

These figures are a controlled Xvfb/X11 baseline, not a measurement on the target Wayland compositor or the user's weakest hardware. They do not imply that the much larger Settings process is resident while the overlay is active.

### Remaining work / release blockers

1. **Windows is not implemented yet.** Windows IPC and overlay startup are explicit stubs. The new Windows CI job is compile-only; no Win32 layered window, monitor/DPI events, no-activate/input-transparent behavior, named pipe, ACL, mutex or Windows integration tests have been delivered. The non-Linux atomic-file-write code was corrected to be compilable, but its security behavior still needs native Windows testing.
2. **macOS is not implemented or a release target.** It would need another window/monitor/lifecycle adapter.
3. **Wayland monitor parity is incomplete.** The read-only output registry query succeeds on the active KDE session, and a real layer-shell lifecycle passes on an isolated KWin virtual compositor. Cursor-active selection is unavailable, and primary currently falls back to the top-left-most output (not a compositor-reported primary). All/index/named-connector selection and duplicate-name ordinals have headless tests. The output-layout math and HiDPI renderer preserve scaled logical values, but physical multi-monitor placement, fractional scale, output hotplug, pointer passthrough and fullscreen-game behavior still need validation on the active KDE compositor.
4. **Installer packaging and transaction rollback remain.** Linux optional XDG autostart and the confirmed Settings uninstall path are implemented. Linux CLI/Settings binaries and the app-menu entry now use no-follow, owner-checked atomic writes; uninstall validates all app-owned targets before deleting any. The multi-file install is not a single transaction, so an I/O failure after one atomic artifact replacement can still leave an incomplete upgrade; add staging/rollback before calling packaging complete. Icon metadata and clean-machine dependency checks also remain. Windows/macOS installers and login-startup equivalents are unimplemented. The Padfire-first default remains no autostart.
5. **Settings worker stress/lifecycle coverage is substantially exercised, but real daemon restart remains unverified.** Xvfb tests cover delayed IPC responsiveness, graceful close/worker join/final auto-save, queued CLI-action draining at shutdown, recovery after a dropped acknowledgement, and rapid edits across an atomic socket-listener replacement. The replacement-listener test does not exercise a live overlay process crash/restart or its compositor interactions.
6. **Persistence and IPC stress coverage is incomplete.** The 64 KiB framing boundary, oversized-request rejection and required newline delimiter have direct shared-reader tests. Five Linux lock/socket tests verify eight-way concurrent lock exclusivity, symlink/hard-link lock refusal, stale owned-socket replacement, private permissions/peer credentials, refusal of unsafe socket targets, and safe cleanup after socket-path substitution. Still add true multi-process cold-start tests, malformed JSON-body handling at the server boundary, command-timeout coverage, live daemon restart handoff, and close-during-debounce tests.
7. **Renderer parity coverage is now representative, not exhaustive.** The 20-case Cairo comparison covers shared shape values, main/outline/shadow passes, color and opacity, and catches zero-gap stroke-overlap errors. Add property-based sweeps over all normalized parameter boundaries and retain screenshot/image inspection against the frozen GTK implementation.
8. **Release packaging is still a prototype.** The Linux user installer now has isolated coverage for desktop-entry escaping, stale autostart cleanup, symlink refusal, all-target uninstall preflight and config retention. Still test staged upgrade/rollback from arbitrary install locations, icon metadata, dependency availability on clean Nobara/Fedora and Ubuntu systems, and decide on portable archive/AppImage/Flatpak/RPM plus a signed Windows installer separately.
9. **Live target-machine gates remain outstanding.** Run on the actual KDE Wayland session, multiple monitors, fractional scaling, game fullscreen modes, and a genuinely low-end CPU/GPU/memory device. Record cold startup, file descriptors, steady-state PSS/CPU and Settings memory separately.

### CI changes prepared on this branch

The workflow now runs the existing Python gates plus:
- Linux Rust formatting, tests, native debug/release builds, and debug/release installer lifecycle tests.
- Isolated Xvfb tests for X11 pass-through/cold-start/shutdown, Settings autostart controls, delayed-IPC responsiveness plus graceful close/save, and independent Settings/overlay lifetime.
- A Windows-latest compile check for the shared core, Settings UI and cfg-gated Windows code.

The Windows job is intentionally not described as Windows feature verification. CI results must be inspected after the branch is pushed.
