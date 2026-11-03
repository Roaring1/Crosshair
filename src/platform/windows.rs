//! Native Windows overlay and per-user named-pipe IPC backend.
//!
//! The overlay uses a small per-monitor layered popup window rather than a
//! full-desktop surface. Its per-pixel alpha buffer is supplied by the shared
//! Rust rasterizer; WS_EX_TRANSPARENT and WM_NCHITTEST keep input flowing to
//! the game underneath without activating Crosshair.

use crate::render::render_crosshair_dpi;
use crate::{APP_ID, APP_VERSION, AppPaths, Config, MonitorDescriptor};
use serde_json::{Value, json};
use std::collections::hash_map::DefaultHasher;
use std::ffi::c_void;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufReader, Write};
use std::mem::size_of;
use std::os::windows::io::{AsRawHandle, FromRawHandle};
use std::path::Path;
use std::ptr::{null, null_mut};
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use windows_sys::Win32::Foundation::{
    ERROR_PIPE_CONNECTED, GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, HWND, LPARAM, POINT,
    SIZE, WPARAM,
};
use windows_sys::Win32::Graphics::Gdi::{
    AC_SRC_ALPHA, AC_SRC_OVER, BI_RGB, BITMAPINFO, BITMAPINFOHEADER, BLENDFUNCTION,
    CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DISPLAY_DEVICEW, DeleteDC, DeleteObject,
    EnumDisplayDevicesW, EnumDisplayMonitors, GetMonitorInfoW, HDC, HGDIOBJ, HMONITOR,
    MONITOR_DEFAULTTONEAREST, MONITORINFOEXW, MonitorFromPoint, SelectObject,
};
use windows_sys::Win32::Security::SECURITY_ATTRIBUTES;
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_FLAG_FIRST_PIPE_INSTANCE, OPEN_EXISTING,
    PIPE_ACCESS_DUPLEX,
};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Pipes::{
    ConnectNamedPipe, CreateNamedPipeW, DisconnectNamedPipe, PIPE_READMODE_BYTE,
    PIPE_REJECT_REMOTE_CLIENTS, PIPE_TYPE_BYTE, PIPE_WAIT, WaitNamedPipeW,
};
use windows_sys::Win32::System::Registry::{
    HKEY_CURRENT_USER, KEY_SET_VALUE, REG_OPTION_NON_VOLATILE, REG_SZ, RegCloseKey,
    RegCreateKeyExW, RegDeleteValueW, RegOpenKeyExW, RegSetValueExW,
};
use windows_sys::Win32::System::Threading::GetCurrentThreadId;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    SetProcessDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    GetCursorPos, GetMessageW, HTTRANSPARENT, HWND_TOPMOST, MA_NOACTIVATE, MONITORINFOF_PRIMARY,
    MSG, PM_NOREMOVE, PeekMessageW, PostThreadMessageW, RegisterClassW, SW_SHOWNOACTIVATE,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SetWindowPos, ShowWindow,
    TranslateMessage, ULW_ALPHA, UnregisterClassW, UpdateLayeredWindow, WM_APP, WM_CLOSE,
    WM_DISPLAYCHANGE, WM_DPICHANGED, WM_MOUSEACTIVATE, WM_NCHITTEST, WM_QUIT, WNDCLASSW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

#[link(name = "user32")]
unsafe extern "system" {
    fn GetDpiForWindow(hwnd: HWND) -> u32;
}

const WM_APP_REFRESH: u32 = WM_APP + 17;
const PIPE_BUFFER_BYTES: u32 = 64 * 1024;
const PIPE_CONNECT_TIMEOUT_MS: u32 = 1000;
const CLASS_NAME: &str = "CrosshairRustOverlayWindow";
const OVERLAY_EX_STYLE: u32 =
    WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;

static MAIN_THREAD_ID: AtomicU32 = AtomicU32::new(0);

#[derive(Clone)]
struct Monitor {
    handle: HMONITOR,
    rect: windows_sys::Win32::Foundation::RECT,
    primary: bool,
    name: String,
    ordinal: u8,
}

