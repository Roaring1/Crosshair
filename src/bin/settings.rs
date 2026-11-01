use crosshair_core::render::{Raster, render_crosshair};
use crosshair_core::{AppPaths, Config, MonitorDescriptor};
use slint::{
    ComponentHandle, Image, ModelRc, Rgba8Pixel, SharedPixelBuffer, SharedString, Timer, TimerMode,
    VecModel,
};
use std::cell::{Cell, RefCell};
use std::path::Path;
use std::process::Command;
use std::rc::Rc;
#[cfg(unix)]
use std::thread;
use std::time::Duration;

slint::include_modules!();

const PREVIEW_SIZE: u32 = 240;
const PREVIEW_SAFE_MARGIN: u32 = 12;

fn parse_color(value: &str) -> Option<[u8; 3]> {
    let clean = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if clean.len() != 6 || !clean.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    Some([
        u8::from_str_radix(&clean[0..2], 16).ok()?,
        u8::from_str_radix(&clean[2..4], 16).ok()?,
        u8::from_str_radix(&clean[4..6], 16).ok()?,
    ])
}

fn canonical_color(value: &str, current: &str) -> String {
    parse_color(value)
        .map(|[r, g, b]| format!("#{r:02x}{g:02x}{b:02x}"))
        .unwrap_or_else(|| current.to_string())
}

fn slint_color(hex: &str) -> slint::Color {
    let [r, g, b] = parse_color(hex).unwrap_or([34, 34, 34]);
    slint::Color::from_rgb_u8(r, g, b)
}

fn monitor_descriptor_label(monitor: &MonitorDescriptor, all: &[MonitorDescriptor]) -> String {
    let duplicates = all.iter().filter(|item| item.name == monitor.name).count();
    if duplicates > 1 {
        format!(
            "Monitor: {} ({})",
            monitor.name,
            monitor.ordinal.saturating_add(1)
        )
    } else {
        format!("Monitor: {}", monitor.name)
    }
}

fn monitor_label(config: &Config, monitors: &[MonitorDescriptor]) -> String {
    match config.monitor_mode.as_str() {
        "primary" => "Primary monitor".into(),
        "active" => "Cursor-active monitor".into(),
        "name" if !config.monitor_name.is_empty() => {
            let descriptor = MonitorDescriptor {
                name: config.monitor_name.clone(),
                ordinal: config.monitor_name_ordinal,
            };
            monitor_descriptor_label(&descriptor, monitors)
        }
        "index" => format!("Monitor index: {}", config.monitor_index.saturating_add(1)),
        _ => "All monitors".into(),
    }
}

fn monitor_options(config: &Config, monitors: &[MonitorDescriptor]) -> Vec<String> {
    let mut options = vec![
        "All monitors".to_string(),
        "Primary monitor".to_string(),
        "Cursor-active monitor".to_string(),
    ];
    for monitor in monitors {
        options.push(monitor_descriptor_label(monitor, monitors));
    }
    if config.monitor_mode == "name" && !config.monitor_name.is_empty() {
        let current = monitor_label(config, monitors);
        if !options.contains(&current) {
            options.push(current);
        }
    }
    if config.monitor_mode == "index" {
        let current = format!("Monitor index: {}", config.monitor_index.saturating_add(1));
        if !options.contains(&current) {
            options.push(current);
        }
    }
    options
}

fn discover_monitors() -> Vec<MonitorDescriptor> {
    let executable = match std::env::current_exe() {
        Ok(path) => path
            .parent()
            .unwrap_or(Path::new("."))
            .join(if cfg!(windows) {
                "crosshair.exe"
            } else {
                "crosshair"
            }),
        Err(_) => return Vec::new(),
    };
    let output = match Command::new(executable).arg("--list-monitors").output() {
        Ok(output) if output.status.success() => output,
        _ => return Vec::new(),
    };
    serde_json::from_slice(&output.stdout).unwrap_or_default()
}

