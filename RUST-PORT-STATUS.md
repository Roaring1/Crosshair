# Native Rust port status

_Last reviewed: 2026-11-03_

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
- Rebuilt and installed the updated Crosshair 4.0.0 binaries through the native installer. The installed CLI and Settings hashes match `target/release`; both desktop entries pass `desktop-file-validate`; the installed SVG matches the repository source. `--help`, `--version` and `--doctor` run, with no overlay daemon active and the saved overlay setting disabled. The new installation has not yet been exercised in an active KDE Wayland session or a game.

## Update — 2026-11-03: Windows validation status

- Actions run #145 passed the full Linux Rust job, five Python test/lint jobs and the dependency audit. The Linux job included optimized build/install tests, X11 click-through, Settings lifecycle/IPC regressions and the seven-screen screenshot artifact.
- The original Windows failure was isolated to a test-only Run-key path comparison that replaced a forward slash with two literal backslashes. The expected path now uses the same single-separator normalization as the actual registry value.
- The Windows implementation includes a deferred uninstaller launched outside the installation directory; it waits for the CLI and Settings processes to exit before deleting files. The smoke test verifies uninstall through the installed CLI from its own working directory and confirms the user config survives.
- Run #150 passed Windows formatting, native Rust tests and optimized compilation but its first integration smoke attempt was cancelled. Run #151 passed formatting, Windows-native Rust tests, optimized compilation, Linux Rust checks, all five Python matrix jobs and the dependency audit; its Windows smoke step timed out.
- Run #152 again passed the Linux Rust job, Windows formatting/native tests/optimized build, Python 3.10–3.14 matrix and dependency audit. Its diagnostic showed the installer, Run-key migration, monitor enumeration, autostart toggles and config patch all completed; the first cold `--show` call then stalled.
- Run #153 passed the complete Linux Rust job, Windows formatting/native tests/optimized build, the full Python matrix and dependency audit. The integration test timed out at the same first cold `--show` invocation. Its diagnostic artifact did not contain a `CLI TIMEOUT` record because the timeout handler then blocked awaiting redirected output tasks after terminating the CLI process tree. That harness bug is now corrected: timeout capture tasks are only read when already completed and are never awaited after a timeout.
- The current patch addresses the application path too: Windows daemon startup errors and staged monitor/window/pipe/IPC progress are recorded in `%LOCALAPPDATA%\\Crosshair\\crosshair.log`; Windows CLI named-pipe reads and writes use overlapped I/O with a shared three-second deadline; and cold-start readiness has a ten-second overall deadline. A Windows unit regression creates a pipe peer that accepts a request but deliberately never replies, and asserts the CLI returns a timeout within five seconds.
- Run #154 stopped at formatting; the Windows backend needed rustfmt layout adjustments. Runs #155 and #156 cleared formatting after the manual corrections. Run #156 also passed Windows-native Rust tests (including the new silent named-pipe peer timeout regression), optimized Windows compilation, the complete Linux Rust job, all Python-version jobs and dependency audit.
- Run #156's Windows integration smoke still timed out. Its uploaded log shows the install, monitor discovery, Run-key migration, autostart toggles and config update all returned successfully; the first cold `--show` call then stayed open until the eight-minute test-step timeout. No Windows package artifact was produced.
- The log stopped before the PowerShell timeout handler recorded anything. The remaining harness suspect is `.NET Process.Kill(true)`/subsequent `WaitForExit`, which can itself block when child processes inherit redirected handles. The current local revision terminates only the timed-out CLI PID without waiting, checks output tasks only when already complete, disposes the process handle, and then lets finally clean up the exact-path daemon/Settings processes. This harness change still needs a fresh CI run. Windows laptop validation remains open.



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