fn wide(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn from_wide(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn last_error(context: &str) -> String {
    format!("{context} (Windows error {})", unsafe { GetLastError() })
}

unsafe extern "system" fn monitor_enum(
    handle: HMONITOR,
    _dc: HDC,
    _rect: *mut windows_sys::Win32::Foundation::RECT,
    data: LPARAM,
) -> windows_sys::core::BOOL {
    let monitors = unsafe { &mut *(data as *mut Vec<Monitor>) };
    let mut info = MONITORINFOEXW::default();
    info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
    if unsafe { GetMonitorInfoW(handle, &mut info.monitorInfo as *mut _) } == 0 {
        return 1;
    }

    let device_name = from_wide(&info.szDevice);
    let mut display = DISPLAY_DEVICEW::default();
    display.cb = size_of::<DISPLAY_DEVICEW>() as u32;
    let friendly_name =
        if unsafe { EnumDisplayDevicesW(info.szDevice.as_ptr(), 0, &mut display, 0) } != 0 {
            let name = from_wide(&display.DeviceString);
            if name.is_empty() { device_name } else { name }
        } else {
            device_name
        };

    let ordinal = monitors
        .iter()
        .filter(|monitor| monitor.name == friendly_name)
        .count()
        .min(u8::MAX as usize) as u8;
    monitors.push(Monitor {
        handle,
        rect: info.monitorInfo.rcMonitor,
        primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        name: friendly_name,
        ordinal,
    });
    1
}

fn enumerate_monitors() -> Result<Vec<Monitor>, String> {
    let mut monitors: Vec<Monitor> = Vec::new();
    let ok = unsafe {
        EnumDisplayMonitors(
            null_mut(),
            null(),
            Some(monitor_enum),
            &mut monitors as *mut Vec<Monitor> as LPARAM,
        )
    };
    if ok == 0 {
        return Err(last_error("Could not enumerate Windows displays"));
    }
    if monitors.is_empty() {
        return Err("Windows did not report any active displays".into());
    }
    monitors.sort_by(|left, right| {
        (left.rect.top, left.rect.left, &left.name).cmp(&(
            right.rect.top,
            right.rect.left,
            &right.name,
        ))
    });
    for index in 0..monitors.len() {
        let duplicate_ordinal = monitors[..index]
            .iter()
            .filter(|previous| previous.name == monitors[index].name)
            .count();
        monitors[index].ordinal = duplicate_ordinal.min(u8::MAX as usize) as u8;
    }
    Ok(monitors)
}

fn descriptors(monitors: &[Monitor]) -> Vec<MonitorDescriptor> {
    monitors
        .iter()
        .map(|monitor| MonitorDescriptor {
            name: monitor.name.clone(),
            ordinal: monitor.ordinal,
        })
        .collect()
}

fn enable_per_monitor_dpi_awareness() {
    unsafe {
        if SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) == 0 {
            // Windows 10 1607 supports PMv1 but predates per-monitor v2.
            SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE);
        }
    }
}

pub fn list_monitors() -> Result<Vec<MonitorDescriptor>, String> {
    enable_per_monitor_dpi_awareness();
    enumerate_monitors().map(|monitors| descriptors(&monitors))
}

pub fn install_autostart(executable: &Path) -> Result<(), String> {
    if !executable.is_file() {
        return Err(format!(
            "Installed Crosshair executable was not found: {}",
            executable.display()
        ));
    }
    // The Run key is launched by Explorer. Start the console-subsystem CLI
    // through hidden PowerShell so login autostart never flashes a terminal.
    let executable = executable.to_string_lossy().replace('\'', "''");
    let command = format!(
        "powershell.exe -NoProfile -WindowStyle Hidden -Command \\\"Start-Process -FilePath '{executable}' -ArgumentList @('--daemon','--no-ui') -WindowStyle Hidden\\\""
    );
    let command = wide(&command);
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = wide("Crosshair");
    let mut key = null_mut();
    let mut disposition = 0u32;
    let created = unsafe {
        RegCreateKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            null(),
            REG_OPTION_NON_VOLATILE,
            KEY_SET_VALUE,
            null(),
            &mut key,
            &mut disposition,
        )
    };
    if created != 0 {
        return Err(format!(
            "Could not open the current-user startup registry key (Windows error {created})"
        ));
    }
    let written = unsafe {
        RegSetValueExW(
            key,
            value_name.as_ptr(),
            0,
            REG_SZ,
            command.as_ptr().cast(),
            (command.len() * size_of::<u16>()) as u32,
        )
    };
    unsafe { RegCloseKey(key) };
    if written != 0 {
        return Err(format!(
            "Could not enable Crosshair login startup (Windows error {written})"
        ));
    }
    Ok(())
}