fn read_ui(
    ui: &SettingsWindow,
    current: &Config,
    monitors: &[MonitorDescriptor],
) -> Result<Config, String> {
    let mut next = current.clone();
    next.enabled = ui.get_enabled();
    next.auto_save = ui.get_auto_save();
    next.shape = ui.get_shape().to_string();
    next.opacity = ui.get_crosshair_opacity();
    next.size = ui.get_size().clamp(1, 200) as u16;
    next.thickness = ui.get_thickness().clamp(1, 100) as u16;
    next.gap = ui.get_gap().clamp(0, 200) as u16;
    next.color = canonical_color(&ui.get_color_text(), &current.color);

    next.outline.enabled = ui.get_outline_enabled();
    next.outline.color = canonical_color(&ui.get_outline_color_text(), &current.outline.color);
    next.outline.opacity = ui.get_outline_opacity();
    next.outline.thickness = ui.get_outline_thickness().clamp(0, 100) as u16;

    next.shadow.enabled = ui.get_shadow_enabled();
    next.shadow.color = canonical_color(&ui.get_shadow_color_text(), &current.shadow.color);
    next.shadow.opacity = ui.get_shadow_opacity();
    next.shadow.offset_x = ui.get_shadow_x().clamp(-200, 200) as i16;
    next.shadow.offset_y = ui.get_shadow_y().clamp(-200, 200) as i16;

    next.reference_dot.enabled = ui.get_reference_enabled();
    next.reference_dot.color =
        canonical_color(&ui.get_reference_color_text(), &current.reference_dot.color);
    next.reference_dot.opacity = ui.get_reference_opacity();
    next.reference_dot.size = ui.get_reference_size().clamp(1, 100) as u16;
    next.reference_dot.offset_x = ui.get_reference_x().clamp(-500, 500) as i16;
    next.reference_dot.offset_y = ui.get_reference_y().clamp(-500, 500) as i16;

    next.offset_x = ui.get_offset_x().clamp(-500, 500) as i16;
    next.offset_y = ui.get_offset_y().clamp(-500, 500) as i16;
    next.preview_bg = canonical_color(&ui.get_preview_background_text(), &current.preview_bg);

    let selection = ui.get_monitor_selection().to_string();
    match selection.as_str() {
        "Primary monitor" => next.monitor_mode = "primary".into(),
        "Cursor-active monitor" => next.monitor_mode = "active".into(),
        "All monitors" => {
            next.monitor_mode = "all".into();
            next.monitor_name.clear();
            next.monitor_name_ordinal = 0;
        }
        label if label.starts_with("Monitor: ") => {
            if let Some(monitor) = monitors
                .iter()
                .find(|monitor| monitor_descriptor_label(monitor, monitors) == label)
            {
                next.monitor_mode = "name".into();
                next.monitor_name = monitor.name.clone();
                next.monitor_name_ordinal = monitor.ordinal;
            } else if current.monitor_mode == "name" && monitor_label(current, monitors) == label {
                // Preserve a saved selection that is currently disconnected.
                next.monitor_mode = "name".into();
                next.monitor_name = current.monitor_name.clone();
                next.monitor_name_ordinal = current.monitor_name_ordinal;
            }
        }
        label if label.starts_with("Monitor index: ") => {
            if let Ok(index) = label.trim_start_matches("Monitor index: ").parse::<u16>() {
                next.monitor_mode = "index".into();
                next.monitor_index = index.saturating_sub(1).min(255);
            }
        }
        _ => {}
    }
    next.normalize();
    Ok(next)
}

fn set_monitor_ui(ui: &SettingsWindow, config: &Config, monitors: &[MonitorDescriptor]) {
    ui.set_monitor_selection(SharedString::from(monitor_label(config, monitors)));
    ui.set_monitor_options(ModelRc::new(VecModel::from(
        monitor_options(config, monitors)
            .into_iter()
            .map(SharedString::from)
            .collect::<Vec<_>>(),
    )));
}

