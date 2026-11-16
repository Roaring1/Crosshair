#[cfg(target_os = "linux")]
#[path = "../platform/linux/wayland.rs"]
mod wayland;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/x11.rs"]
mod x11;

use crosshair_core::{APP_VERSION, AppPaths, Config, MonitorDescriptor};
#[cfg(target_os = "linux")]
use crosshair_core::{
    atomic_write_user_executable, atomic_write_user_file, remove_user_file, remove_user_files,
    validate_user_file_targets,
};
#[cfg(unix)]
use std::io::{BufReader, Write};
use std::{
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

#[cfg(target_os = "linux")]
fn discover_monitors() -> Result<Vec<MonitorDescriptor>, String> {
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        wayland::list_outputs()
    } else if std::env::var_os("DISPLAY").is_some() {
        x11::list_outputs()
    } else {
        Err("No Wayland or X11 display session was detected.".into())
    }
}

#[cfg(target_os = "windows")]
fn discover_monitors() -> Result<Vec<MonitorDescriptor>, String> {
    crosshair_core::windows::list_monitors()
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn discover_monitors() -> Result<Vec<MonitorDescriptor>, String> {
    Err("Monitor enumeration is not implemented for this platform.".into())
}

fn monitor_selection_warning(config: &Config, monitors: &[MonitorDescriptor]) -> Option<String> {
    if monitors.is_empty() {
        return Some("No active displays are visible to this session.".into());
    }

    match config.monitor_mode.as_str() {
        "name"
            if !monitors.iter().any(|monitor| {
                monitor.name == config.monitor_name
                    && monitor.ordinal == config.monitor_name_ordinal
            }) =>
        {
            Some(format!(
                "Saved monitor target {} (match {}) is not connected; no fallback monitor will be used.",
                config.monitor_name,
                config.monitor_name_ordinal + 1
            ))
        }
        "index" if config.monitor_index as usize >= monitors.len() => Some(format!(
            "Saved monitor index {} is unavailable; {} output(s) are connected.",
            config.monitor_index,
            monitors.len()
        )),
        _ => None,
    }
}

fn print_monitor_diagnostics(config: &Config) {
    match discover_monitors() {
        Ok(monitors) => {
            let connected = if monitors.is_empty() {
                "none".into()
            } else {
                monitors
                    .iter()
                    .map(|monitor| {
                        if monitor.ordinal == 0 {
                            monitor.name.clone()
                        } else {
                            format!("{}#{}", monitor.name, monitor.ordinal + 1)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ")
            };
            println!("Connected outputs: {connected}");
            if let Some(warning) = monitor_selection_warning(config, &monitors) {
                println!("Monitor selection: {warning}");
            } else {
                println!("Monitor selection: {}", config.monitor_mode);
            }
        }
        Err(error) => println!("Output discovery: unavailable ({error})"),
    }
}

#[cfg(test)]
mod monitor_diagnostic_tests {
    use super::monitor_selection_warning;
    use crosshair_core::{Config, MonitorDescriptor};

    #[test]
    fn doctor_warns_when_saved_named_monitor_is_missing_without_selecting_a_fallback() {
        let config = Config {
            monitor_mode: "name".into(),
            monitor_name: "DP-2".into(),
            monitor_name_ordinal: 0,
            ..Config::default()
        };
        let connected = vec![MonitorDescriptor {
            name: "HDMI-A-2".into(),
            ordinal: 0,
        }];

        assert_eq!(
            monitor_selection_warning(&config, &connected).as_deref(),
            Some(
                "Saved monitor target DP-2 (match 1) is not connected; no fallback monitor will be used."
            )
        );
    }

    #[test]
    fn doctor_accepts_a_matching_duplicate_name_ordinal() {
        let config = Config {
            monitor_mode: "name".into(),
            monitor_name: "DP-1".into(),
            monitor_name_ordinal: 1,
            ..Config::default()
        };
        let connected = vec![
            MonitorDescriptor {
                name: "DP-1".into(),
                ordinal: 0,
            },
            MonitorDescriptor {
                name: "DP-1".into(),
                ordinal: 1,
            },
        ];

        assert_eq!(monitor_selection_warning(&config, &connected), None);
    }
}

#[cfg(unix)]
fn send_command(
    paths: &AppPaths,
    command: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(&paths.socket_file).map_err(|error| {
        if matches!(
            error.kind(),
            std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
        ) {
            format!("Crosshair overlay is not running: {error}")
        } else {
            format!("Could not connect to Crosshair overlay IPC: {error}")
        }
    })?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let mut request = serde_json::json!({"cmd": command});
    if let Some(payload) = payload {
        request["payload"] = payload;
    }
    let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() >= crosshair_core::MAX_IPC_MESSAGE_BYTES {
        return Err("Control request exceeds 64 KiB".into());
    }
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| error.to_string())?;
    let response = crosshair_core::read_bounded_line(BufReader::new(stream))
        .map_err(|error| format!("Invalid response from overlay: {error}"))?;
    let response: serde_json::Value = serde_json::from_slice(&response)
        .map_err(|error| format!("Invalid response from overlay: {error}"))?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(response
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Overlay rejected command")
            .to_string());
    }
    Ok(response)
}

#[cfg(target_os = "windows")]
fn send_command(
    paths: &AppPaths,
    command: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    crosshair_core::windows::send_command(paths, command, payload)
}

#[cfg(not(any(unix, target_os = "windows")))]
fn send_command(
    _: &AppPaths,
    _: &str,
    _: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    Err("This platform's native IPC backend is not implemented yet".into())
}

fn overlay_is_not_running(error: &str) -> bool {
    error.starts_with("Crosshair overlay is not running")
}

fn sibling_executable(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let path = current
        .parent()
        .unwrap_or(Path::new("."))
        .join(if cfg!(windows) {
            format!("{name}.exe")
        } else {
            name.to_string()
        });
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!(
            "Required Crosshair executable was not found: {}",
            path.display()
        ))
    }
}

