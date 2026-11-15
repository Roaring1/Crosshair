# Crosshair

Native Rust crosshair overlay for Linux and Windows.

> **Project status:** Native Rust Crosshair `4.0.0` is the default implementation on `main` for Linux and Windows. It is in active use and will be improved iteratively as real-world issues are reported. Windows multi-monitor rendering and Settings on Windows/Linux have been user-tested; other platform-specific behavior can be refined through normal follow-up changes. See [RUST-PORT-STATUS.md](RUST-PORT-STATUS.md) for the implementation and deployment history.

## Platform support

| Platform | Backend | Current status |
| --- | --- | --- |
| Linux / Wayland | Native Wayland layer-shell client | Implemented and installed on Nobara; compositor-specific behavior is refined from normal use |
| Linux / X11 | Native X11 overlay | Implemented; click-through and lifecycle smoke coverage is in CI |
| Windows 10 1607+ / x64 | Win32 layered window and per-user named-pipe IPC | Implemented and installed on the Windows 11 laptop; multi-monitor rendering and Settings are user-confirmed |
| macOS | — | Not implemented |

The overlay is designed to remain topmost, non-activating and click-through. The overlay daemon exists only while the overlay is enabled. Turning it off removes its windows, closes its IPC endpoint and exits the daemon; an off state is not represented by a hidden resident process. Settings is a separate Slint application and can close while an enabled overlay continues running.

The Windows build includes an optional native notification-area controller. It is available only while the overlay is enabled and the user explicitly selects **System tray** mode. Turning the overlay off from the tray, Settings or CLI closes both the overlay daemon and tray controller. The Start Menu app launcher opens foreground Settings while OFF as a re-enable path; a direct `--tray` launch exits immediately, leaving no background process. After Settings saves an enabled tray-mode configuration and closes, the tray controller starts. A lightweight wait on the per-user named-pipe endpoint detects external daemon shutdown without periodic status traffic. The tray's **Exit Crosshair** action waits for the overlay shutdown acknowledgement before closing. Linux/Wayland intentionally uses the app UI or explicit CLI commands rather than claiming a portable system tray. No global hotkey service is claimed; KDE/Wayland users can bind `crosshair --toggle` to a compositor shortcut.

## Features

- Shapes: dot, cross, X, plus, ring/circle, and combinations.
- Adjustable size, gap, thickness, color, opacity, outline, shadow and reference dot.
- Monitor selection: all, primary, index and connector name; active/cursor-based selection is supported by the Windows path.
- Live Settings preview, saved profiles and bounded configuration validation.
- App Activity launch preference: App UI, System tray (Windows), or Command only. Explicit Settings and CLI actions remain available in every mode.
- Strict on-demand lifecycle: no overlay daemon or Windows tray controller remains while disabled. Tray mode is available only while the overlay is enabled. Login autostart is an explicit opt-in.
- Local newline-framed IPC on Linux and a per-user local named pipe on Windows.
- Atomic configuration writes; uninstall retains appearance and monitor preferences while leaving the overlay disabled.

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

Download the `Crosshair-Windows-x64` artifact from a successful [main-branch Actions run](https://github.com/Roaring1/Crosshair/actions/workflows/checks.yml?query=branch%3Amain), extract it to a normal user-writable directory, then run this from PowerShell:

```powershell
pwsh -NoProfile -ExecutionPolicy Bypass -File .\Install-Crosshair.ps1
```

No administrator rights are required. The installer targets `%LOCALAPPDATA%\Programs\Crosshair`, adds that directory to the current user's PATH, and creates a Start Menu shortcut for Settings. A fresh install does not enable login autostart. Existing explicit opt-in startup is migrated to the new native executable.

The in-app uninstall action defers file removal until the CLI and Settings processes have exited. Configuration under `%APPDATA%\Crosshair` is retained.

The hosted Windows smoke covers the installer, overlay, named-pipe IPC, Settings, tray and shutdown lifecycle. The Windows 11 installation has been updated from the current CI package; the overlay renders correctly across multiple monitors and Settings has been user-confirmed. More unusual display, fullscreen, pointer and sleep/resume behavior should be reported and fixed as it is encountered during normal use, rather than treated as a release gate.

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

Legacy Python source is retained in the repository as a compatibility and renderer-parity reference for CI. It is not installed or launched by the native Rust package. The native Rust application is the default Crosshair implementation on `main` and is what the Linux and Windows installers deploy.

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
xvfb-run -a -s "-screen 0 1280x900x24" bash tests/settings-autostart-smoke.sh
bash tests/settings-async-io-smoke.sh
bash tests/cli-ipc-failure-smoke.sh
bash tests/settings-ipc-recovery-smoke.sh
bash tests/settings-ipc-restart-smoke.sh
xvfb-run -a bash tests/settings-close-smoke.sh
xvfb-run -a -s "-screen 0 1280x900x24" bash tests/settings-screenshots.sh
```

The Linux job additionally captures eight Settings UI screenshots under an isolated Xvfb display, including General/Appearance navigation, color selection, expanded controls and scrolling. The Windows job runs native Rust tests, optimized builds and a PowerShell install/overlay/IPC/Settings/uninstall smoke test before packaging the x64 installer.

See [RUST-PORT-TESTING.md](RUST-PORT-TESTING.md) for the CI commands, isolated Wayland compositor smoke and screenshot coverage. See [RUST-PORT-STATUS.md](RUST-PORT-STATUS.md) for the current repository/deployment state and historical implementation record.

## License status

This repository currently has no `LICENSE` file. No open-source license is implied; do not assume permission to redistribute or publish modified builds until the project owner selects and adds a license.