pub fn remove_autostart() -> Result<bool, String> {
    let subkey = wide(r"Software\Microsoft\Windows\CurrentVersion\Run");
    let value_name = wide("Crosshair");
    let mut key = null_mut();
    let opened = unsafe {
        RegOpenKeyExW(
            HKEY_CURRENT_USER,
            subkey.as_ptr(),
            0,
            KEY_SET_VALUE,
            &mut key,
        )
    };
    if opened == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        return Ok(false);
    }
    if opened != 0 {
        return Err(format!(
            "Could not open the current-user startup registry key (Windows error {opened})"
        ));
    }
    let removed = unsafe { RegDeleteValueW(key, value_name.as_ptr()) };
    unsafe { RegCloseKey(key) };
    if removed == windows_sys::Win32::Foundation::ERROR_FILE_NOT_FOUND {
        Ok(false)
    } else if removed != 0 {
        Err(format!(
            "Could not remove Crosshair login startup (Windows error {removed})"
        ))
    } else {
        Ok(true)
    }
}

fn selected_monitors(config: &Config, monitors: &[Monitor]) -> Vec<Monitor> {
    match config.monitor_mode.as_str() {
        "all" => monitors.to_vec(),
        "primary" => vec![
            monitors
                .iter()
                .find(|monitor| monitor.primary)
                .cloned()
                .unwrap_or_else(|| monitors[0].clone()),
        ],
        "active" => {
            let mut point = POINT::default();
            let active = if unsafe { GetCursorPos(&mut point) } != 0 {
                unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) }
            } else {
                null_mut()
            };
            vec![
                monitors
                    .iter()
                    .find(|monitor| monitor.handle == active)
                    .cloned()
                    .unwrap_or_else(|| {
                        monitors
                            .iter()
                            .find(|monitor| monitor.primary)
                            .cloned()
                            .unwrap_or_else(|| monitors[0].clone())
                    }),
            ]
        }
        "index" => vec![
            monitors
                .get(config.monitor_index as usize)
                .cloned()
                .unwrap_or_else(|| {
                    monitors
                        .iter()
                        .find(|monitor| monitor.primary)
                        .cloned()
                        .unwrap_or_else(|| monitors[0].clone())
                }),
        ],
        "name" if !config.monitor_name.is_empty() => vec![
            monitors
                .iter()
                .find(|monitor| {
                    monitor.name == config.monitor_name
                        && monitor.ordinal == config.monitor_name_ordinal
                })
                .cloned()
                .unwrap_or_else(|| {
                    monitors
                        .iter()
                        .find(|monitor| monitor.primary)
                        .cloned()
                        .unwrap_or_else(|| monitors[0].clone())
                }),
        ],
        _ => vec![
            monitors
                .iter()
                .find(|monitor| monitor.primary)
                .cloned()
                .unwrap_or_else(|| monitors[0].clone()),
        ],
    }
}

fn window_dpi(hwnd: HWND) -> u32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi > 0 { dpi.clamp(48, 384) } else { 96 }
}

struct OverlayWindow {
    monitor: HMONITOR,
    hwnd: HWND,
}