fn launch_settings() -> Result<(), String> {
    let executable = sibling_executable("crosshair-settings")?;
    Command::new(executable)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not open Crosshair Settings: {error}"))?;
    Ok(())
}

fn spawn_daemon(paths: &AppPaths) -> Result<std::process::Child, String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    let mut command = Command::new(executable);
    command.arg("--daemon");

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    let stderr = {
        let parent = paths
            .log_file
            .parent()
            .ok_or_else(|| "Crosshair log path has no parent directory".to_string())?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("Could not create Crosshair log directory: {error}"))?;
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&paths.log_file)
            .map_err(|error| format!("Could not open Crosshair startup log: {error}"))?;
        Stdio::from(log)
    };

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    let stderr = Stdio::null();

    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .map_err(|error| format!("Could not start Crosshair: {error}"))
}

fn start_enabled(paths: &AppPaths) -> Result<(), String> {
    let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    let monitors = discover_monitors()
        .map_err(|error| format!("Could not start Crosshair: {error}"))?;
    if let Some(warning) = monitor_selection_warning(&config, &monitors) {
        return Err(format!("Could not start Crosshair: {warning}"));
    }

    config.enabled = true;
    config
        .save_atomic(&paths.config_file)
        .map_err(|error| error.to_string())?;

    let mut child = spawn_daemon(paths)?;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        match child.try_wait() {
            Ok(Some(status)) => {
                return Err(format!(
                    "Crosshair daemon exited before becoming ready ({status}). Startup details: {}",
                    paths.log_file.display()
                ));
            }
            Ok(None) => {}
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!(
                    "Could not inspect Crosshair daemon startup; the child was stopped: {error}"
                ));
            }
        }

        if let Ok(response) = send_command(paths, "status", None) {
            if response.get("enabled").and_then(serde_json::Value::as_bool) == Some(true) {
                return Ok(());
            }
        }
        thread::sleep(Duration::from_millis(25));
    }

    // Do not leave an unresponsive child behind after the caller reports a
    // failed cold start. The daemon owns no external state beyond its own
    // process and overlay, so terminating this exact child is safe here.
    let _ = child.kill();
    let _ = child.wait();
    Err(format!(
        "Crosshair did not become ready within ten seconds; the unresponsive startup process was stopped. Startup details: {}",
        paths.log_file.display()
    ))
}

