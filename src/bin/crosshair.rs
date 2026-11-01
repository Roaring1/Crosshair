#[cfg(target_os = "linux")]
#[path = "../platform/linux/wayland.rs"]
mod wayland;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/x11.rs"]
mod x11;

use crosshair_core::{AppPaths, Config, APP_VERSION};
use std::{
    io::{BufRead, BufReader, Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    thread,
    time::Duration,
};

#[cfg(unix)]
fn send_command(paths: &AppPaths, command: &str, payload: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    use std::os::unix::net::UnixStream;
    let mut stream = UnixStream::connect(&paths.socket_file)
        .map_err(|error| format!("Crosshair overlay is not running: {error}"))?;
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let mut request = serde_json::json!({"cmd": command});
    if let Some(payload) = payload {
        request["payload"] = payload;
    }
    let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() >= 64 * 1024 {
        return Err("Control request exceeds 64 KiB".into());
    }
    bytes.push(b'\n');
    stream.write_all(&bytes).map_err(|error| error.to_string())?;
    let reader = BufReader::new(stream);
    let mut response = Vec::with_capacity(512);
    reader.take(64 * 1024).read_until(b'\n', &mut response)
        .map_err(|error| error.to_string())?;
    let response: serde_json::Value =
        serde_json::from_slice(&response).map_err(|error| format!("Invalid response from overlay: {error}"))?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(response.get("message").and_then(serde_json::Value::as_str)
            .unwrap_or("Overlay rejected command").to_string());
    }
    Ok(response)
}

#[cfg(not(unix))]
fn send_command(_: &AppPaths, _: &str, _: Option<serde_json::Value>) -> Result<serde_json::Value, String> {
    Err("This platform's native IPC backend is not implemented yet".into())
}

fn sibling_executable(name: &str) -> Result<PathBuf, String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let path = current.parent().unwrap_or(Path::new(".")).join(if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    });
    if path.is_file() {
        Ok(path)
    } else {
        Err(format!("Required Crosshair executable was not found: {}", path.display()))
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

fn spawn_daemon() -> Result<(), String> {
    let executable = std::env::current_exe().map_err(|error| error.to_string())?;
    Command::new(executable)
        .arg("--daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start Crosshair: {error}"))?;
    Ok(())
}

fn start_enabled(paths: &AppPaths) -> Result<(), String> {
    let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    config.enabled = true;
    config.save_atomic(&paths.config_file).map_err(|error| error.to_string())?;
    spawn_daemon()?;
    for _ in 0..80 {
        thread::sleep(Duration::from_millis(25));
        if let Ok(response) = send_command(paths, "status", None) {
            if response.get("enabled").and_then(serde_json::Value::as_bool) == Some(true) {
                return Ok(());
            }
        }
    }
    Err("Crosshair did not become ready. Run crosshair --doctor for diagnostics.".into())
}

fn ensure_running_then(paths: &AppPaths, command: &str) -> Result<(), String> {
    match send_command(paths, command, None) {
        Ok(_) => Ok(()),
        // A cold toggle is a show operation by definition: there is no live
        // state to invert. Starting and then sending "toggle" would turn it OFF.
        Err(_) => start_enabled(paths),
    }
}

fn print_status(paths: &AppPaths) -> Result<(), String> {
    let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    match send_command(paths, "status", None) {
        Ok(response) => {
            println!("Crosshair daemon: running");
            println!("Overlay: {}", if response["enabled"].as_bool() == Some(true) { "enabled" } else { "disabled" });
            if let Some(monitors) = response["monitors"].as_array() {
                for monitor in monitors {
                    if let Some(name) = monitor.as_str() {
                        println!("Output: {name}");
                    }
                }
            }
        }
        Err(_) => {
            println!("Crosshair daemon: not running");
            println!("Overlay setting: {}", if config.enabled { "enabled (will show on next start)" } else { "disabled" });
        }
    }
    Ok(())
}

