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