fn ensure_running_then(paths: &AppPaths, command: &str) -> Result<(), String> {
    match send_command(paths, command, None) {
        Ok(_) => Ok(()),
        // A cold toggle is a show operation by definition: there is no live
        // state to invert. Starting and then sending "toggle" would turn it OFF.
        Err(error) if overlay_is_not_running(&error) => start_enabled(paths),
        Err(error) => Err(format!(
            "Crosshair may still be running but did not acknowledge {command}: {error}"
        )),
    }
}

fn print_status(paths: &AppPaths) -> Result<(), String> {
    let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    match send_command(paths, "status", None) {
        Ok(response) => {
            println!("Crosshair daemon: running");
            println!(
                "Overlay: {}",
                if response["enabled"].as_bool() == Some(true) {
                    "enabled"
                } else {
                    "disabled"
                }
            );
            if let Some(monitors) = response["monitors"].as_array() {
                for monitor in monitors {
                    if let Some(name) = monitor.as_str() {
                        println!("Output: {name}");
                    }
                }
            }
        }
        Err(error) if overlay_is_not_running(&error) => {
            println!("Crosshair daemon: not running");
            println!(
                "Overlay setting: {}",
                if config.enabled {
                    "enabled (will show on next start)"
                } else {
                    "disabled"
                }
            );
        }
        Err(error) => {
            return Err(format!(
                "Could not determine whether the Crosshair daemon is still running: {error}"
            ));
        }
    }
    Ok(())
}

fn restart_daemon(paths: &AppPaths) -> Result<(), String> {
    let enabled = Config::load(&paths.config_file)
        .map_err(|error| error.to_string())?
        .enabled;
    match send_command(paths, "quit", None) {
        Ok(_) => {
            let mut stopped = false;
            for _ in 0..120 {
                match send_command(paths, "status", None) {
                    Ok(_) => thread::sleep(Duration::from_millis(25)),
                    Err(error) if overlay_is_not_running(&error) => {
                        stopped = true;
                        break;
                    }
                    Err(error) => {
                        return Err(format!(
                            "Crosshair was asked to stop but its shutdown could not be confirmed: {error}"
                        ));
                    }
                }
            }
            if !stopped {
                return Err(
                    "Crosshair did not stop within three seconds; refusing to start a second daemon."
                        .into(),
                );
            }
        }
        Err(error) if overlay_is_not_running(&error) => {}
        Err(error) => {
            return Err(format!(
                "Could not stop the existing Crosshair daemon before restart: {error}"
            ));
        }
    }
    if enabled {
        start_enabled(paths)?;
    }
    Ok(())
}

fn import_profile(paths: &AppPaths, code: &str) -> Result<(), String> {
    let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    config
        .import_code(code)
        .map_err(|error| error.to_string())?;
    config
        .save_atomic(&paths.config_file)
        .map_err(|error| error.to_string())?;
    match send_command(
        paths,
        "set",
        Some(serde_json::to_value(&config).map_err(|error| error.to_string())?),
    ) {
        Ok(_) => println!("Crosshair profile imported and applied."),
        Err(error) if overlay_is_not_running(&error) => {
            println!(
                "Crosshair profile imported. It will be used the next time the overlay is shown."
            );
        }
        Err(error) => {
            return Err(format!(
                "Profile was saved, but the running overlay did not acknowledge the update: {error}"
            ));
        }
    }
    Ok(())
}

fn open_folder(paths: &AppPaths) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let (program, argument) = ("explorer.exe", paths.config_dir.as_os_str().to_owned());
    #[cfg(target_os = "linux")]
    let (program, argument) = ("xdg-open", paths.config_dir.as_os_str().to_owned());
    Command::new(program)
        .arg(argument)
        .spawn()
        .map_err(|error| format!("Could not open config folder: {error}"))?;
    Ok(())
}

