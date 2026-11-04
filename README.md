# Crosshair

Native Rust crosshair overlay for Linux and Windows.

> **Development-branch notice:** This is the native Rust rewrite on `rust-port` (Cargo version `4.0.0`), not a published stable release. The established Python application and its release history remain on `main`; this branch is a separate implementation. Windows multi-monitor rendering and Settings on Windows/Linux have been user-tested. Some platform-specific fullscreen, pointer-passthrough, DPI and monitor-hotplug checks remain unclaimed. See [RUST-PORT-STATUS.md](RUST-PORT-STATUS.md) for the current validation record.

## Platform support

| Platform | Backend | Current status |
| --- | --- | --- |
| Linux / Wayland | Native Wayland layer-shell client | Implemented; Settings is user-confirmed on Linux; live pointer-passthrough and fullscreen acceptance remain open |
| Linux / X11 | Native X11 overlay | Implemented; automated Xvfb click-through and lifecycle checks pass |
| Windows 10 1607+ / x64 | Win32 layered window and per-user named-pipe IPC | User-confirmed multi-monitor rendering and Settings; hosted installer/tray/overlay tests pass; mixed-DPI, hot-plug and fullscreen acceptance remain separate checks |
| macOS | — | Not implemented |

The overlay is designed to remain topmost, non-activating and click-through. Settings is a separate Slint application and can close without necessarily closing an enabled overlay.

The Windows build includes a native notification-area controller. Linux/Wayland intentionally uses the app UI or explicit CLI commands rather than claiming a portable system tray. No global hotkey service is claimed; KDE/Wayland users can bind `crosshair --toggle` to a compositor shortcut.

## Features

- Shapes: dot, cross, X, plus, ring/circle, and combinations.
- Adjustable size, gap, thickness, color, opacity, outline, shadow and reference dot.
- Monitor selection: all, primary, index and connector name; active/cursor-based selection is supported by the Windows path.
- Live Settings preview, saved profiles and bounded configuration validation.
- App Activity launch preference: App UI, System tray (Windows), or Command only. Explicit Settings and CLI actions remain available in every mode.
- On-demand daemon: no overlay process is required while hidden. Login autostart is an explicit opt-in.
- Local newline-framed IPC on Linux and a per-user local named pipe on Windows.
- Atomic configuration writes and config retention when the application is uninstalled.

## Build from source

Use a current stable Rust toolchain (the crate declares Rust 1.92 or newer). Build the CLI and Settings application together:

```bash
cargo build --locked --release --features settings-ui --bins
```

The outputs are:

- `target/release/crosshair`
- `target/release/crosshair-settings`

The Linux build uses the native Wayland/X11 system libraries and Fontconfig. The CI workflow installs the corresponding development packages before compiling. The Windows x64 build is produced by the `windows-latest` CI job.

## Linux install

Run the installer from the build directory so it can find both sibling executables:

```bash
./target/release/crosshair --install
```

This installs the native CLI and Settings binary under `~/.local/bin`, creates a desktop-menu entry, installs the bundled SVG in the hicolor icon theme, and removes a stale opt-in autostart entry. It does not enable login startup by default.

```bash
~/.local/bin/crosshair --config       # open Settings
~/.local/bin/crosshair --toggle       # cold start shows; live daemon toggles
~/.local/bin/crosshair --status
~/.local/bin/crosshair --doctor
```

Enable login startup only when explicitly wanted:

```bash
~/.local/bin/crosshair --autostart
~/.local/bin/crosshair --remove-autostart
```

Uninstall removes the installed executables, desktop entry, icon and autostart entry, while retaining the user's configuration:

```bash
~/.local/bin/crosshair --uninstall
```

## Windows x64 package

The Windows CI job builds the two native executables, exercises the per-user installer and overlay lifecycle, and packages:

- `crosshair.exe`
- `crosshair-settings.exe`
- `Install-Crosshair.ps1`
- `Uninstall-Crosshair.ps1`
- `README-WINDOWS.txt`

