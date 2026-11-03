# Native Rust port status

_Last reviewed: 2026-11-03_


## Current deployment checkpoint — 2026-11-03 (supersedes older deployment notes below)

The exact hosted PR head `0c4b9ab40ad7f032030208d7f927bf16fc596414` has now been built and installed on NobaraPC. The installation and validation below are current; earlier statements in this file about leaving the installed pair untouched describe the pre-deployment checkpoint only.

- GitHub Actions run [#179](https://github.com/Roaring1/Crosshair/actions/runs/41700492720) completed fully green at this head: Linux Rust job, Windows native build/tests/full nine-stage installer-overlay-named-pipe-Settings-close-uninstall smoke, Python 3.10–3.14 matrix, and dependency vulnerability audit.
- On NobaraPC, rebuilt the clean detached worktree at the exact PR head and passed all 52 Rust tests (32 core/renderer/IPC, 18 Linux backend, 2 Settings worker), debug and optimized builds, and isolated install/autostart/uninstall smoke against both debug and release binaries. The source branch and uncommitted working tree at `/home/roaring/Documents/GitHub/Crosshair` were not reset, stashed, cleaned, or overwritten.
- Ran the private KWin Wayland test against this build: KWin advertised `zwlr_layer_shell_v1`; Crosshair discovered `Virtual-0`, created/configured/rendered the layer surface, applied a live appearance update, and exited cleanly. This remains an isolated one-output compositor test, not a validation of the active desktop's physical displays or games.
- Seven Settings screenshots were captured under private 1280x900 Xvfb, and Settings exited cleanly. The same seven-screen capture/close check passed again against the binaries after installation.
- Installed CLI SHA-256: `674e94fd7a0311207f969f4f81a875cdbc204a1da0ca0140f810ca50ccf79d1d`. Installed Settings SHA-256: `55ca70c95c0c53de6da2d7562739ad6fac9fb0d83db8a2a0b46bc408fc20d72a`. Both exactly match the optimized release artifacts from the PR-head worktree.
- User config SHA-256 before and after installation: `da886a6aa0c60f934514ef7437390c67de456e31bb29b53cf05a7b5d4296dcf5`; unchanged. The saved overlay setting remains disabled. The existing explicit XDG login-autostart preference was retained and now targets the installed Rust CLI. Running its real `--daemon --no-ui` command with the disabled config exits without leaving a daemon or socket.
- Both installed desktop entries pass `desktop-file-validate`. `--help`, `--version`, `--doctor`, `--list-monitors`, and `--status` work. Read-only discovery in the active KDE Wayland session sees `DP-1`, `DP-2`, and `HDMI-A-2`. No visible overlay or Settings window was opened on the active desktop; final status is daemon not running / overlay disabled.
- A rollback snapshot of the previously installed binaries and desktop/icon/autostart entries is retained at `~/.local/state/crosshair/installer-backups/pre-pr3-0c4b9ab-20261103T202903Z`.
- Keep PR #3 in draft. Remaining gates are the actual Windows laptop (mixed DPI, monitor changes/hotplug, fullscreen and pointer passthrough), a deliberate visual/pass-through/fullscreen test on the real KDE desktop, broader real multi-monitor and fractional-DPI checks, and a decision about tray parity. The private KWin test does not close those real-session gates.


## Update — 2026-11-03: Windows Settings shutdown, release smoke, and resource audit

- The Windows Settings process-close issue is now resolved in the current implementation and verified in hosted Windows CI. The app's close callback waits for the native `WM_CLOSE` dispatch to unwind, explicitly hides the Settings component on the next Slint event-loop turn, and then requests loop termination. Once `ui.run()` returns, the existing worker-drain and final-config flush complete normally. No forced process exit, helper thread, polling loop, or overlay-daemon shutdown was introduced.
- The initial Windows smoke test itself had a separate defect: it enumerated every HWND owned by the Settings PID and posted `WM_CLOSE` to Winit's hidden event-target HWND as well as the visible Settings window. That broke the event-loop plumbing and made the process appear hung. The harness now waits for the visible window titled `Crosshair Settings` and sends `WM_CLOSE` only to that HWND; it no longer falls back to `Process.MainWindowHandle`. The exact harness change is commit `384e64510614f2f366fe97bdeefb5ed1fecd10a7`.
- Hosted Actions run #177 on that head passed the Windows optimized build, 29 Windows-native Rust tests, 2 Settings-bin tests, and the full nine-stage Windows smoke: per-user installation, monitor discovery, opt-in Run-registry startup and removal, cold overlay launch, named-pipe config update, opening Settings through the installed CLI, closing Settings while the overlay remains active, hide/toggle-off, and uninstall. The smoke log confirms the visible Settings HWND received `WM_CLOSE` and the Settings process exited. The Windows x64 package was generated successfully.
- The Windows package is the unsigned CI ZIP `Crosshair-Windows-x64.zip` (5 files: `crosshair.exe`, `crosshair-settings.exe`, install/uninstall PowerShell scripts and README). SHA-256 of the extracted package ZIP: `b65ee8f95f8e67b7ba7c7953841b2105cfdcc928c3cc6306b17108b30fdddf5c`. SHA-256 of the enclosing GitHub Actions artifact download: `173e833eeb914541e8adef2583e0e74e0234bae6a0e98f5de5f3b8a40f63fdb2`. The README explicitly says to validate the package on the target Windows laptop before considering native Windows support fully verified.
- The Linux job in run #177 also passed all stages: 52 Rust tests; 20 shared Cairo/Rust rendering parity cases; debug and optimized install/autostart/uninstall lifecycle tests; X11 ARGB/no-focus/input-shape/click-through/cold-start checks; Settings autostart controls; delayed-IPC responsiveness and shutdown drain; IPC failure recovery; rapid edits during listener replacement; Settings close with the overlay kept alive; and screenshot capture. The 7 Settings screenshots were uploaded as the `crosshair-rust-settings-ui` artifact.
- Python compatibility jobs for 3.10, 3.11, 3.12, 3.13 and 3.14 all passed. The first Python 3.14 attempt hit a timing-sensitive `BrokenPipeError` in the existing peer-UID rejection test (the server rejects and closes before the client finishes `sendall`); the isolated retry of that exact job passed. The dependency vulnerability audit also passed on the retry, so run #177's full matrix is green.
- A controlled Xvfb sample of the installed 4.0.0 daemon measured 26 ms `--show` client latency, one daemon thread, 3,016 KiB RSS, six open descriptors and zero CPU ticks over five seconds at idle. `--hide` removed the daemon process and IPC socket. This is not a live-compositor performance benchmark and does not include PSS/private memory or GPU/display timing.
- The real KDE session was inspected read-only. Crosshair's Wayland discovery returns `DP-1`, `DP-2` and `HDMI-A-2`; KScreen reports all three enabled at scale 1. The saved monitor selection is `DP-2`, which is present. No overlay or Settings window was opened on the desktop, so compositor rendering, fullscreen-game behavior, display hotplug, focus/click-through and real-session shutdown still need a deliberate interactive check.
- At the run #177 checkpoint, the actual Nobara installation had not yet been updated: executable hashes and desktop entries were unchanged, the saved config was disabled, and the existing explicit XDG login-autostart preference was preserved. Its `--daemon --no-ui` behavior had been tested in a private XDG sandbox. This describes the pre-deployment checkpoint; see the current deployment checkpoint above for the subsequent controlled install and verification.
- Keep PR #3 in draft. Remaining release gates are a real Windows-laptop pass (including mixed-DPI, multi-monitor, hotplug and fullscreen-game behavior), a controlled visual/pass-through check on the active KDE Wayland desktop, and an explicit decision about whether the known tray parity gap is acceptable. The isolated KWin layer-shell smoke is useful platform evidence but does not replace those checks.



## Update — 2026-11-03: NobaraPC Rust deployment

The Rust Linux build is now installed as the active NobaraPC application:

- Built the Rust CLI and Slint Settings executable with `cargo build --locked --release --features settings-ui --bins`.
- Installed through the native Rust `--autostart` path into `~/.local/bin`. The installed `crosshair` and `crosshair-settings` hashes exactly match the freshly built `target/release` binaries.
- Preserved the existing explicit login-autostart choice. The XDG entry starts the installed Rust daemon with `--daemon --no-ui`; the current saved config remains disabled, so it does not display an overlay unless enabled.
- Ran `tests/settings-screenshots.sh` against the installed binaries under a private 1280x900 Xvfb display. All seven captures completed; the script reported PASS after checking palette persistence, Appearance scrolling and clean Settings exit. No interactive desktop window was opened.
- Removed the installed Python-only Crosshair launcher, Python app directory, fallback desktop entry and stale Python launcher shim. Shared system Python 3.14.4 was intentionally retained for other system/developer tools. The unused Python 3.12.13 interpreter, libraries and Tkinter RPMs were separately removed after a dependency dry-run showed no installed application required them; Python 3.14.4, pip and pipx tools remain functional. The Python baseline source in the Git checkout remains for CI and Python/Cairo renderer comparison; it is not an installed Crosshair launcher.
- Restored `crosshair --help` and `-h` in the native CLI, with help/version intentionally independent of the user's XDG/config directories. The isolated install smoke test covers both aliases and version.
- Added the original repository SVG to the native Linux install path. The desktop entry now uses `Icon=crosshair`; the installer installs the embedded SVG under `~/.local/share/icons/hicolor/scalable/apps/`, preflights that destination against symlinks, and removes it during uninstall. The isolated install smoke test checks icon bytes, desktop reference, symlink refusal and cleanup.
- Existing user configuration under `~/.config/crosshair` was hash-checked before and after cleanup and remained unchanged. Crosshair logs/state were retained. The previous installed Rust binaries and desktop entries are saved in private rollback snapshots under `~/.local/state/crosshair/installer-backups/`.
- Rebuilt and installed the updated Crosshair 4.0.0 binaries through the native installer. The installed CLI and Settings hashes match the PR-head `target/release` binaries; both desktop entries pass `desktop-file-validate`; the installed SVG matches the repository source. `--help`, `--version`, `--doctor`, and read-only `--list-monitors` run. The active KDE Wayland session enumerates `DP-1`, `DP-2`, and `HDMI-A-2`; no overlay was opened on the user's desktop and no game/fullscreen or pointer-passthrough test is claimed. The current config remains disabled and the original config hash is unchanged.

## Update — 2026-11-03: Windows validation status

- Actions run #145 passed the full Linux Rust job, five Python test/lint jobs and the dependency audit. The Linux job included optimized build/install tests, X11 click-through, Settings lifecycle/IPC regressions and the seven-screen screenshot artifact.
- The original Windows failure was isolated to a test-only Run-key path comparison that replaced a forward slash with two literal backslashes. The expected path now uses the same single-separator normalization as the actual registry value.
- The Windows implementation includes a deferred uninstaller launched outside the installation directory; it waits for the CLI and Settings processes to exit before deleting files. The smoke test verifies uninstall through the installed CLI from its own working directory and confirms the user config survives.
- Run #150 passed Windows formatting, native Rust tests and optimized compilation but its first integration smoke attempt was cancelled. Run #151 passed formatting, Windows-native Rust tests, optimized compilation, Linux Rust checks, all five Python matrix jobs and the dependency audit; its Windows smoke step timed out.
- Run #152 again passed the Linux Rust job, Windows formatting/native tests/optimized build, Python 3.10–3.14 matrix and dependency audit. Its diagnostic showed the installer, Run-key migration, monitor enumeration, autostart toggles and config patch all completed; the first cold `--show` call then stalled.
- Run #153 passed the complete Linux Rust job, Windows formatting/native tests/optimized build, the full Python matrix and dependency audit. The integration test timed out at the same first cold `--show` invocation. Its diagnostic artifact did not contain a `CLI TIMEOUT` record because the timeout handler then blocked awaiting redirected output tasks after terminating the CLI process tree. That harness bug is now corrected: timeout capture tasks are only read when already completed and are never awaited after a timeout.
- The current patch addresses the application path too: Windows daemon startup errors and staged monitor/window/pipe/IPC progress are recorded in `%LOCALAPPDATA%\\Crosshair\\crosshair.log`; Windows CLI named-pipe reads and writes use overlapped I/O with a shared three-second deadline; and cold-start readiness has a ten-second overall deadline. A Windows unit regression creates a pipe peer that accepts a request but deliberately never replies, and asserts the CLI returns a timeout within five seconds.
- Run #154 stopped at formatting; the Windows backend needed rustfmt layout adjustments. Runs #155 and #156 cleared formatting after the manual corrections. Run #156 also passed Windows-native Rust tests (including the new silent named-pipe peer timeout regression), optimized Windows compilation, the complete Linux Rust job, all Python-version jobs and dependency audit.
- Run #156's Windows integration smoke still timed out because the smoke wrapper used a second, parameterless `.NET Process.WaitForExit()` after its finite wait; that overload also waits for redirected stream readers which a detached daemon may keep open. The wrapper now avoids that second wait, reads output tasks only when complete and records the CLI exit code immediately.
- Run #160 passed formatting, Windows-native Rust tests (including the silent-pipe regression), optimized Windows compilation, the complete Linux Rust job, Python 3.10–3.14 and dependency audit. The integration smoke completed installation, monitor discovery, autostart migration/toggles, cold `--show`, overlay status and persisted IPC settings, and opened the native Settings window. It then failed during the Settings-close stage; no Windows package artifact was produced.
- The Settings application now appends close lifecycle milestones to `%LOCALAPPDATA%\\Crosshair\\crosshair.log` (close callback entry, Slint event-loop quit result, event-loop return, worker shutdown/join). This makes it possible to distinguish a stale window handle from a callback or worker-shutdown failure.
- Run #166 passed formatting, Windows-native Rust tests (including the silent named-pipe peer timeout regression), optimized Windows compilation, the full Linux Rust job, Python 3.10–3.14 and the dependency audit. Its smoke completed installation, monitor discovery, autostart migration/toggles, cold `--show`, status, persisted IPC settings and launching Settings. Posting asynchronous `WM_CLOSE` to the reported HWND returned success, but the Settings process remained alive and its reported main HWND changed from 262600 to 262632; the Settings close callback was not recorded. No Windows package artifact was produced.
- The latest local smoke fix enumerates every top-level HWND owned by the Settings process, logs each window's class/title/visibility, posts `WM_CLOSE` to all of them and re-enumerates/retries while the close callback has not appeared. This handles a Slint/native window handle being replaced after the first close request and should provide much more precise output if the callback remains absent. It is committed to the local working tree but still needs a fresh Windows CI run.
- The Windows smoke harness uses a Stopwatch/HasExited watchdog for every CLI, daemon-quit, Settings-close and deferred-uninstall process wait; it does not use any `WaitForExit` call. The timeout handler reads stdout/stderr only when the capture tasks have already completed, so detached descendants cannot hang the test harness.
- On Nobara, the isolated X11 overlay cold-start/lifecycle smoke completed with exit code 0 under Xvfb. The current-source Rust suite passed 52 tests, the optimized Linux build and isolated installer/autostart/uninstall smoke passed, and the seven-screen Settings screenshot smoke passed against the newly installed binaries. Both installed executable hashes now match `target/release`; the existing config hash is unchanged, the SVG icon and desktop/autostart entries validate, and no Crosshair daemon or Xvfb test process remains. Python 3.14.4 is the only installed interpreter; the unused Python 3.12.13 packages are removed, and pip/pipx applications still work.



## Release and branch relationship

This repository currently maintains two distinct application implementations:

| Item | Established application | Native rewrite |
|---|---|---|
| Git branch | `main` | `rust-port` |
| Implementation | Python / GTK3 / PyGObject / Cairo | Rust core and native platform backends, Slint Settings UI |
| Version shown in source/docs | Python core identifies as `3.2.0-candidate`; README describes the v3.2.0 line | Cargo package version `4.0.0` |
| Release posture | Existing baseline | Draft development branch; not represented as a published Rust release |
| Current review path | Default branch | Draft PR [#3](https://github.com/Roaring1/Crosshair/pull/3) |

The Rust version number is a package version, not evidence that a v4.0.0 release has been published. Keep the Python files and the default branch intact until the native implementation has passed the required platform and real-session validation.

## Feature comparison

| Area | Python baseline | Rust branch status |
|---|---|---|
| Rendering | Cairo raster path | Shared native Rust rasterizer; 20 Python/Cairo parity cases currently pass bounding-box and coverage tolerances |
| Shapes and visual settings | Dot, cross, X, plus, ring/circle and combinations; size, thickness, gap, color, opacity, outline, shadow | Shared configuration and renderer carry the existing visual model, with a separate reference dot and bounded DPI scaling |
| Settings | GTK3 settings | Slint Settings UI, live preview and profiles; color fields now include a 24-swatch picker plus custom HEX entry |
| Appearance layout | Established GTK layout | Wider Appearance workspace, bounded viewport height, independent preview column and a scroll regression that expands Outline and checks movement |
| Linux overlay | GTK / GTK Layer Shell on Wayland and X11 | Native Linux backends for Wayland and X11; compositor-specific behavior still requires a real KDE Wayland pass |
| Monitor targeting | All, primary, active, index and connector name | All/primary/index/name on both platform paths; cursor-active selection is supported on the native Windows path, while the Wayland backend explicitly fails closed where cursor-active output cannot be queried |
| IPC / CLI | Local Unix socket and CLI control | Bounded newline-framed local IPC on Linux; per-user named-pipe IPC in the Windows implementation |
| Install lifecycle | Linux install, optional autostart, uninstall and config retention | Linux install lifecycle smoke coverage; Windows per-user installer, Start Menu shortcut, opt-in Run-key autostart and uninstall preserving config are included in the draft |
| System tray | AppIndicator / GTK status icon fallback | Not yet at parity; no Rust tray process is claimed |
| Global hotkeys | Platform/compositor-dependent; KDE may bind CLI commands | No global hotkey service is claimed; Wayland users can bind the Rust CLI through compositor shortcuts |
| Windows overlay | Not part of the Python GTK Linux implementation | Native Win32 layered-window backend and smoke test are now part of the draft branch; Windows CI is the first authoritative compile/runtime gate |
| macOS | Not supported by the current baseline | Not implemented |

## Current Rust UI changes

The Settings window is widened and its content height is calculated from the space remaining below the tab header, divider and optional profile panel. This prevents the old full-window-height child from extending past the visible parent and clipping the bottom controls.

Each of the primary crosshair, outline, shadow, reference-dot and preview-background color fields uses a reusable color field. Clicking its swatch opens 24 preset colors; the existing HEX edit remains available for custom values. A chosen preset uses the normal settings callback, so the preview and persisted configuration follow the same path as typed values.

The Linux workflow captures seven 1280x900 Settings states under Xvfb and uploads the `crosshair-rust-settings-ui` artifact for 14 days. The test verifies palette selection (`#0ea5e9`), config persistence, and actual scrolling after expanding Outline. It uses temporary HOME/XDG paths and does not attach to the interactive desktop.

## Validation and evidence

The local Linux checkout has been validated with:

- 52 Rust tests passing (32 core/renderer, 18 Linux backend, 2 Settings-worker tests).
- Debug and optimized Linux builds of the CLI and Settings executables.
- 20 shared Python/Cairo rendering parity cases passing geometry and coverage tolerances.
- The isolated Linux install/autostart/uninstall lifecycle smoke test.
- Seven inspected Xvfb screenshots, including the color picker, selected-color preview, expanded Outline and lower scrolled controls.

Local Rust formatting and MSVC-target checks were unavailable on the development machine because its installed toolchain does not include `cargo-fmt` or the Windows standard-library target. The draft CI workflow installs a stable Rust toolchain and runs the Windows compile/test path on `windows-latest`; results must be checked before calling the Windows port verified.

## Release gates still open

1. Run the full Windows-native compile, tests and install/overlay/Settings/uninstall smoke against the latest branch head; produce and inspect the Windows x64 package artifact.
2. Verify the Windows x64 package on the user's Windows laptop before removing the draft/release warning.
3. Validate on a real Nobara/KDE Wayland session: overlay placement, click-through, fullscreen behavior, DPI and multi-monitor transitions, and Settings close/reopen behavior.
4. Recheck monitor selection and config migration against the established application. Keep the Git Python baseline and user configuration recoverable until this is done.
5. Decide separately whether tray parity is required for the first Rust release; it is a known gap, not a completed feature.

Until those checks pass, keep PR #3 in draft and do not label the Rust rewrite a stable replacement.