#[cfg(target_os = "linux")]
fn install_bin_dir() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME is not set to an absolute path".to_string())?;
    Ok(std::env::var_os("XDG_BIN_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/bin")))
}

#[cfg(target_os = "windows")]
fn install_bin_dir() -> Result<PathBuf, String> {
    let local = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "LOCALAPPDATA is not set to an absolute path".to_string())?;
    Ok(local.join("Programs/Crosshair"))
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
fn install_bin_dir() -> Result<PathBuf, String> {
    Err("Installation is not implemented for this platform yet.".into())
}

#[cfg(target_os = "linux")]
fn autostart_entry_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME is not set to an absolute path".to_string())?;
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".config"));
    Ok(config_home.join("autostart").join("crosshair.desktop"))
}

#[cfg(target_os = "linux")]
fn applications_entry_path() -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .ok_or_else(|| "HOME is not set to an absolute path".to_string())?;
    let data_home = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
        .unwrap_or_else(|| home.join(".local/share"));
    Ok(data_home.join("applications").join("crosshair.desktop"))
}

#[cfg(target_os = "linux")]
fn application_icon_path() -> Result<PathBuf, String> {
    let app_entry = applications_entry_path()?;
    let data_home = app_entry
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| "Could not resolve the XDG data directory for the app icon".to_string())?;
    Ok(data_home.join("icons/hicolor/scalable/apps/crosshair.svg"))
}

#[cfg(target_os = "linux")]
const APPLICATION_ICON_SVG: &[u8] = include_bytes!("../../crosshair.svg");