fn set_ui(ui: &SettingsWindow, config: &Config, monitors: &[MonitorDescriptor]) {
    ui.set_enabled(config.enabled);
    ui.set_auto_save(config.auto_save);
    ui.set_shape(SharedString::from(config.shape.as_str()));
    ui.set_crosshair_opacity(config.opacity);
    ui.set_size(config.size as i32);
    ui.set_thickness(config.thickness as i32);
    ui.set_gap(config.gap as i32);
    ui.set_color_text(SharedString::from(config.color.as_str()));
    ui.set_main_swatch(slint_color(&config.color));

    ui.set_outline_enabled(config.outline.enabled);
    ui.set_outline_color_text(SharedString::from(config.outline.color.as_str()));
    ui.set_outline_swatch(slint_color(&config.outline.color));
    ui.set_outline_opacity(config.outline.opacity);
    ui.set_outline_thickness(config.outline.thickness as i32);

    ui.set_shadow_enabled(config.shadow.enabled);
    ui.set_shadow_color_text(SharedString::from(config.shadow.color.as_str()));
    ui.set_shadow_swatch(slint_color(&config.shadow.color));
    ui.set_shadow_opacity(config.shadow.opacity);
    ui.set_shadow_x(config.shadow.offset_x as i32);
    ui.set_shadow_y(config.shadow.offset_y as i32);

    ui.set_reference_enabled(config.reference_dot.enabled);
    ui.set_reference_color_text(SharedString::from(config.reference_dot.color.as_str()));
    ui.set_reference_swatch(slint_color(&config.reference_dot.color));
    ui.set_reference_opacity(config.reference_dot.opacity);
    ui.set_reference_size(config.reference_dot.size as i32);
    ui.set_reference_x(config.reference_dot.offset_x as i32);
    ui.set_reference_y(config.reference_dot.offset_y as i32);

    ui.set_offset_x(config.offset_x as i32);
    ui.set_offset_y(config.offset_y as i32);
    set_monitor_ui(ui, config, monitors);
    ui.set_preview_background(slint_color(&config.preview_bg));
    ui.set_preview_background_text(SharedString::from(config.preview_bg.as_str()));
    update_preview(ui, config);
}

fn preview_pixels(config: &Config) -> Option<(Vec<u8>, u32, u32)> {
    let background = parse_color(&config.preview_bg).unwrap_or([34, 34, 34]);
    let mut output = vec![0u8; (PREVIEW_SIZE * PREVIEW_SIZE * 4) as usize];
    for pixel in output.chunks_exact_mut(4) {
        pixel.copy_from_slice(&[background[0], background[1], background[2], 255]);
    }

    // The preview describes appearance, not runtime visibility. Disabling the
    // overlay must not make the user's editing preview disappear.
    let mut preview_config = config.clone();
    preview_config.enabled = true;
    let raster = render_crosshair(&preview_config).ok()?;
    composite_raster(&mut output, PREVIEW_SIZE, &raster);
    Some((output, PREVIEW_SIZE, PREVIEW_SIZE))
}

fn composite_raster(destination: &mut [u8], destination_size: u32, raster: &Raster) {
    let max_source = raster.width.max(raster.height).max(1);
    let max_allowed = PREVIEW_SIZE.saturating_sub(PREVIEW_SAFE_MARGIN * 2);
    let scale = (max_allowed as f32 / max_source as f32).min(1.0);
    let draw_width = ((raster.width as f32 * scale).round() as u32).max(1);
    let draw_height = ((raster.height as f32 * scale).round() as u32).max(1);
    let origin_x = ((destination_size as i32 - draw_width as i32) / 2)
        + (raster.origin_x as f32 * scale).round() as i32;
    let origin_y = ((destination_size as i32 - draw_height as i32) / 2)
        + (raster.origin_y as f32 * scale).round() as i32;

    for y in 0..draw_height {
        let target_y = origin_y + y as i32;
        if !(0..destination_size as i32).contains(&target_y) {
            continue;
        }
        let source_y = ((y as f32 / scale).floor() as u32).min(raster.height - 1);
        for x in 0..draw_width {
            let target_x = origin_x + x as i32;
            if !(0..destination_size as i32).contains(&target_x) {
                continue;
            }
            let source_x = ((x as f32 / scale).floor() as u32).min(raster.width - 1);
            let src = ((source_y * raster.width + source_x) * 4) as usize;
            let dst = ((target_y as u32 * destination_size + target_x as u32) * 4) as usize;
            let b = raster.bgra_premultiplied[src] as u32;
            let g = raster.bgra_premultiplied[src + 1] as u32;
            let r = raster.bgra_premultiplied[src + 2] as u32;
            let a = raster.bgra_premultiplied[src + 3] as u32;
            let remain = 255 - a;
            destination[dst] = (r + (destination[dst] as u32 * remain + 127) / 255).min(255) as u8;
            destination[dst + 1] =
                (g + (destination[dst + 1] as u32 * remain + 127) / 255).min(255) as u8;
            destination[dst + 2] =
                (b + (destination[dst + 2] as u32 * remain + 127) / 255).min(255) as u8;
            destination[dst + 3] = 255;
        }
    }
}