Download the `Crosshair-Windows-x64` artifact from a successful [PR #3 Actions run](https://github.com/Roaring1/Crosshair/actions), extract it to a normal user-writable directory, then run this from PowerShell:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\Install-Crosshair.ps1
```

No administrator rights are required. The installer targets `%LOCALAPPDATA%\Programs\Crosshair`, adds that directory to the current user's PATH, and creates a Start Menu shortcut for Settings. A fresh install does not enable login autostart. Existing explicit opt-in startup is migrated to the new native executable.

The in-app uninstall action defers file removal until the CLI and Settings processes have exited. Configuration under `%APPDATA%\Crosshair` is retained.

The hosted Windows smoke covers the installer, overlay, named-pipe IPC, Settings, tray and shutdown lifecycle. User testing has also confirmed that the overlay renders correctly across multiple monitors and that Settings works on Windows. Mixed/fractional DPI, monitor hot-plug, fullscreen-game behavior, pointer passthrough and sleep/resume are distinct checks and are not implied by those results.

## CLI

Run `crosshair --help` for the complete command list. Help and version are filesystem-free, so they also work before installation or before a configuration directory exists.

Common commands:

```text
crosshair --config
crosshair --toggle
crosshair --show
crosshair --hide
crosshair --set '{"shape":"cross","size":31}'
crosshair --reload
crosshair --restart
crosshair --quit
crosshair --status
crosshair --doctor
crosshair --list-monitors
crosshair --autostart
crosshair --remove-autostart
crosshair --uninstall
```

`--set` accepts a JSON object of configuration fields. Values are validated against the same configuration model used by Settings. A cold `--toggle` starts the overlay enabled; a toggle against a running daemon changes its current state.

## Configuration and logs

| Purpose | Linux | Windows |
| --- | --- | --- |
| Configuration | `${XDG_CONFIG_HOME:-~/.config}/crosshair/config.json` | `%APPDATA%\Crosshair\config.json` |
| CLI | `${XDG_BIN_HOME:-~/.local/bin}/crosshair` | `%LOCALAPPDATA%\Programs\Crosshair\crosshair.exe` |
| Settings | `${XDG_BIN_HOME:-~/.local/bin}/crosshair-settings` | `%LOCALAPPDATA%\Programs\Crosshair\crosshair-settings.exe` |
| App-menu / Start Menu | `${XDG_DATA_HOME:-~/.local/share}/applications/crosshair.desktop` | `%APPDATA%\Microsoft\Windows\Start Menu\Programs\Crosshair.lnk` |
| Log | `${XDG_STATE_HOME:-~/.local/state}/crosshair/crosshair.log` | `%LOCALAPPDATA%\Crosshair\crosshair.log` |

On Windows, daemon startup, display discovery, overlay creation, and named-pipe request stages are appended to the Crosshair log under `%LOCALAPPDATA%\Crosshair\crosshair.log`.

The legacy Python source in this branch is retained as a compatibility and renderer-parity reference for CI. It is not installed or launched by the native Rust package. The established Python application and its release history remain on `main`.

## Validation

The automated workflow runs:

```bash
cargo fmt --all -- --check
cargo test --locked --features settings-ui --all-targets
cargo build --locked --release --features settings-ui --bins
python3 tests/render-parity.py
bash tests/install-smoke.sh
CROSSHAIR_BIN=target/release/crosshair bash tests/install-smoke.sh
xvfb-run -a bash tests/x11-smoke.sh
xvfb-run -a bash tests/settings-autostart-smoke.sh
bash tests/settings-async-io-smoke.sh
bash tests/settings-ipc-recovery-smoke.sh
bash tests/settings-ipc-restart-smoke.sh
xvfb-run -a bash tests/settings-close-smoke.sh
```

The Linux job additionally captures eight Settings UI screenshots under an isolated Xvfb display, including General/Appearance navigation, color selection, expanded controls and scrolling. The Windows job runs native Rust tests, optimized builds and a PowerShell install/overlay/IPC/Settings/uninstall smoke test before packaging the x64 installer.

See [RUST-PORT-TESTING.md](RUST-PORT-TESTING.md) for the isolated Wayland compositor test, screenshot coverage and limitations. See [RUST-PORT-STATUS.md](RUST-PORT-STATUS.md) for current CI evidence and remaining platform checks.

## License status

This repository currently has no `LICENSE` file. No open-source license is implied; do not assume permission to redistribute or publish modified builds until the project owner selects and adds a license.