#[cfg(target_os = "linux")]
fn read_install_source(path: &Path) -> Result<Vec<u8>, String> {
    use std::io::Read;
    use std::os::unix::fs::OpenOptionsExt;

    // Debug Settings builds can exceed 250 MiB because of Slint metadata;
    // keep installs bounded while supporting both debug and release artifacts.
    const MAX_INSTALL_SOURCE_BYTES: u64 = 512 * 1024 * 1024;
    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let source = options.open(path).map_err(|error| {
        format!(
            "Could not safely open install source {}: {error}",
            path.display()
        )
    })?;
    let metadata = source.metadata().map_err(|error| {
        format!(
            "Could not inspect install source {}: {error}",
            path.display()
        )
    })?;
    if !metadata.is_file() {
        return Err(format!(
            "Install source is not a regular file: {}",
            path.display()
        ));
    }
    if metadata.len() > MAX_INSTALL_SOURCE_BYTES {
        return Err(format!(
            "Install source is unexpectedly large: {}",
            path.display()
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    source
        .take(MAX_INSTALL_SOURCE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| format!("Could not read install source {}: {error}", path.display()))?;
    if bytes.len() as u64 > MAX_INSTALL_SOURCE_BYTES {
        return Err(format!(
            "Install source grew beyond the size limit: {}",
            path.display()
        ));
    }
    Ok(bytes)
}

#[cfg(target_os = "linux")]
fn desktop_entry_value(path: &Path) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "Desktop entry path is not valid UTF-8".to_string())?;
    if value.is_empty() || value.chars().any(char::is_control) {
        return Err("Desktop entry path contains control characters".into());
    }
    Ok(value.replace('\\', "\\\\"))
}

#[cfg(target_os = "linux")]
fn desktop_entry_exec(path: &Path, arguments: &str) -> Result<String, String> {
    let value = path
        .to_str()
        .ok_or_else(|| "Desktop launcher path is not valid UTF-8".to_string())?;
    if value.is_empty() || value.contains('=') || value.chars().any(char::is_control) {
        return Err("Desktop launcher path contains unsupported characters".into());
    }
    let escaped = value
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace('`', "\\`")
        .replace('$', "\\$")
        .replace('%', "%%");
    if arguments.trim().is_empty() {
        Ok(format!("\"{escaped}\""))
    } else {
        Ok(format!("\"{escaped}\" {arguments}"))
    }
}

#[cfg(target_os = "linux")]
fn remove_autostart_entry() -> Result<bool, String> {
    let path = autostart_entry_path()?;
    remove_user_file(&path).map_err(|error| error.to_string())
}

#[cfg(target_os = "linux")]
fn install_autostart() -> Result<(), String> {
    // The ordinary installer intentionally removes stale autostart. Recreate
    // it only after the user explicitly requests login startup.
    install()?;
    let entry = autostart_entry_path()?;
    let executable = install_bin_dir()?.join("crosshair");
    let exec = desktop_entry_exec(&executable, "--daemon --no-ui")?;
    let try_exec = desktop_entry_value(&executable)?;
    let content = format!(
        "[Desktop Entry]\nType=Application\nVersion=1.0\nName=Crosshair\nComment=Crosshair overlay\nTryExec={try_exec}\nExec={exec}\nTerminal=false\nCategories=Utility;\nStartupNotify=false\nX-KDE-autostart-after=panel\nNoDisplay=true\n"
    );
    atomic_write_user_file(&entry, content.as_bytes()).map_err(|error| error.to_string())?;
    println!("Optional login autostart enabled at {}.", entry.display());
    Ok(())
}

#[cfg(target_os = "linux")]
fn remove_autostart() -> Result<(), String> {
    let path = autostart_entry_path()?;
    if remove_user_file(&path).map_err(|error| error.to_string())? {
        println!("Login autostart removed. Crosshair remains available through Padfire.");
    } else {
        println!("No Crosshair login autostart entry was installed.");
    }
    Ok(())
}

fn install() -> Result<(), String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let settings = sibling_executable("crosshair-settings")?;
    let bin_dir = install_bin_dir()?;
    let target_cli = bin_dir.join(if cfg!(windows) {
        "crosshair.exe"
    } else {
        "crosshair"
    });
    let target_settings = bin_dir.join(if cfg!(windows) {
        "crosshair-settings.exe"
    } else {
        "crosshair-settings"
    });

    #[cfg(target_os = "linux")]
    {
        let current_bytes = read_install_source(&current)?;
        let settings_bytes = read_install_source(&settings)?;
        let app_entry = applications_entry_path()?;
        let app_icon = application_icon_path()?;
        let app_exec = desktop_entry_exec(&target_cli, "")?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Crosshair\nComment=Crosshair overlay and Settings\nIcon=crosshair\nExec={app_exec}\nTerminal=false\nCategories=Utility;\nStartupWMClass=crosshair\n"
        );

        validate_user_file_targets(&[&target_cli, &target_settings, &app_entry, &app_icon])
            .map_err(|error| format!("Install destinations are not safe: {error}"))?;
        // Only disable a previous explicit login startup after every install
        // destination has passed preflight and before replacing any artifact.
        remove_autostart_entry()?;
        atomic_write_user_executable(&target_cli, &current_bytes)
            .map_err(|error| format!("Could not install CLI: {error}"))?;
        atomic_write_user_executable(&target_settings, &settings_bytes)
            .map_err(|error| format!("Could not install Settings: {error}"))?;
        atomic_write_user_file(&app_entry, desktop.as_bytes())
            .map_err(|error| format!("Could not create app-menu entry: {error}"))?;
        atomic_write_user_file(&app_icon, APPLICATION_ICON_SVG)
            .map_err(|error| format!("Could not install the application icon: {error}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        std::fs::create_dir_all(&bin_dir)
            .map_err(|error| format!("Could not create {}: {error}", bin_dir.display()))?;
        let source_icon = current
            .parent()
            .unwrap_or(Path::new("."))
            .join("crosshair.ico");
        if !source_icon.is_file() {
            return Err(format!(
                "The Crosshair icon is missing: {}",
                source_icon.display()
            ));
        }
        std::fs::copy(&current, &target_cli)
            .map_err(|error| format!("Could not install CLI: {error}"))?;
        std::fs::copy(&settings, &target_settings)
            .map_err(|error| format!("Could not install Settings: {error}"))?;
        std::fs::copy(&source_icon, bin_dir.join("crosshair.ico"))
            .map_err(|error| format!("Could not install the Crosshair icon: {error}"))?;
    }

    println!("Crosshair installed to {}.", bin_dir.display());
    println!(
        "No login autostart or resident tray process was started. The selected launch mode is used when Crosshair is opened."
    );
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    let bin_dir = install_bin_dir()?;

    #[cfg(target_os = "linux")]
    let (cli, settings, autostart, app_entry, app_icon) = {
        let cli = bin_dir.join("crosshair");
        let settings = bin_dir.join("crosshair-settings");
        let autostart = autostart_entry_path()?;
        let app_entry = applications_entry_path()?;
        let app_icon = application_icon_path()?;
        validate_user_file_targets(&[&cli, &settings, &autostart, &app_entry, &app_icon])
            .map_err(|error| format!("Uninstall targets are not safe: {error}"))?;
        (cli, settings, autostart, app_entry, app_icon)
    };

    #[cfg(target_os = "windows")]
    let cleanup_script = {
        let cleanup_script = bin_dir.join("Uninstall-Crosshair.ps1");
        if !cleanup_script.is_file() {
            return Err(format!(
                "Windows cleanup script is missing: {}. Run Uninstall-Crosshair.ps1 from the original package.",
                cleanup_script.display()
            ));
        }
        cleanup_script
    };

    // Preflight every removal target before modifying either the live daemon
    // or retained user configuration. A refused uninstall must be side-effect free.
    let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    config.enabled = false;
    config
        .save_atomic(&paths.config_file)
        .map_err(|error| format!("Could not save the disabled state before uninstall: {error}"))?;
    match send_command(&paths, "hide", None) {
        Ok(_) => {}
        Err(error) if overlay_is_not_running(&error) => {}
        Err(error) => {
            return Err(format!(
                "Disabled state was saved, but Crosshair did not acknowledge shutdown before uninstall: {error}"
            ));
        }
    }

    #[cfg(target_os = "linux")]
    {
        remove_user_files(&[&cli, &settings, &autostart, &app_entry, &app_icon])
            .map_err(|error| format!("Could not safely uninstall Crosshair: {error}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        Command::new("powershell.exe")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-ExecutionPolicy",
                "Bypass",
                "-File",
            ])
            .arg(&cleanup_script)
            .arg("-WaitForProcessId")
            .arg(std::process::id().to_string())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            // Settings is normally launched with the install folder as its
            // working directory. The uninstaller must run elsewhere so it can
            // remove that folder after Settings and this CLI have exited.
            .current_dir(std::env::temp_dir())
            .spawn()
            .map_err(|error| {
                format!("Could not start the deferred Windows uninstaller: {error}")
            })?;
        println!(
            "Windows uninstall started. Close Settings to complete file removal; your configuration will be retained."
        );
        return Ok(());
    }

    #[cfg(target_os = "linux")]
    {
        println!(
            "Crosshair executables and app-menu entry removed. Your configuration was retained."
        );
        Ok(())
    }

    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        Err("Uninstallation is not implemented for this platform.".into())
    }
}