fn restart_daemon(paths: &AppPaths) -> Result<(), String> {
    let enabled = Config::load(&paths.config_file).map_err(|error| error.to_string())?.enabled;
    if send_command(paths, "quit", None).is_ok() {
        for _ in 0..120 {
            if send_command(paths, "status", None).is_err() {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }
    }
    if enabled {
        start_enabled(paths)?;
    }
    Ok(())
}

fn import_profile(paths: &AppPaths, code: &str) -> Result<(), String> {
    let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    config.import_code(code).map_err(|error| error.to_string())?;
    config.save_atomic(&paths.config_file).map_err(|error| error.to_string())?;
    if send_command(paths, "set", Some(serde_json::to_value(&config).map_err(|error| error.to_string())?)).is_ok() {
        println!("Crosshair profile imported and applied.");
    } else {
        println!("Crosshair profile imported. It will be used the next time the overlay is shown.");
    }
    Ok(())
}

fn open_folder(paths: &AppPaths) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let (program, argument) = ("explorer.exe", paths.config_dir.as_os_str().to_owned());
    #[cfg(target_os = "linux")]
    let (program, argument) = ("xdg-open", paths.config_dir.as_os_str().to_owned());
    Command::new(program).arg(argument).spawn()
        .map_err(|error| format!("Could not open config folder: {error}"))?;
    Ok(())
}

