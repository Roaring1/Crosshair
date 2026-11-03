# Crosshair for Windows

Native Rust build: Crosshair 4.0.0 (x64).

## Install

1. Extract the Windows package to a normal user-writable folder.
2. Open PowerShell in the extracted folder.
3. Run:

   pwsh -NoProfile -ExecutionPolicy Bypass -File .\Install-Crosshair.ps1

No administrator rights are required. The installer places both executables under
%LOCALAPPDATA%\Programs\Crosshair, adds that folder to the current user's PATH,
and creates a Start Menu shortcut. It does not enable login autostart.

## Run

Open Crosshair from the Start Menu. The Settings app starts separately from
the overlay. Settings can be closed while the overlay remains active. The overlay
can be hidden from Settings or with crosshair.exe --hide.

The configuration is stored at %APPDATA%\Crosshair\config.json.

## Uninstall

Run:

   pwsh -NoProfile -ExecutionPolicy Bypass -File .\Uninstall-Crosshair.ps1

This removes the Rust binaries, the Start Menu shortcut, and the PATH entry.
It intentionally retains the configuration directory.

## Compatibility and status

This package uses native Win32 layered windows, per-monitor DPI-aware rendering,
and a per-user local named pipe. The overlay is designed to remain topmost,
non-activating, and click-through. This build is x64 only and requires Windows 10
version 1607 (Anniversary Update) or later. Per-monitor-v2 awareness is used on
Windows 10 version 1703 and newer, with a per-monitor-v1 fallback on 1607/1607-era
systems.

The native Windows backend is under active validation. Run the supplied Windows
smoke test on the target laptop before considering the port fully verified,
especially for multi-monitor DPI changes, display hotplug, game fullscreen modes,
cursor passthrough, and Settings-window behavior on the user's actual desktop.