fn update_layered_window(hwnd: HWND, monitor: &Monitor, config: &Config) -> Result<(), String> {
    // Move the HWND onto its target monitor before querying its DPI. This is
    // important when hotplug/reordering reuses a window from another output.
    if unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            monitor.rect.left,
            monitor.rect.top,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOSIZE,
        )
    } == 0
    {
        return Err(last_error("Could not place overlay on its target monitor"));
    }
    let dpi = window_dpi(hwnd);
    let raster = render_crosshair_dpi(config, dpi).map_err(|error| error.to_string())?;
    let width = raster.width.max(1);
    let height = raster.height.max(1);
    let width_i32 = i32::try_from(width).map_err(|_| "Crosshair raster is too wide")?;
    let height_i32 = i32::try_from(height).map_err(|_| "Crosshair raster is too tall")?;
    let monitor_width = monitor.rect.right - monitor.rect.left;
    let monitor_height = monitor.rect.bottom - monitor.rect.top;
    let x = monitor.rect.left + (monitor_width - width_i32) / 2 + raster.origin_x;
    let y = monitor.rect.top + (monitor_height - height_i32) / 2 + raster.origin_y;

    let memory_dc = unsafe { CreateCompatibleDC(null_mut()) };
    if memory_dc.is_null() {
        return Err(last_error("Could not create a memory device context"));
    }

    let mut bitmap_info = BITMAPINFO::default();
    bitmap_info.bmiHeader = BITMAPINFOHEADER {
        biSize: size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: width_i32,
        biHeight: -height_i32,
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        ..Default::default()
    };
    let mut bits: *mut c_void = null_mut();
    let bitmap = unsafe {
        CreateDIBSection(
            memory_dc,
            &bitmap_info,
            DIB_RGB_COLORS,
            &mut bits,
            null_mut(),
            0,
        )
    };
    if bitmap.is_null() || bits.is_null() {
        unsafe { DeleteDC(memory_dc) };
        return Err(last_error("Could not allocate the layered-window bitmap"));
    }

    let byte_count = raster.bgra_premultiplied.len();
    unsafe {
        std::ptr::copy_nonoverlapping(
            raster.bgra_premultiplied.as_ptr(),
            bits.cast::<u8>(),
            byte_count,
        );
    }
    let previous = unsafe { SelectObject(memory_dc, bitmap as HGDIOBJ) };
    let destination = POINT { x, y };
    let source = POINT { x: 0, y: 0 };
    let size = SIZE {
        cx: width_i32,
        cy: height_i32,
    };
    let blend = BLENDFUNCTION {
        BlendOp: AC_SRC_OVER as u8,
        BlendFlags: 0,
        SourceConstantAlpha: 255,
        AlphaFormat: AC_SRC_ALPHA as u8,
    };
    let updated = unsafe {
        UpdateLayeredWindow(
            hwnd,
            null_mut(),
            &destination,
            &size,
            memory_dc,
            &source,
            0,
            &blend,
            ULW_ALPHA,
        )
    };
    if !previous.is_null() {
        unsafe { SelectObject(memory_dc, previous) };
    }
    unsafe {
        DeleteObject(bitmap as HGDIOBJ);
        DeleteDC(memory_dc);
    }
    if updated == 0 {
        return Err(last_error(
            "Could not update the transparent overlay surface",
        ));
    }
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            0,
            0,
            SWP_NOACTIVATE | SWP_NOMOVE | SWP_NOSIZE | SWP_SHOWWINDOW,
        );
        ShowWindow(hwnd, SW_SHOWNOACTIVATE);
    }
    Ok(())
}

