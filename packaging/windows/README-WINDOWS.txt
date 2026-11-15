# Crosshair for Windows

Native Rust build: Crosshair 4.0.0 (x64).

## Install

1. Extract the Windows package to a normal user-writable folder.
2. Open PowerShell in the extracted folder.
3. Run:

   pwsh -NoProfile -ExecutionPolicy Bypass -File .\Install-Crosshair.ps1

No administrator rights are required. The installer places both executables and
the tray icon under %LOCALAPPDATA%\Programs\Crosshair, adds that folder to the
current user's PATH, and creates a Start Menu shortcut. It does not enable login
autostart.

## Run

Open **Crosshair** from the Start Menu. It follows the saved App Activity preference.
The separate **Crosshair Settings** Start Menu entry always opens Settings. Under
General, choose **App Activity**:

- **App UI** opens the full Settings window on a plain launch.
- **System tray** starts a small Crosshair icon in the notification area. When the
  saved crosshair Enabled setting is on, launch also starts or reveals the overlay.
  Left-click the icon to toggle the crosshair; right-click for **Show app UI**,
  **Toggle crosshair**, and **Exit Crosshair**. Turning the overlay off closes
  both the overlay daemon and the tray controller.
- **Command only** makes a plain launch exit quietly. Use an explicit command to
  show, hide, or toggle the overlay.

These are launch preferences, not feature locks. Settings can always be opened
from the **Crosshair Settings** Start Menu entry or with `crosshair --config`.
The Start Menu **Crosshair** entry opens Settings while the overlay is OFF, so it
is always possible to re-enable the app. An explicit `crosshair --tray` launch
exits immediately when the overlay is disabled; it does not leave a background
controller behind. The tray is a companion to an enabled overlay, not an
independent background process. Closing Settings leaves an enabled overlay
running. Turning the overlay OFF from the tray, Settings, or CLI closes both the
overlay daemon and tray controller. **Exit Crosshair** hides/disables the overlay
before closing the tray; if shutdown fails, the tray stays available. The tray's
**Show app UI** action works even when System tray is the saved default.

The command-line controls are always available regardless of App Activity:

- `crosshair --toggle`
- `crosshair --show`
- `crosshair --hide`
- `crosshair --status`
- `crosshair --config`

The configuration is stored at %APPDATA%\Crosshair\config.json.

## Uninstall

Run:

   pwsh -NoProfile -ExecutionPolicy Bypass -File .\Uninstall-Crosshair.ps1

This removes the Rust binaries, the tray icon, Start Menu shortcut, login-startup
entry, and PATH entry. It intentionally retains the configuration directory.

## Compatibility and status

This package uses native Win32 layered windows, per-monitor DPI-aware rendering,
and a per-user local named pipe. The overlay is designed to remain topmost,
non-activating, and click-through. This build is x64 only and requires Windows 10
version 1607 (Anniversary Update) or later. Per-monitor-v2 awareness is used on
Windows 10 version 1703 and newer, with a per-monitor-v1 fallback on older systems.

The GitHub Actions Windows job compiles and tests the native backend, then runs
the per-user installer, overlay, named-pipe IPC, Settings, tray and shutdown smoke
before packaging the x64 build. The Windows 11 target installation has been
updated and the overlay and Settings have been user-confirmed. Unusual mixed or
fractional DPI transitions, display hotplug, fullscreen-game behavior and cursor
passthrough remain useful real-world feedback areas; report issues as they occur
rather than treating them as blockers to routine updates.