fn install() -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let settings = sibling_executable("crosshair-settings")?;
    #[cfg(target_os = "linux")]
    let bin_dir = {
        let home = std::env::var_os("HOME").map(PathBuf::from)
            .ok_or_else(|| "HOME is not set".to_string())?;
        std::env::var_os("XDG_BIN_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/bin"))
    };
    #[cfg(target_os = "windows")]
    let bin_dir = {
        let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
            .ok_or_else(|| "LOCALAPPDATA is not set".to_string())?;
        local.join("Programs/Crosshair")
    };
    std::fs::create_dir_all(&bin_dir).map_err(|error| format!("Could not create {}: {error}", bin_dir.display()))?;
    let target_cli = bin_dir.join(if cfg!(windows) { "crosshair.exe" } else { "crosshair" });
    let target_settings = bin_dir.join(if cfg!(windows) { "crosshair-settings.exe" } else { "crosshair-settings" });
    std::fs::copy(&current, &target_cli).map_err(|error| format!("Could not install CLI: {error}"))?;
    std::fs::copy(&settings, &target_settings).map_err(|error| format!("Could not install Settings: {error}"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&target_cli, std::fs::Permissions::from_mode(0o755)).ok();
        std::fs::set_permissions(&target_settings, std::fs::Permissions::from_mode(0o755)).ok();
    }
    #[cfg(target_os = "linux")]
    {
        let applications = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .filter(|p| p.is_absolute()).unwrap_or_else(|| {
                std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/share")
            }).join("applications");
        std::fs::create_dir_all(&applications).map_err(|error| error.to_string())?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Crosshair\nComment=Configure the Crosshair overlay\nExec={} --config\nTerminal=false\nCategories=Utility;\nStartupWMClass=crosshair\n",
            target_cli.display()
        );
        std::fs::write(applications.join("crosshair.desktop"), desktop)
            .map_err(|error| format!("Could not create app-menu entry: {error}"))?;
    }
    println!("Crosshair installed to {}.", bin_dir.display());
    println!("No autostart or system-tray process was installed. Settings and overlay are started on demand.");
    let _ = paths;
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    let _ = send_command(&paths, "quit", None);
    #[cfg(target_os = "linux")]
    let bin_dir = {
        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        std::env::var_os("XDG_BIN_HOME").map(PathBuf::from).filter(|p| p.is_absolute())
            .unwrap_or_else(|| home.join(".local/bin"))
    };
    #[cfg(target_os = "windows")]
    let bin_dir = std::env::var_os("LOCALAPPDATA").map(PathBuf::from)
        .unwrap_or_default().join("Programs/Crosshair");
    let _ = std::fs::remove_file(bin_dir.join(if cfg!(windows) { "crosshair.exe" } else { "crosshair" }));
    let _ = std::fs::remove_file(bin_dir.join(if cfg!(windows) { "crosshair-settings.exe" } else { "crosshair-settings" }));
    #[cfg(target_os = "linux")]
    {
        let applications = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
            .filter(|p| p.is_absolute()).unwrap_or_else(|| {
                std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default().join(".local/share")
            }).join("applications");
        let _ = std::fs::remove_file(applications.join("crosshair.desktop"));
    }
    println!("Crosshair executables and app-menu entry removed. Your configuration was retained.");
    Ok(())
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;

    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("Crosshair {APP_VERSION}");
        return Ok(());
    }
    if args.is_empty() || args.iter().any(|a| a == "--config") {
        return launch_settings();
    }
    if args.iter().any(|a| a == "--export-code") {
        let config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        println!("{}", config.export_code().map_err(|error| error.to_string())?);
        return Ok(());
    }
    if args.iter().any(|a| a == "--install") {
        return install();
    }
    if args.iter().any(|a| a == "--uninstall") {
        return uninstall();
    }
    if let Some(index) = args.iter().position(|a| a == "--import-code") {
        let code = args.get(index + 1).ok_or_else(|| "--import-code requires a profile code".to_string())?;
        return import_profile(&paths, code);
    }
    if args.iter().any(|a| a == "--list-monitors") {
        #[cfg(target_os = "linux")]
        {
            let monitors = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                wayland::list_outputs()?
            } else if std::env::var_os("DISPLAY").is_some() {
                x11::list_outputs()?
            } else {
                return Err("No Wayland or X11 display session was detected.".into());
            };
            println!("{}", serde_json::to_string(&monitors).map_err(|error| error.to_string())?);
            return Ok(());
        }
        #[cfg(not(target_os = "linux"))]
        {
            return Err("Monitor enumeration is not implemented for this platform yet.".into());
        }
    }
    if args.iter().any(|a| a == "--daemon") {
        let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        if args.iter().any(|a| a == "--show") {
            config.enabled = true;
            config.save_atomic(&paths.config_file).map_err(|error| error.to_string())?;
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
        #[cfg(not(target_os = "linux"))]
        {
            Err("This platform's overlay backend is not built yet.".into())
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
        if send_command(&paths, "hide", None).is_err() {
            let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
            config.enabled = false;
            config.save_atomic(&paths.config_file).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--quit") {
        let _ = send_command(&paths, "quit", None);
        return Ok(());
    }
    if args.iter().any(|a| a == "--restart") {
        return restart_daemon(&paths);
    }
    if args.iter().any(|a| a == "--reload") {
        if send_command(&paths, "reload", None).is_err() {
            Config::load(&paths.config_file).map_err(|error| error.to_string())?;
        }
        return Ok(());
    }
    if args.iter().any(|a| a == "--doctor") {
        println!("Crosshair {APP_VERSION}");
        println!("OS: {}", std::env::consts::OS);
        println!("Session: {}", std::env::var("XDG_SESSION_TYPE").unwrap_or_else(|_| "unknown".into()));
        println!("Wayland: {}", std::env::var("WAYLAND_DISPLAY").unwrap_or_else(|_| "not active".into()));
        println!("X11: {}", std::env::var("DISPLAY").unwrap_or_else(|_| "not active".into()));
        print_status(&paths)?;
        return Ok(());
    }
    if args.iter().any(|a| a == "--open-config-folder") {
        return open_folder(&paths);
    }
    if let Some(index) = args.iter().position(|a| a == "--set") {
        let json = args.get(index + 1).ok_or_else(|| "--set requires a JSON argument".to_string())?;
        let patch: serde_json::Value = serde_json::from_str(json).map_err(|error| error.to_string())?;
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
        config.save_atomic(&paths.config_file).map_err(|error| error.to_string())?;
        let _ = send_command(&paths, "set", Some(serde_json::to_value(&config).map_err(|error| error.to_string())?));
        return Ok(());
    }
    Err("Unknown command. Run crosshair --doctor or open Settings.".into())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Crosshair: {error}");
        std::process::exit(1);
    }
}