unsafe extern "system" fn overlay_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> isize {
    match message {
        WM_MOUSEACTIVATE => MA_NOACTIVATE as isize,
        WM_NCHITTEST => HTTRANSPARENT as isize,
        WM_DISPLAYCHANGE | WM_DPICHANGED => {
            let thread_id = MAIN_THREAD_ID.load(Ordering::Acquire);
            if thread_id != 0 {
                unsafe { PostThreadMessageW(thread_id, WM_APP_REFRESH, 0, 0) };
            }
            0
        }
        WM_CLOSE => 0,
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn register_overlay_class() -> Result<(Vec<u16>, windows_sys::Win32::Foundation::HINSTANCE), String>
{
    let class_name = wide(CLASS_NAME);
    let instance = unsafe { GetModuleHandleW(null()) };
    if instance.is_null() {
        return Err(last_error("Could not get the Crosshair module handle"));
    }
    let class = WNDCLASSW {
        style: CS_HREDRAW | CS_VREDRAW,
        lpfnWndProc: Some(overlay_window_proc),
        hInstance: instance,
        lpszClassName: class_name.as_ptr(),
        ..Default::default()
    };
    if unsafe { RegisterClassW(&class) } == 0 {
        return Err(last_error("Could not register the overlay window class"));
    }
    Ok((class_name, instance))
}

fn refresh_windows(
    config: &Config,
    windows: &mut Vec<OverlayWindow>,
    class_name: &[u16],
    instance: windows_sys::Win32::Foundation::HINSTANCE,
) -> Result<(), String> {
    if !config.enabled {
        for window in windows.drain(..) {
            unsafe { DestroyWindow(window.hwnd) };
        }
        return Ok(());
    }

    let monitors = enumerate_monitors()?;
    let selected = selected_monitors(config, &monitors);
    let previous = std::mem::take(windows);
    let mut next: Vec<OverlayWindow> = Vec::with_capacity(selected.len());

    for monitor in selected {
        let hwnd = if let Some(index) = previous
            .iter()
            .position(|window| window.monitor == monitor.handle)
        {
            previous[index].hwnd
        } else {
            let hwnd = unsafe {
                CreateWindowExW(
                    OVERLAY_EX_STYLE,
                    class_name.as_ptr(),
                    wide("").as_ptr(),
                    WS_POPUP,
                    monitor.rect.left,
                    monitor.rect.top,
                    1,
                    1,
                    null_mut(),
                    null_mut(),
                    instance,
                    null(),
                )
            };
            if hwnd.is_null() {
                for window in next.drain(..) {
                    unsafe { DestroyWindow(window.hwnd) };
                }
                for window in previous.iter() {
                    unsafe { DestroyWindow(window.hwnd) };
                }
                return Err(last_error("Could not create a transparent overlay window"));
            }
            hwnd
        };

        if let Err(error) = update_layered_window(hwnd, &monitor, config) {
            unsafe { DestroyWindow(hwnd) };
            for window in next.drain(..) {
                unsafe { DestroyWindow(window.hwnd) };
            }
            for window in previous.iter() {
                unsafe { DestroyWindow(window.hwnd) };
            }
            return Err(error);
        }
        next.push(OverlayWindow {
            monitor: monitor.handle,
            hwnd,
        });
    }

    for window in previous {
        if !next.iter().any(|active| active.hwnd == window.hwnd) {
            unsafe { DestroyWindow(window.hwnd) };
        }
    }
    *windows = next;
    Ok(())
}

fn pipe_name() -> Vec<u16> {
    let identity = [
        std::env::var("USERDOMAIN").unwrap_or_default(),
        std::env::var("USERNAME").unwrap_or_default(),
        std::env::var("USERPROFILE").unwrap_or_default(),
    ]
    .join("|");
    let mut hasher = DefaultHasher::new();
    identity.hash(&mut hasher);
    let name = format!(r"\\.\pipe\{APP_ID}-{:016x}", hasher.finish());
    wide(&name)
}

fn create_pipe_server(name: &[u16], first: bool) -> Result<File, String> {
    let mut attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        // NULL uses the current process token's default DACL. This keeps the
        // pipe private to the current Windows logon rather than granting the
        // Everyone/anonymous read ACE used by a NULL SECURITY_ATTRIBUTES ptr.
        lpSecurityDescriptor: null_mut(),
        bInheritHandle: 0,
    };
    let open_mode = PIPE_ACCESS_DUPLEX
        | if first {
            FILE_FLAG_FIRST_PIPE_INSTANCE
        } else {
            0
        };
    let handle = unsafe {
        CreateNamedPipeW(
            name.as_ptr(),
            open_mode,
            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT | PIPE_REJECT_REMOTE_CLIENTS,
            1,
            PIPE_BUFFER_BYTES,
            PIPE_BUFFER_BYTES,
            PIPE_CONNECT_TIMEOUT_MS,
            &mut attributes,
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(last_error(
            "Could not create the per-user Crosshair named pipe",
        ));
    }
    Ok(unsafe { File::from_raw_handle(handle as _) })
}

fn merge_config(current: &Config, payload: Value) -> Result<Config, String> {
    let patch = payload
        .as_object()
        .ok_or_else(|| "Settings IPC payload must be a JSON object".to_string())?;
    let mut value = serde_json::to_value(current).map_err(|error| error.to_string())?;
    let base = value
        .as_object_mut()
        .ok_or_else(|| "Could not serialize the current Crosshair configuration".to_string())?;
    for (key, item) in patch {
        base.insert(key.clone(), item.clone());
    }
    Config::from_json(&value.to_string()).map_err(|error| error.to_string())
}

fn process_request(
    request: &[u8],
    config: &Arc<Mutex<Config>>,
    paths: &AppPaths,
    running: &Arc<AtomicBool>,
    main_thread: u32,
) -> (Value, bool) {
    let parsed = match serde_json::from_slice::<Value>(request) {
        Ok(value) => value,
        Err(error) => {
            return (
                json!({"ok": false, "message": format!("Invalid IPC JSON: {error}")}),
                false,
            );
        }
    };
    let command = parsed.get("cmd").and_then(Value::as_str).unwrap_or("");
    let result: Result<Value, String> = (|| match command {
        "status" => {
            let current = config.lock().map_err(|_| "Configuration lock poisoned")?;
            let monitors = enumerate_monitors()?;
            Ok(json!({
                "ok": true,
                "version": APP_VERSION,
                "enabled": current.enabled,
                "monitors": selected_monitors(&current, &monitors)
                    .iter()
                    .map(|monitor| monitor.name.clone())
                    .collect::<Vec<_>>()
            }))
        }
        "set" | "preview" => {
            let payload = parsed
                .get("payload")
                .cloned()
                .ok_or_else(|| "Settings IPC request is missing its payload".to_string())?;
            let current = config
                .lock()
                .map_err(|_| "Configuration lock poisoned")?
                .clone();
            let next = merge_config(&current, payload)?;
            if command == "set" {
                next.save_atomic(&paths.config_file)
                    .map_err(|error| error.to_string())?;
            }
            *config.lock().map_err(|_| "Configuration lock poisoned")? = next;
            unsafe { PostThreadMessageW(main_thread, WM_APP_REFRESH, 0, 0) };
            Ok(json!({"ok": true}))
        }
        "reload" => {
            let next = Config::load(&paths.config_file).map_err(|error| error.to_string())?;
            *config.lock().map_err(|_| "Configuration lock poisoned")? = next;
            unsafe { PostThreadMessageW(main_thread, WM_APP_REFRESH, 0, 0) };
            Ok(json!({"ok": true}))
        }
        "show" | "hide" | "toggle" => {
            let current = config
                .lock()
                .map_err(|_| "Configuration lock poisoned")?
                .clone();
            let mut next = current;
            next.enabled = match command {
                "show" => true,
                "hide" => false,
                _ => !next.enabled,
            };
            next.save_atomic(&paths.config_file)
                .map_err(|error| error.to_string())?;
            let should_stop = command == "hide" || (command == "toggle" && !next.enabled);
            *config.lock().map_err(|_| "Configuration lock poisoned")? = next;
            if should_stop {
                running.store(false, Ordering::Release);
                unsafe { PostThreadMessageW(main_thread, WM_QUIT, 0, 0) };
            } else {
                unsafe { PostThreadMessageW(main_thread, WM_APP_REFRESH, 0, 0) };
            }
            Ok(json!({"ok": true}))
        }
        "quit" => {
            running.store(false, Ordering::Release);
            unsafe { PostThreadMessageW(main_thread, WM_QUIT, 0, 0) };
            Ok(json!({"ok": true}))
        }
        other => Err(format!("Unknown Crosshair IPC command: {other}")),
    })();
    match result {
        Ok(value) => (value, command == "quit"),
        Err(error) => (json!({"ok": false, "message": error}), false),
    }
}

fn named_pipe_server(
    first_pipe: File,
    config: Arc<Mutex<Config>>,
    paths: AppPaths,
    running: Arc<AtomicBool>,
    main_thread: u32,
) -> Result<(), String> {
    // Reuse the listening instance after each client. Closing/recreating the
    // pipe between requests creates a window where a second CLI command can
    // incorrectly observe "not running".
    let mut file = first_pipe;
    let handle = file.as_raw_handle() as HANDLE;
    while running.load(Ordering::Acquire) {
        let connected = unsafe { ConnectNamedPipe(handle, null_mut()) };
        if connected == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_PIPE_CONNECTED {
                if !running.load(Ordering::Acquire) {
                    break;
                }
                let _ = unsafe { DisconnectNamedPipe(handle) };
                if error == windows_sys::Win32::Foundation::ERROR_NO_DATA {
                    continue;
                }
                return Err(format!(
                    "Could not accept a Crosshair named-pipe client (Windows error {error})"
                ));
            }
        }

        let response = match crate::read_bounded_line(BufReader::new(&mut file)) {
            Ok(request) => process_request(&request, &config, &paths, &running, main_thread).0,
            Err(error) => json!({"ok": false, "message": format!("Invalid IPC frame: {error}")}),
        };
        if let Ok(mut bytes) = serde_json::to_vec(&response) {
            bytes.push(b'\n');
            let _ = file.write_all(&bytes);
            let _ = file.flush();
        }
        unsafe { DisconnectNamedPipe(handle) };
    }
    Ok(())
}

pub fn send_command(
    _paths: &AppPaths,
    command: &str,
    payload: Option<Value>,
) -> Result<Value, String> {
    let name = pipe_name();
    if unsafe { WaitNamedPipeW(name.as_ptr(), PIPE_CONNECT_TIMEOUT_MS) } == 0 {
        return Err("Crosshair overlay is not running".into());
    }
    let handle = unsafe {
        CreateFileW(
            name.as_ptr(),
            GENERIC_READ | GENERIC_WRITE,
            0,
            null(),
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            null_mut(),
        )
    };
    if handle == windows_sys::Win32::Foundation::INVALID_HANDLE_VALUE {
        return Err(last_error("Could not connect to the Crosshair named pipe"));
    }
    let mut file = unsafe { File::from_raw_handle(handle as _) };
    let mut request = json!({"cmd": command});
    if let Some(payload) = payload {
        request["payload"] = payload;
    }
    let mut bytes = serde_json::to_vec(&request).map_err(|error| error.to_string())?;
    if bytes.len() >= crosshair_ipc_limit() {
        return Err("Control request exceeds 64 KiB".into());
    }
    bytes.push(b'\n');
    file.write_all(&bytes).map_err(|error| error.to_string())?;
    file.flush().map_err(|error| error.to_string())?;
    let response = crate::read_bounded_line(BufReader::new(&mut file))
        .map_err(|error| format!("Invalid response from Crosshair: {error}"))?;
    let response: Value = serde_json::from_slice(&response).map_err(|error| error.to_string())?;
    if response.get("ok").and_then(Value::as_bool) != Some(true) {
        return Err(response
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("Overlay rejected the command")
            .to_string());
    }
    Ok(response)
}

fn crosshair_ipc_limit() -> usize {
    crate::MAX_IPC_MESSAGE_BYTES
}

pub fn run(config: Config, paths: AppPaths) -> Result<(), String> {
    enable_per_monitor_dpi_awareness();
    paths
        .ensure_runtime_dir()
        .map_err(|error| error.to_string())?;
    let (class_name, instance) = register_overlay_class()?;
    let mut message = MSG::default();
    unsafe { PeekMessageW(&mut message, null_mut(), 0, 0, PM_NOREMOVE) };

    let config = Arc::new(Mutex::new(config));
    let running = Arc::new(AtomicBool::new(true));
    let main_thread = unsafe { GetCurrentThreadId() };
    MAIN_THREAD_ID.store(main_thread, Ordering::Release);

    let mut windows = Vec::new();
    let initial_config = config
        .lock()
        .map_err(|_| "Configuration lock poisoned")?
        .clone();
    if let Err(error) = refresh_windows(&initial_config, &mut windows, &class_name, instance) {
        MAIN_THREAD_ID.store(0, Ordering::Release);
        unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
        return Err(error);
    }

    let pipe = match create_pipe_server(&pipe_name(), true) {
        Ok(pipe) => pipe,
        Err(error) => {
            for window in windows.drain(..) {
                unsafe { DestroyWindow(window.hwnd) };
            }
            unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
            return Err(error);
        }
    };
    let worker_config = config.clone();
    let worker_running = running.clone();
    let worker_paths = paths.clone();
    let worker = thread::Builder::new()
        .name("crosshair-windows-ipc".into())
        .spawn(move || {
            let result = named_pipe_server(
                pipe,
                worker_config,
                worker_paths,
                worker_running,
                main_thread,
            );
            if result.is_err() {
                unsafe { PostThreadMessageW(main_thread, WM_QUIT, 0, 0) };
            }
            result
        })
        .map_err(|error| format!("Could not start the Windows IPC worker: {error}"))?;

    loop {
        let result = unsafe { GetMessageW(&mut message, null_mut(), 0, 0) };
        if result <= 0 {
            break;
        }
        if message.message == WM_APP_REFRESH {
            let current = config
                .lock()
                .map_err(|_| "Configuration lock poisoned")?
                .clone();
            if let Err(error) = refresh_windows(&current, &mut windows, &class_name, instance) {
                eprintln!("Crosshair Windows overlay refresh failed: {error}");
            }
        } else {
            unsafe {
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }

    running.store(false, Ordering::Release);
    MAIN_THREAD_ID.store(0, Ordering::Release);
    for window in windows.drain(..) {
        unsafe { DestroyWindow(window.hwnd) };
    }
    unsafe { UnregisterClassW(class_name.as_ptr(), instance) };
    worker
        .join()
        .map_err(|_| "Windows IPC worker panicked".to_string())??;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{pipe_name, run, send_command};
    use crate::{AppPaths, Config};
    use serde_json::json;
    use std::fs;
    use std::thread;
    use std::time::Duration;

    #[test]
    fn named_pipe_is_local_and_uses_a_stable_per_user_name() {
        let first = pipe_name();
        let second = pipe_name();
        assert_eq!(first, second);
        let name = String::from_utf16(&first[..first.len() - 1]).unwrap();
        assert!(name.starts_with(r"\\.\pipe\crosshair-"));
        assert!(!name[9..].contains('\\'));
    }

    #[test]
    fn layered_overlay_is_topmost_tool_window_and_mouse_transparent() {
        use super::{
            OVERLAY_EX_STYLE, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
            WS_EX_TRANSPARENT,
        };
        let required =
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE;
        assert_eq!(OVERLAY_EX_STYLE & required, required);
    }

    #[test]
    fn overlay_window_messages_do_not_activate_and_pass_hit_testing_through() {
        use super::{
            HTTRANSPARENT, MA_NOACTIVATE, WM_MOUSEACTIVATE, WM_NCHITTEST, overlay_window_proc,
        };
        let mouse_activate =
            unsafe { overlay_window_proc(std::ptr::null_mut(), WM_MOUSEACTIVATE, 0, 0) };
        let hit_test = unsafe { overlay_window_proc(std::ptr::null_mut(), WM_NCHITTEST, 0, 0) };
        assert_eq!(mouse_activate, MA_NOACTIVATE as isize);
        assert_eq!(hit_test, HTTRANSPARENT as isize);
    }

    #[test]
    fn windows_overlay_named_pipe_round_trip_and_shutdown() {
        let _monitors = super::list_monitors().expect("Windows must expose at least one display");
        let directory = tempfile::tempdir().unwrap();
        let paths = AppPaths {
            config_dir: directory.path().join("config"),
            config_file: directory.path().join("config/config.json"),
            data_dir: directory.path().join("data"),
            state_dir: directory.path().join("state"),
            runtime_dir: directory.path().join("runtime"),
            socket_file: directory.path().join("runtime/unused.sock"),
            lock_file: directory.path().join("runtime/unused.lock"),
            log_file: directory.path().join("state/crosshair.log"),
        };
        fs::create_dir_all(&paths.config_dir).unwrap();
        let initial = Config {
            enabled: true,
            ..Config::default()
        };
        initial.save_atomic(&paths.config_file).unwrap();

        let worker_paths = paths.clone();
        let worker = thread::spawn(move || run(initial, worker_paths));

        let mut status = None;
        for _ in 0..100 {
            match send_command(&paths, "status", None) {
                Ok(response) => {
                    status = Some(response);
                    break;
                }
                Err(_) if !worker.is_finished() => thread::sleep(Duration::from_millis(50)),
                Err(error) => panic!("Windows overlay exited before pipe became ready: {error}"),
            }
        }
        let status = status.expect("Windows overlay named pipe did not become ready");
        assert_eq!(status["enabled"].as_bool(), Some(true));
        assert_eq!(status["version"].as_str(), Some(crate::APP_VERSION));
        assert!(
            status["monitors"]
                .as_array()
                .is_some_and(|items| !items.is_empty()),
            "status must include the selected Windows output"
        );

        send_command(
            &paths,
            "preview",
            Some(json!({"enabled": true, "shape": "cross", "size": 31})),
        )
        .expect("preview IPC should update the live layered window");
        let persisted = Config::load(&paths.config_file).unwrap();
        assert_eq!(persisted.shape, "dot", "preview must not auto-save");

        send_command(
            &paths,
            "set",
            Some(json!({"enabled": true, "shape": "x", "size": 37})),
        )
        .expect("set IPC should save and refresh the layered window");
        let persisted = Config::load(&paths.config_file).unwrap();
        assert_eq!(persisted.shape, "x");
        assert_eq!(persisted.size, 37);

        send_command(&paths, "hide", None).expect("hide should be acknowledged");
        let result = worker
            .join()
            .expect("Windows overlay thread should not panic");
        result.expect("Windows overlay should shut down cleanly after hide");
        assert!(
            send_command(&paths, "status", None).is_err(),
            "named pipe must stop accepting requests after daemon shutdown"
        );
    }
}
