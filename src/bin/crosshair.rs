#[cfg(target_os = "linux")]
#[path = "../platform/linux/wayland.rs"]
mod wayland;
#[cfg(target_os = "linux")]
#[path = "../platform/linux/x11.rs"]
mod x11;

use crosshair_core::{APP_VERSION, AppPaths, Config};
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
    time::Duration,
};

#[cfg(unix)]
fn send_command(
    paths: &AppPaths,
    command: &str,
    payload: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
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
    config
        .save_atomic(&paths.config_file)
        .map_err(|error| error.to_string())?;
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
        Err(_) => {
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
    }
    Ok(())
}

fn restart_daemon(paths: &AppPaths) -> Result<(), String> {
    let enabled = Config::load(&paths.config_file)
        .map_err(|error| error.to_string())?
        .enabled;
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
    config
        .import_code(code)
        .map_err(|error| error.to_string())?;
    config
        .save_atomic(&paths.config_file)
        .map_err(|error| error.to_string())?;
    if send_command(
        paths,
        "set",
        Some(serde_json::to_value(&config).map_err(|error| error.to_string())?),
    )
    .is_ok()
    {
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
    Ok(format!("\"{escaped}\" {arguments}"))
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
        let app_exec = desktop_entry_exec(&target_cli, "--config")?;
        let desktop = format!(
            "[Desktop Entry]\nType=Application\nName=Crosshair\nComment=Configure the Crosshair overlay\nExec={app_exec}\nTerminal=false\nCategories=Utility;\nStartupWMClass=crosshair\n"
        );

        validate_user_file_targets(&[&target_cli, &target_settings, &app_entry])
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
    }

    #[cfg(target_os = "windows")]
    {
        std::fs::create_dir_all(&bin_dir)
            .map_err(|error| format!("Could not create {}: {error}", bin_dir.display()))?;
        std::fs::copy(&current, &target_cli)
            .map_err(|error| format!("Could not install CLI: {error}"))?;
        std::fs::copy(&settings, &target_settings)
            .map_err(|error| format!("Could not install Settings: {error}"))?;
    }

    println!("Crosshair installed to {}.", bin_dir.display());
    println!(
        "No autostart or system-tray process was installed. Settings and overlay are started on demand."
    );
    Ok(())
}

fn uninstall() -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    let _ = send_command(&paths, "quit", None);
    let bin_dir = install_bin_dir()?;

    #[cfg(target_os = "linux")]
    {
        let cli = bin_dir.join("crosshair");
        let settings = bin_dir.join("crosshair-settings");
        let autostart = autostart_entry_path()?;
        let app_entry = applications_entry_path()?;
        remove_user_files(&[&cli, &settings, &autostart, &app_entry])
            .map_err(|error| format!("Could not safely uninstall Crosshair: {error}"))?;
    }

    #[cfg(target_os = "windows")]
    {
        let cleanup_script = bin_dir.join("Uninstall-Crosshair.ps1");
        if !cleanup_script.is_file() {
            return Err(format!(
                "Windows cleanup script is missing: {}. Run Uninstall-Crosshair.ps1 from the original package.",
                cleanup_script.display()
            ));
        }
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
        #[cfg(target_os = "linux")]
        {
            let monitors = if std::env::var_os("WAYLAND_DISPLAY").is_some() {
                wayland::list_outputs()?
            } else if std::env::var_os("DISPLAY").is_some() {
                x11::list_outputs()?
            } else {
                return Err("No Wayland or X11 display session was detected.".into());
            };
            println!(
                "{}",
                serde_json::to_string(&monitors).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        #[cfg(target_os = "windows")]
        {
            let monitors = crosshair_core::windows::list_monitors()?;
            println!(
                "{}",
                serde_json::to_string(&monitors).map_err(|error| error.to_string())?
            );
            return Ok(());
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            return Err("Monitor enumeration is not implemented for this platform yet.".into());
        }
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
        if send_command(&paths, "hide", None).is_err() {
            let mut config = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
            config.enabled = false;
            config
                .save_atomic(&paths.config_file)
                .map_err(|error| error.to_string())?;
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
        print_status(&paths)?;
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
        let _ = send_command(
            &paths,
            "set",
            Some(serde_json::to_value(&config).map_err(|error| error.to_string())?),
        );
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
