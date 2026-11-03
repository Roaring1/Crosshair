# Native Rust port status

_Last reviewed: 2026-11-02_

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

1. Complete the current GitHub Actions run, including Windows-native compile, tests and the Windows install/overlay/Settings/uninstall smoke test.
2. Inspect the uploaded Linux Settings screenshots and Windows x64 package artifacts from CI.
3. Validate on a real Nobara/KDE Wayland session: overlay placement, click-through, fullscreen behavior, DPI and multi-monitor transitions, and Settings close/reopen behavior.
4. Recheck monitor selection and config migration against the established application. Keep the Python baseline and configuration recoverable until this is done.
5. Decide separately whether tray parity is required for the first Rust release; it is a known gap, not a completed feature.

Until those checks pass, keep PR #3 in draft and do not label the Rust rewrite a stable replacement.
