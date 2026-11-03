# Native Rust port testing

The Rust rewrite is developed on the `rust-port` branch. Its normal automated
checks are:

```bash
cargo fmt --all -- --check
cargo test --locked --features settings-ui --all-targets
cargo build --locked --features settings-ui --bins
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

## Optional live Wayland compositor smoke

On a KDE Plasma system with `kwin_wayland`, `wayland-info`, and
`dbus-run-session` installed, build the debug binary and run:

```bash
cargo build --locked --features settings-ui --bins
bash tests/wayland-kwin-smoke.sh
```

The script starts a separate KWin virtual-framebuffer compositor inside a
private DBus session. It uses temporary `HOME`, runtime, config, data, cache,
and state directories; it does not connect to or change the user's active
compositor or saved desktop settings. It verifies the `wlr-layer-shell`
global, output discovery, initial layer-surface configure/render, live
appearance IPC, and graceful daemon shutdown.

This is a real layer-shell client/compositor test, but KWin's virtual backend
currently exposes a single integer-scale virtual output. It does not validate
fractional scaling, physical multi-monitor layout, hotplug, GPU scanout,
fullscreen game behavior, or target-machine pointer passthrough. Those remain
separate manual release checks.


## Settings UI screenshots

The Linux CI job captures the Rust Settings window under a private Xvfb display.
It records General, Appearance, the interactive color palette, and the scrolled
lower Appearance controls. The screenshots are uploaded as the
`crosshair-rust-settings-ui` workflow artifact; they are diagnostic evidence,
not a substitute for visual review on the target KDE Wayland session.

To capture them locally after a debug build:

```bash
xvfb-run -a -s "-screen 0 1280x900x24" bash tests/settings-screenshots.sh
```

The script uses temporary HOME/XDG directories and shuts down its own overlay
and Settings processes on exit. It does not attach to or change the active
desktop session.


### Visual capture coverage

`tests/settings-screenshots.sh` records seven screenshots into
`artifacts/settings-ui/` under a private Xvfb display:

1. General tab.
2. Appearance tab at the top.
3. Open 24-color palette.
4. Selected preset (`#0ea5e9`) with a config-persistence assertion.
5. Appearance with Outline expanded.
6. The lower Appearance controls after wheel scrolling, with a movement
   assertion against the expanded-top capture.
7. Appearance returned to the top.

The workflow uploads the directory as the
`crosshair-rust-settings-ui` artifact for 14 days. The test isolates HOME and
all XDG paths, creates and shuts down only its own overlay process, and never
interacts with the logged-in desktop.