fn print_help() {
    let help = format!(
        r#"Crosshair {APP_VERSION} — native crosshair overlay

USAGE:
    crosshair [COMMAND]

Run without a command to use the saved App Activity preference:
    App UI, System tray (Windows), or Command only.

Explicit commands remain available in every mode.

COMMANDS:
    --config                Open Settings, regardless of the launch preference
    --tray                  Start the Windows system tray controller
    --toggle                Show if stopped; otherwise toggle visibility
    --show                  Show the overlay
    --hide                  Hide the overlay
    --set <JSON>            Validate and persist a partial config update
    --reload                Reload config.json from disk
    --restart               Restart the overlay daemon
    --quit                  Stop the overlay daemon
    --status                Show daemon and overlay status
    --doctor                Show platform and session diagnostics
    --list-monitors         Print monitor descriptors as JSON
    --export-code           Print the current shareable profile code
    --import-code <CODE>    Import a profile code
    --open-config-folder    Open the user configuration folder
    --install               Install the native binaries and app-menu entry
    --autostart             Enable optional login startup
    --remove-autostart      Disable login startup
    --uninstall             Remove installed binaries and launchers; keep config
    --version, -V           Print the application version
    --help, -h              Show this help
"#
    );
    print!("{help}");
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Help and version are intentionally filesystem-free. They must work even
    // before the user has created a config directory or installed the app.
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print_help();
        return Ok(());
    }
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Crosshair {APP_VERSION}");
        return Ok(());
    }

    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    if args.is_empty() {
        let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        match config.activity_mode.as_str() {
            "command" => return Ok(()),
            "tray" => {
                #[cfg(windows)]
                {
                    return crosshair_core::windows::run_tray();
                }
                #[cfg(not(windows))]
                {
                    eprintln!(
                        "System tray mode is only available on Windows; opening Settings instead."
                    );
                    return launch_settings();
                }
            }
            _ => return launch_settings(),
        }
    }
    if args.iter().any(|a| a == "--config") {
        return launch_settings();
    }
    if args.iter().any(|a| a == "--tray") {
        #[cfg(windows)]
        {
            return crosshair_core::windows::run_tray();
        }
        #[cfg(not(windows))]
        {
            return Err("System tray mode is only available on Windows.".into());
        }
    }
    if args.iter().any(|a| a == "--export-code") {
        let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        println!(
            "{}",
            config.export_code().map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--install") {
        return install();
    }
    if args.iter().any(|a| a == "--autostart") {
        #[cfg(target_os = "linux")]
        {
            return install_autostart();
        }
        #[cfg(target_os = "windows")]
        {
            let executable = install_bin_dir()?.join("crosshair.exe");
            if !executable.is_file() {
                return Err(format!(
                    "Install Crosshair before enabling login startup: {}",
                    executable.display()
                ));
            }
            crosshair_core::windows::install_autostart(&executable)?;
            println!("Optional Windows login startup enabled for the current user.");
            return Ok(());
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            return Err("Login autostart is not implemented for this platform yet.".into());
        }
    }
    if args.iter().any(|a| a == "--remove-autostart") {
        #[cfg(target_os = "linux")]
        {
            return remove_autostart();
        }
        #[cfg(target_os = "windows")]
        {
            let removed = crosshair_core::windows::remove_autostart()?;
            println!(
                "{}",
                if removed {
                    "Windows login startup removed."
                } else {
                    "No Windows Crosshair login startup entry was installed."
                }
            );
            return Ok(());
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            return Err("Login autostart is not implemented for this platform yet.".into());
        }
    }
    if args.iter().any(|a| a == "--uninstall") {
        return uninstall();
    }
    if let Some(index) = args.iter().position(|a| a == "--import-code") {
        let code = args
            .get(index + 1)
            .ok_or_else(|| "--import-code requires a profile code".to_string())?;
        return import_profile(&paths, code);
    }
    if args.iter().any(|a| a == "--list-monitors") {
        let monitors = discover_monitors()?;
        println!(
            "{}",
            serde_json::to_string(&monitors).map_err(|error| error.to_string())?
        );
        return Ok(());
    }
    if args.iter().any(|a| a == "--daemon") {
        let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        if args.iter().any(|a| a == "--show") {
            config.enabled = true;
            config
                .save_atomic(&paths.config_file)
                .map_err(|error| error.to_string())?;
        }
        if !config.enabled {
            return Ok(());
        }
        #[cfg(target_os = "linux")]
        {
            if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                return wayland::run(config, paths);
            }
            if std::env::var_os("DISPLAY").is_some() {
                return x11::run(config, paths);
            }
            return Err("No Wayland or X11 display session was detected.".into());
        }
        #[cfg(target_os = "windows")]
        {
            return crosshair_core::windows::run(config, paths);
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            return Err("This platform's overlay backend is not built yet.".into());
        }
    }
    if args.iter().any(|a| a == "--status") {
        return print_status(&paths);
    }
    if args.iter().any(|a| a == "--toggle") {
        return ensure_running_then(&paths, "toggle");
    }
    if args.iter().any(|a| a == "--show") {
        return ensure_running_then(&paths, "show");
    }
    if args.iter().any(|a| a == "--hide") {
        let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        config.enabled = false;
        config
            .save_atomic(&paths.config_file)
            .map_err(|error| error.to_string())?;
        match send_command(&paths, "hide", None) {
            Ok(_) => return Ok(()),
            Err(error) if overlay_is_not_running(&error) => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "Disabled state was saved, but the running overlay did not acknowledge shutdown: {error}"
                ));
            }
        }
    }
    if args.iter().any(|a| a == "--quit") {
        match send_command(&paths, "quit", None) {
            Ok(_) => return Ok(()),
            Err(error) if overlay_is_not_running(&error) => return Ok(()),
            Err(error) => return Err(format!("Could not stop Crosshair: {error}")),
        }
    }
    if args.iter().any(|a| a == "--restart") {
        return restart_daemon(&paths);
    }
    if args.iter().any(|a| a == "--reload") {
        match send_command(&paths, "reload", None) {
            Ok(_) => return Ok(()),
            Err(error) if overlay_is_not_running(&error) => {
                Config::load(&paths.config_file).map_err(|error| error.to_string())?;
                return Ok(());
            }
            Err(error) => return Err(format!("Could not reload the running overlay: {error}")),
        }
    }
    if args.iter().any(|a| a == "--doctor") {
        println!("Crosshair {APP_VERSION}");
        println!("OS: {}", std::env::consts::OS);
        println!(
            "Session: {}",
            std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into())
        );
        println!(
            "Wayland: {}",
            std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not active".into())
        );
        println!(
            "X11: {}",
            std::env::var("DISPLAY").unwrap_or_else(|_| "not active".into())
        );
        let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        print_status(&paths)?;
        print_monitor_diagnostics(&config);
        return Ok(());
    }
    if args.iter().any(|a| a == "--open-config-folder") {
        return open_folder(&paths);
    }
    if let Some(index) = args.iter().position(|a| a == "--set") {
        let json = args
            .get(index + 1)
            .ok_or_else(|| "--set requires a JSON argument".to_string())?;
        let patch: serde_json::Value =
            serde_json::from_str(json).map_err(|error| error.to_string())?;
        let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        if let Some(object) = patch.as_object() {
            let mut base = serde_json::to_value(&config).map_err(|error| error.to_string())?;
            if let Some(base_object) = base.as_object_mut() {
                for (key, value) in object {
                    base_object.insert(key.clone(), value.clone());
                }
            }
            config = Config::from_json(&base.to_string()).map_err(|error| error.to_string())?;
        } else {
            return Err("--set expects a JSON object".into());
        }
        config
            .save_atomic(&paths.config_file)
            .map_err(|error| error.to_string())?;
        match send_command(
            &paths,
            "set",
            Some(serde_json::to_value(&config).map_err(|error| error.to_string())?),
        ) {
            Ok(_) => return Ok(()),
            Err(error) if overlay_is_not_running(&error) => return Ok(()),
            Err(error) => {
                return Err(format!(
                    "Configuration was saved, but the running overlay did not acknowledge the update: {error}"
                ));
            }
        }
    }
    Err("Unknown command. Run crosshair --doctor or open Settings.".into())
}

#[cfg(test)]
mod tests {
    use super::overlay_is_not_running;

    #[test]
    fn only_an_explicit_missing_endpoint_allows_cold_start_or_noop_shutdown() {
        assert!(overlay_is_not_running("Crosshair overlay is not running"));
        assert!(overlay_is_not_running(
            "Crosshair overlay is not running: Connection refused"
        ));
        assert!(!overlay_is_not_running(
            "Invalid response from overlay: operation timed out"
        ));
        assert!(!overlay_is_not_running(
            "Could not reach the Crosshair named-pipe listener (Windows error 121)"
        ));
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Crosshair: {error}");
        std::process::exit(1);
    }
}