fn update_preview(ui: &SettingsWindow, config: &Config) {
    if let Some((bytes, width, height)) = preview_pixels(config) {
        let pixels = SharedPixelBuffer::<Rgba8Pixel>::clone_from_slice(&bytes, width, height);
        ui.set_preview_image(Image::from_rgba8_premultiplied(pixels));
    }
}

#[cfg(unix)]
fn notify_overlay(config: &Config, paths: &AppPaths) -> Result<(), String> {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::net::UnixStream;

    let mut connected = UnixStream::connect(&paths.socket_file);
    if connected.is_err() && config.enabled {
        start_overlay(paths, true)?;
        for _ in 0..80 {
            thread::sleep(Duration::from_millis(25));
            connected = UnixStream::connect(&paths.socket_file);
            if connected.is_ok() {
                break;
            }
        }
    }
    let mut stream = match connected {
        Ok(stream) => stream,
        Err(error)
            if !config.enabled
                && (error.kind() == std::io::ErrorKind::NotFound
                    || error.kind() == std::io::ErrorKind::ConnectionRefused) =>
        {
            return Ok(());
        }
        Err(error) => return Err(format!("Could not contact overlay: {error}")),
    };
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let request = serde_json::json!({
        "cmd": "preview",
        "payload": serde_json::to_value(config).map_err(|error| error.to_string())?,
    });
    let mut encoded = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if encoded.len() > 64 * 1024 {
        return Err("Settings update exceeds the IPC message limit".into());
    }
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| error.to_string())?;
    let reader = BufReader::new(stream);
    let mut line = Vec::with_capacity(256);
    reader
        .take(64 * 1024)
        .read_until(b'\n', &mut line)
        .map_err(|error| error.to_string())?;
    let response: serde_json::Value =
        serde_json::from_slice(&line).map_err(|error| error.to_string())?;
    if response.get("ok").and_then(serde_json::Value::as_bool) != Some(true) {
        return Err(response
            .get("message")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("Overlay rejected the settings update")
            .to_string());
    }
    Ok(())
}

#[cfg(windows)]
fn notify_overlay(config: &Config, paths: &AppPaths) -> Result<(), String> {
    let _ = (config, paths);
    Err("Windows named-pipe control is not yet enabled in this build".into())
}

#[cfg(unix)]
fn start_overlay(paths: &AppPaths, show: bool) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|error| error.to_string())?;
    let name = if cfg!(windows) {
        "crosshair.exe"
    } else {
        "crosshair"
    };
    let executable = current.parent().unwrap_or(Path::new(".")).join(name);
    if !executable.is_file() {
        return Err(format!(
            "Overlay executable was not found beside Settings: {}",
            executable.display()
        ));
    }
    let mut command = Command::new(executable);
    if show {
        command.arg("--show");
    } else {
        command.arg("--daemon");
    }
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("Could not start overlay: {error}"))?;
    let _ = paths;
    Ok(())
}

fn open_config_directory(paths: &AppPaths) -> Result<(), String> {
    #[cfg(target_os = "windows")]
    let mut command = {
        let mut c = Command::new("explorer.exe");
        c.arg(&paths.config_dir);
        c
    };
    #[cfg(target_os = "linux")]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(&paths.config_dir);
        c
    };
    #[cfg(target_os = "macos")]
    let mut command = {
        let mut c = Command::new("open");
        c.arg(&paths.config_dir);
        c
    };
    command
        .spawn()
        .map_err(|error| format!("Could not open config folder: {error}"))?;
    Ok(())
}

fn apply_candidate(
    ui: &SettingsWindow,
    config: &Rc<RefCell<Config>>,
    paths: &AppPaths,
    candidate: Config,
    persist: bool,
    feedback: Option<&str>,
    monitors: &[MonitorDescriptor],
) -> Result<(), String> {
    notify_overlay(&candidate, paths).map_err(|error| {
        ui.set_profile_feedback(SharedString::from(error.clone()));
        error
    })?;
    if persist {
        candidate
            .save_atomic(&paths.config_file)
            .map_err(|error| error.to_string())?;
    }
    *config.borrow_mut() = candidate.clone();
    set_ui(ui, &candidate, monitors);
    if let Some(message) = feedback {
        ui.set_profile_feedback(SharedString::from(message));
    }
    Ok(())
}

fn run() -> Result<(), String> {
    let paths = AppPaths::discover().map_err(|error| error.to_string())?;
    let mut initial = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
    initial.normalize();
    let ui = SettingsWindow::new().map_err(|error| error.to_string())?;
    let config = Rc::new(RefCell::new(initial.clone()));
    let last_auto_save = Rc::new(Cell::new(initial.auto_save));
    let monitors = Rc::new(RefCell::new(discover_monitors()));
    set_ui(&ui, &initial, &monitors.borrow());
    if initial.enabled {
        if let Err(error) = notify_overlay(&initial, &paths) {
            // Keep Settings open so the user can choose another display mode
            // instead of making a stale/unsupported saved choice uneditable.
            eprintln!("Could not show the saved overlay configuration: {error}");
        }
    }

    let weak = ui.as_weak();
    let timer = Rc::new(Timer::default());
    {
        let weak = weak.clone();
        let timer = timer.clone();
        let config = config.clone();
        let last_auto_save = last_auto_save.clone();
        let paths = paths.clone();
        let monitors_for_settings = monitors.clone();
        ui.on_settings_changed(move || {
            let weak = weak.clone();
            let config = config.clone();
            let last_auto_save = last_auto_save.clone();
            let paths = paths.clone();
            let monitors = monitors_for_settings.clone();
            timer.start(
                TimerMode::SingleShot,
                Duration::from_millis(75),
                move || {
                    let Some(ui) = weak.upgrade() else { return };
                    let old = config.borrow().clone();
                    let Ok(candidate) = read_ui(&ui, &old, &monitors.borrow()) else {
                        return;
                    };
                    let autosave_before = last_auto_save.get();
                    last_auto_save.set(candidate.auto_save);
                    let should_persist =
                        candidate.auto_save || (autosave_before && !candidate.auto_save);
                    update_preview(&ui, &candidate);
                    if let Err(error) = notify_overlay(&candidate, &paths) {
                        ui.set_profile_feedback(SharedString::from(error));
                        return;
                    }
                    if should_persist {
                        if let Err(error) = candidate.save_atomic(&paths.config_file) {
                            ui.set_profile_feedback(SharedString::from(format!(
                                "Save failed: {error}"
                            )));
                        }
                    }
                    *config.borrow_mut() = candidate;
                },
            );
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let paths = paths.clone();
        let monitors = monitors.clone();
        ui.on_save_requested(move || {
            let Some(ui) = weak.upgrade() else { return };
            let old = config.borrow().clone();
            match read_ui(&ui, &old, &monitors.borrow()) {
                Ok(candidate) => {
                    if let Err(error) = notify_overlay(&candidate, &paths) {
                        ui.set_profile_feedback(SharedString::from(error));
                        return;
                    }
                    if let Err(error) = candidate.save_atomic(&paths.config_file) {
                        ui.set_profile_feedback(SharedString::from(format!(
                            "Save failed: {error}"
                        )));
                        return;
                    }
                    *config.borrow_mut() = candidate.clone();
                    set_ui(&ui, &candidate, &monitors.borrow());
                    ui.set_profile_feedback(SharedString::from("Saved"));
                }
                Err(error) => ui.set_profile_feedback(SharedString::from(error)),
            }
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let paths = paths.clone();
        let last_auto_save = last_auto_save.clone();
        let monitors = monitors.clone();
        ui.on_reset_requested(move || {
            let Some(ui) = weak.upgrade() else { return };
            let candidate = Config::default();
            match apply_candidate(
                &ui,
                &config,
                &paths,
                candidate.clone(),
                true,
                Some("Defaults restored"),
                &monitors.borrow(),
            ) {
                Ok(()) => last_auto_save.set(candidate.auto_save),
                Err(error) => eprintln!("Reset failed: {error}"),
            }
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let monitors = monitors.clone();
        ui.on_export_profile(move || {
            let Some(ui) = weak.upgrade() else { return };
            let old = config.borrow().clone();
            let candidate = match read_ui(&ui, &old, &monitors.borrow()) {
                Ok(candidate) => candidate,
                Err(error) => {
                    ui.set_profile_feedback(SharedString::from(error));
                    return;
                }
            };
            match candidate.export_code() {
                Ok(code) => {
                    ui.set_profile_code(SharedString::from(code));
                    ui.set_profile_feedback(SharedString::from(
                        "Copy this code to share the crosshair",
                    ));
                }
                Err(error) => ui.set_profile_feedback(SharedString::from(error.to_string())),
            }
            *config.borrow_mut() = candidate;
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let paths = paths.clone();
        let last_auto_save = last_auto_save.clone();
        let monitors = monitors.clone();
        ui.on_import_profile(move |code| {
            let Some(ui) = weak.upgrade() else { return };
            let mut candidate = config.borrow().clone();
            match candidate.import_code(&code) {
                Ok(()) => {
                    match apply_candidate(
                        &ui,
                        &config,
                        &paths,
                        candidate.clone(),
                        true,
                        Some("Crosshair imported"),
                        &monitors.borrow(),
                    ) {
                        Ok(()) => last_auto_save.set(candidate.auto_save),
                        Err(error) => eprintln!("Import apply failed: {error}"),
                    }
                }
                Err(error) => ui.set_profile_feedback(SharedString::from(error.to_string())),
            }
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let monitors = monitors.clone();
        ui.on_refresh_monitors(move || {
            let updated = discover_monitors();
            *monitors.borrow_mut() = updated;
            if let Some(ui) = weak.upgrade() {
                let current = config.borrow().clone();
                set_monitor_ui(&ui, &current, &monitors.borrow());
            }
        });
    }

    {
        let paths = paths.clone();
        ui.on_open_config_folder(move || {
            if let Err(error) = open_config_directory(&paths) {
                eprintln!("{error}");
            }
        });
    }

    {
        let weak = weak.clone();
        let config = config.clone();
        let paths = paths.clone();
        let monitors = monitors.clone();
        ui.on_reload_config(move || {
            let Some(ui) = weak.upgrade() else { return };
            match Config::load(&paths.config_file) {
                Ok(candidate) => {
                    if let Err(error) = notify_overlay(&candidate, &paths) {
                        ui.set_profile_feedback(SharedString::from(error));
                    } else {
                        *config.borrow_mut() = candidate.clone();
                        set_ui(&ui, &candidate, &monitors.borrow());
                        ui.set_profile_feedback(SharedString::from("Reloaded"));
                    }
                }
                Err(error) => {
                    ui.set_profile_feedback(SharedString::from(format!("Reload failed: {error}")))
                }
            }
        });
    }

    {
        let weak = weak.clone();
        ui.on_install_autostart(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_profile_feedback(SharedString::from(
                    "Autostart installation is being added to the installer workflow.",
                ));
            }
        });
    }

    {
        let weak = weak.clone();
        ui.on_remove_autostart(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_profile_feedback(SharedString::from(
                    "No autostart entry is installed by default.",
                ));
            }
        });
    }

    {
        let weak = weak.clone();
        ui.on_uninstall_app(move || {
            if let Some(ui) = weak.upgrade() {
                ui.set_profile_feedback(SharedString::from(
                    "Use crosshair --uninstall to remove both native executables. Your settings are retained.",
                ));
            }
        });
    }

    // The Settings process is intentionally separate from the overlay daemon.
    // Closing this window ends this process; an active overlay keeps running.
    ui.run().map_err(|error| error.to_string())?;

    // A user can close the window during the 75 ms coalescing delay. Flush the
    // final values when auto-save is enabled (including the instant the user
    // turns auto-save off). With auto-save already off, discard the unsaved
    // preview state and restore the last persisted configuration.
    let previous = config.borrow().clone();
    let should_save = ui.get_auto_save() || previous.auto_save;
    let final_config = if should_save {
        let candidate = read_ui(&ui, &previous, &monitors.borrow())?;
        candidate
            .save_atomic(&paths.config_file)
            .map_err(|error| format!("Could not save final Settings changes: {error}"))?;
        candidate
    } else {
        Config::load(&paths.config_file).map_err(|error| error.to_string())?
    };

    if final_config != previous {
        if let Err(error) = notify_overlay(&final_config, &paths) {
            eprintln!("Could not apply final Settings state after close: {error}");
        }
        *config.borrow_mut() = final_config;
    }
    Ok(())
}

fn main() {
    if let Err(error) = run() {
        eprintln!("Crosshair Settings: {error}");
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appearance_preview_is_independent_of_overlay_enabled_state() {
        let enabled = Config::default();
        let mut disabled = enabled.clone();
        disabled.enabled = false;

        assert_eq!(preview_pixels(&enabled), preview_pixels(&disabled));
    }
}
