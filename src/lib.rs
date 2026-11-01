use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

pub mod render;

pub const APP_ID: &str = "crosshair";
pub const APP_VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct MonitorDescriptor {
    pub name: String,
    /// Zero-based occurrence among outputs that advertise the same name.
    pub ordinal: u8,
}

const PROFILE_PREFIX: &str = "CH1-";
const PROFILE_BYTES: usize = 36;
const MAX_CONFIG_BYTES: usize = 1024 * 1024;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("configuration error: {0}")]
    Config(String),
    #[error("profile code error: {0}")]
    Profile(String),
    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),
}

pub const MAX_IPC_MESSAGE_BYTES: usize = 64 * 1024;

/// Read one bounded newline-delimited IPC message.
///
/// The extra byte distinguishes a full, valid-size message from an oversized
/// message even when the delimiter has not arrived yet. EOF without a newline
/// is rejected so callers never parse a truncated frame as a complete request.
pub fn read_bounded_line<R: std::io::BufRead>(reader: R) -> std::io::Result<Vec<u8>> {
    use std::io::BufRead;

    let mut line = Vec::with_capacity(1024);
    let mut reader = reader.take((MAX_IPC_MESSAGE_BYTES + 1) as u64);
    reader.read_until(b'\n', &mut line)?;
    if line.len() > MAX_IPC_MESSAGE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "IPC message exceeds 64 KiB",
        ));
    }
    if line.last() != Some(&b'\n') {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "IPC message is missing its newline delimiter",
        ));
    }
    Ok(line)
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Config {
    pub enabled: bool,
    pub auto_save: bool,
    pub shape: String,
    pub opacity: f32,
    pub size: u16,
    pub thickness: u16,
    pub gap: u16,
    pub color: String,
    pub outline: Outline,
    pub shadow: Shadow,
    pub monitor_mode: String,
    pub monitor_index: u16,
    pub monitor_name: String,
    pub monitor_name_ordinal: u8,
    pub preview_bg: String,
    pub offset_x: i16,
    pub offset_y: i16,
    pub reference_dot: ReferenceDot,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Outline {
    pub enabled: bool,
    pub color: String,
    pub opacity: f32,
    pub thickness: u16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Shadow {
    pub enabled: bool,
    pub color: String,
    pub opacity: f32,
    pub offset_x: i16,
    pub offset_y: i16,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ReferenceDot {
    pub enabled: bool,
    pub color: String,
    pub opacity: f32,
    pub size: u16,
    pub offset_x: i16,
    pub offset_y: i16,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            enabled: true,
            auto_save: true,
            shape: "dot".into(),
            opacity: 0.95,
            size: 5,
            thickness: 2,
            gap: 3,
            color: "#a000ff".into(),
            outline: Outline::default(),
            shadow: Shadow::default(),
            monitor_mode: "all".into(),
            monitor_index: 0,
            monitor_name: String::new(),
            monitor_name_ordinal: 0,
            preview_bg: "#222222".into(),
            offset_x: 0,
            offset_y: 0,
            reference_dot: ReferenceDot::default(),
        }
    }
}

impl Default for Outline {
    fn default() -> Self {
        Self {
            enabled: true,
            color: "#000000".into(),
            opacity: 0.80,
            thickness: 1,
        }
    }
}

impl Default for Shadow {
    fn default() -> Self {
        Self {
            enabled: false,
            color: "#000000".into(),
            opacity: 0.35,
            offset_x: 1,
            offset_y: 1,
        }
    }
}

impl Default for ReferenceDot {
    fn default() -> Self {
        Self {
            enabled: false,
            color: "#a000ff".into(),
            opacity: 0.95,
            size: 3,
            offset_x: 0,
            offset_y: 64,
        }
    }
}

impl Config {
    pub fn from_json(input: &str) -> Result<Self, CoreError> {
        if input.len() > MAX_CONFIG_BYTES {
            return Err(CoreError::Config("configuration exceeds 1 MiB".into()));
        }
        let mut config: Self = serde_json::from_str(input)?;
        config.normalize();
        Ok(config)
    }

    pub fn to_json(&self) -> Result<String, CoreError> {
        let mut config = self.clone();
        config.normalize();
        Ok(serde_json::to_string_pretty(&config)?)
    }

    pub fn normalize(&mut self) {
        const SHAPES: &[&str] = &[
            "dot",
            "cross",
            "x",
            "plus",
            "ring",
            "circle",
            "dot_ring",
            "cross_dot",
            "x_dot",
        ];
        if !SHAPES.contains(&self.shape.as_str()) {
            self.shape = "dot".into();
        }
        if self.shape == "circle" {
            self.shape = "ring".into();
        }
        if !["all", "primary", "active", "name", "index"].contains(&self.monitor_mode.as_str()) {
            self.monitor_mode = "all".into();
        }
        self.opacity = finite_clamp(self.opacity, 0.95, 0.05, 1.0);
        self.size = self.size.clamp(1, 200);
        self.thickness = self.thickness.clamp(1, 100);
        self.gap = self.gap.min(200);
        self.color = normalize_color(&self.color, "#a000ff");
        self.outline.opacity = finite_clamp(self.outline.opacity, 0.8, 0.0, 1.0);
        self.outline.thickness = self.outline.thickness.min(100);
        self.outline.color = normalize_color(&self.outline.color, "#000000");
        self.shadow.opacity = finite_clamp(self.shadow.opacity, 0.35, 0.0, 1.0);
        self.shadow.offset_x = self.shadow.offset_x.clamp(-200, 200);
        self.shadow.offset_y = self.shadow.offset_y.clamp(-200, 200);
        self.shadow.color = normalize_color(&self.shadow.color, "#000000");
        self.monitor_index = self.monitor_index.min(255);
        self.monitor_name.truncate(256);
        self.monitor_name_ordinal = self.monitor_name_ordinal.min(255);
        self.preview_bg = normalize_color(&self.preview_bg, "#222222");
        self.offset_x = self.offset_x.clamp(-500, 500);
        self.offset_y = self.offset_y.clamp(-500, 500);
        self.reference_dot.opacity = finite_clamp(self.reference_dot.opacity, 0.95, 0.0, 1.0);
        self.reference_dot.size = self.reference_dot.size.clamp(1, 100);
        self.reference_dot.offset_x = self.reference_dot.offset_x.clamp(-500, 500);
        self.reference_dot.offset_y = self.reference_dot.offset_y.clamp(-500, 500);
        self.reference_dot.color = normalize_color(&self.reference_dot.color, &self.color);
    }

    pub fn save_atomic(&self, path: &Path) -> Result<(), CoreError> {
        let text = self.to_json()?;
        atomic_write(path, text.as_bytes())
    }

    pub fn load(path: &Path) -> Result<Self, CoreError> {
        let bytes = read_config(path)?;
        match bytes {
            Some(bytes) => {
                let text = std::str::from_utf8(&bytes)
                    .map_err(|_| CoreError::Config("config is not valid UTF-8".into()))?;
                Self::from_json(text)
            }
            None => Ok(Self::default()),
        }
    }

    pub fn export_code(&self) -> Result<String, CoreError> {
        let mut c = self.clone();
        c.normalize();
        let shape =
            shape_index(&c.shape).ok_or_else(|| CoreError::Config("unknown shape".into()))?;
        let mut data = Vec::with_capacity(PROFILE_BYTES);
        let flags = shape
            | ((c.outline.enabled as u8) << 4)
            | ((c.shadow.enabled as u8) << 5)
            | ((c.reference_dot.enabled as u8) << 6);
        data.push(flags);
        data.push(c.size as u8);
        data.push(c.thickness as u8);
        data.push(c.gap as u8);
        data.push(float_to_byte(c.opacity));
        data.extend(color_to_rgb(&c.color)?);
        data.push(c.outline.thickness as u8);
        data.push(float_to_byte(c.outline.opacity));
        data.extend(color_to_rgb(&c.outline.color)?);
        data.push(float_to_byte(c.shadow.opacity));
        data.extend(color_to_rgb(&c.shadow.color)?);
        put_i16(&mut data, c.shadow.offset_x);
        put_i16(&mut data, c.shadow.offset_y);
        put_i16(&mut data, c.offset_x);
        put_i16(&mut data, c.offset_y);
        data.push(c.reference_dot.size as u8);
        data.push(float_to_byte(c.reference_dot.opacity));
        data.extend(color_to_rgb(&c.reference_dot.color)?);
        put_i16(&mut data, c.reference_dot.offset_x);
        put_i16(&mut data, c.reference_dot.offset_y);
        if data.len() != PROFILE_BYTES - 2 {
            return Err(CoreError::Profile(
                "internal profile layout mismatch".into(),
            ));
        }
        let crc = crc16_ccitt(&data);
        data.extend(crc.to_le_bytes());
        Ok(format!("{PROFILE_PREFIX}{}", base64_url_encode(&data)))
    }

    pub fn import_code(&mut self, code: &str) -> Result<(), CoreError> {
        let normalized: String = code.chars().filter(|c| !c.is_ascii_whitespace()).collect();
        let payload = normalized
            .strip_prefix(PROFILE_PREFIX)
            .or_else(|| normalized.strip_prefix("ch1-"))
            .ok_or_else(|| CoreError::Profile("code must start with CH1-".into()))?;
        let bytes = base64_url_decode(payload)?;
        if bytes.len() != PROFILE_BYTES {
            return Err(CoreError::Profile("wrong code length".into()));
        }
        let stored_crc = u16::from_le_bytes([bytes[34], bytes[35]]);
        if crc16_ccitt(&bytes[..34]) != stored_crc {
            return Err(CoreError::Profile(
                "code check failed; verify the copied characters".into(),
            ));
        }

        let flags = bytes[0];
        let shape = index_shape(flags & 0x0f)
            .ok_or_else(|| CoreError::Profile("unsupported crosshair shape".into()))?;
        let mut p = 1;
        let size = bytes[p] as u16;
        p += 1;
        let thickness = bytes[p] as u16;
        p += 1;
        let gap = bytes[p] as u16;
        p += 1;
        let opacity = byte_to_float(bytes[p]);
        p += 1;
        let color = rgb_to_color(&bytes[p..p + 3]);
        p += 3;
        let outline_thickness = bytes[p] as u16;
        p += 1;
        let outline_opacity = byte_to_float(bytes[p]);
        p += 1;
        let outline_color = rgb_to_color(&bytes[p..p + 3]);
        p += 3;
        let shadow_opacity = byte_to_float(bytes[p]);
        p += 1;
        let shadow_color = rgb_to_color(&bytes[p..p + 3]);
        p += 3;
        let shadow_x = get_i16(&bytes, &mut p);
        let shadow_y = get_i16(&bytes, &mut p);
        let offset_x = get_i16(&bytes, &mut p);
        let offset_y = get_i16(&bytes, &mut p);
        let reference_size = bytes[p] as u16;
        p += 1;
        let reference_opacity = byte_to_float(bytes[p]);
        p += 1;
        let reference_color = rgb_to_color(&bytes[p..p + 3]);
        p += 3;
        let reference_x = get_i16(&bytes, &mut p);
        let reference_y = get_i16(&bytes, &mut p);
        if p != 34 {
            return Err(CoreError::Profile("invalid profile layout".into()));
        }

        let mut candidate = self.clone();
        candidate.shape = shape.into();
        candidate.size = size;
        candidate.thickness = thickness;
        candidate.gap = gap;
        candidate.opacity = opacity;
        candidate.color = color;
        candidate.outline = Outline {
            enabled: flags & (1 << 4) != 0,
            color: outline_color,
            opacity: outline_opacity,
            thickness: outline_thickness,
        };
        candidate.shadow = Shadow {
            enabled: flags & (1 << 5) != 0,
            color: shadow_color,
            opacity: shadow_opacity,
            offset_x: shadow_x,
            offset_y: shadow_y,
        };
        candidate.offset_x = offset_x;
        candidate.offset_y = offset_y;
        candidate.reference_dot = ReferenceDot {
            enabled: flags & (1 << 6) != 0,
            color: reference_color,
            opacity: reference_opacity,
            size: reference_size,
            offset_x: reference_x,
            offset_y: reference_y,
        };
        candidate.normalize();
        *self = candidate;
        Ok(())
    }
}

#[derive(Clone, Debug)]
pub struct AppPaths {
    pub config_dir: PathBuf,
    pub config_file: PathBuf,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub runtime_dir: PathBuf,
    pub socket_file: PathBuf,
    pub lock_file: PathBuf,
    pub log_file: PathBuf,
}

impl AppPaths {
    pub fn ensure_runtime_dir(&self) -> Result<(), CoreError> {
        #[cfg(target_os = "linux")]
        {
            let _directory = open_directory_linux(&self.runtime_dir, true, true)?;
        }
        #[cfg(not(target_os = "linux"))]
        {
            std::fs::create_dir_all(&self.runtime_dir)?;
            secure_create_dir(&self.runtime_dir)?;
        }
        Ok(())
    }

    pub fn discover() -> Result<Self, CoreError> {
        let home = home_dir()?;
        #[cfg(target_os = "windows")]
        let (config_home, data_home, state_home) = {
            let roaming = std::env::var_os("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Roaming"));
            let local = std::env::var_os("LOCALAPPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home.join("AppData/Local"));
            (
                roaming.join("Crosshair"),
                local.join("Crosshair"),
                local.join("Crosshair"),
            )
        };
        #[cfg(not(target_os = "windows"))]
        let (config_home, data_home, state_home) = {
            let config = std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".config"));
            let data = std::env::var_os("XDG_DATA_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".local/share"));
            let state = std::env::var_os("XDG_STATE_HOME")
                .map(PathBuf::from)
                .filter(|p| p.is_absolute())
                .unwrap_or_else(|| home.join(".local/state"));
            (config.join(APP_ID), data.join(APP_ID), state.join(APP_ID))
        };
        #[cfg(target_os = "linux")]
        let runtime = std::env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
            .map(|p| p.join(APP_ID))
            .unwrap_or_else(|| {
                std::env::temp_dir().join(format!("{APP_ID}-{}", unsafe { libc::getuid() }))
            });
        #[cfg(target_os = "windows")]
        let runtime = std::env::var_os("LOCALAPPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join("AppData/Local"))
            .join("Crosshair/Runtime");
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        let runtime = std::env::temp_dir().join(APP_ID);

        Ok(Self {
            config_file: config_home.join("config.json"),
            socket_file: runtime.join("ipc.sock"),
            lock_file: runtime.join("daemon.lock"),
            log_file: state_home.join("crosshair.log"),
            config_dir: config_home,
            data_dir: data_home,
            state_dir: state_home,
            runtime_dir: runtime,
        })
    }
}

fn home_dir() -> Result<PathBuf, CoreError> {
    #[cfg(target_os = "windows")]
    let key = "USERPROFILE";
    #[cfg(not(target_os = "windows"))]
    let key = "HOME";
    std::env::var_os(key)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .ok_or_else(|| CoreError::Config(format!("{key} is not set to an absolute path")))
}

fn finite_clamp(value: f32, fallback: f32, lo: f32, hi: f32) -> f32 {
    if value.is_finite() {
        value.clamp(lo, hi)
    } else {
        fallback
    }
}

fn normalize_color(value: &str, fallback: &str) -> String {
    let stripped = value.trim().trim_start_matches('#');
    if stripped.len() == 6 && stripped.bytes().all(|b| b.is_ascii_hexdigit()) {
        format!("#{stripped}").to_ascii_lowercase()
    } else {
        fallback.to_string()
    }
}

fn color_to_rgb(value: &str) -> Result<[u8; 3], CoreError> {
    let clean = value.strip_prefix('#').unwrap_or(value);
    if clean.len() != 6 {
        return Err(CoreError::Config("color must be #RRGGBB".into()));
    }
    let mut rgb = [0; 3];
    for i in 0..3 {
        rgb[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16)
            .map_err(|_| CoreError::Config("invalid color".into()))?;
    }
    Ok(rgb)
}

fn rgb_to_color(bytes: &[u8]) -> String {
    format!("#{:02x}{:02x}{:02x}", bytes[0], bytes[1], bytes[2])
}

fn float_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn byte_to_float(value: u8) -> f32 {
    value as f32 / 255.0
}

fn put_i16(out: &mut Vec<u8>, value: i16) {
    out.extend(value.clamp(-500, 500).to_le_bytes());
}

fn get_i16(bytes: &[u8], offset: &mut usize) -> i16 {
    let value = i16::from_le_bytes([bytes[*offset], bytes[*offset + 1]]);
    *offset += 2;
    value
}

fn shape_index(shape: &str) -> Option<u8> {
    match shape {
        "dot" => Some(0),
        "cross" => Some(1),
        "x" => Some(2),
        "plus" => Some(3),
        "ring" | "circle" => Some(4),
        "dot_ring" => Some(5),
        "cross_dot" => Some(6),
        "x_dot" => Some(7),
        _ => None,
    }
}

fn index_shape(index: u8) -> Option<&'static str> {
    match index {
        0 => Some("dot"),
        1 => Some("cross"),
        2 => Some("x"),
        3 => Some("plus"),
        4 => Some("ring"),
        5 => Some("dot_ring"),
        6 => Some("cross_dot"),
        7 => Some("x_dot"),
        8 => Some("circle"),
        _ => None,
    }
}

fn crc16_ccitt(bytes: &[u8]) -> u16 {
    let mut crc = 0xffffu16;
    for byte in bytes {
        crc ^= (*byte as u16) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    crc
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64_url_encode(bytes: &[u8]) -> String {
    let mut result = String::with_capacity((bytes.len() * 4 + 2) / 3);
    for chunk in bytes.chunks(3) {
        let a = chunk[0] as u32;
        let b = chunk.get(1).copied().unwrap_or(0) as u32;
        let c = chunk.get(2).copied().unwrap_or(0) as u32;
        let n = (a << 16) | (b << 8) | c;
        result.push(B64[((n >> 18) & 63) as usize] as char);
        result.push(B64[((n >> 12) & 63) as usize] as char);
        if chunk.len() > 1 {
            result.push(B64[((n >> 6) & 63) as usize] as char);
        }
        if chunk.len() > 2 {
            result.push(B64[(n & 63) as usize] as char);
        }
    }
    result
}

fn base64_url_decode(value: &str) -> Result<Vec<u8>, CoreError> {
    if value.len() % 4 == 1 || value.len() > 256 {
        return Err(CoreError::Profile("invalid base64url length".into()));
    }
    let mut output = Vec::with_capacity(value.len() * 3 / 4);
    let mut accumulator = 0u32;
    let mut bits = 0u8;
    for byte in value.bytes() {
        let digit = B64
            .iter()
            .position(|candidate| *candidate == byte)
            .ok_or_else(|| CoreError::Profile("invalid character in code".into()))?
            as u32;
        accumulator = (accumulator << 6) | digit;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            output.push(((accumulator >> bits) & 0xff) as u8);
        }
    }
    if bits > 0 && (accumulator & ((1u32 << bits) - 1)) != 0 {
        return Err(CoreError::Profile("invalid trailing bits in code".into()));
    }
    Ok(output)
}

#[cfg(target_os = "linux")]
fn open_directory_linux(
    path: &Path,
    create: bool,
    private_final: bool,
) -> Result<std::os::fd::OwnedFd, CoreError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    if !path.is_absolute() {
        return Err(CoreError::Config("directory path must be absolute".into()));
    }
    let root = CString::new("/").expect("literal path");
    let root_fd = unsafe {
        libc::open(
            root.as_ptr(),
            libc::O_RDONLY | libc::O_DIRECTORY | libc::O_CLOEXEC | libc::O_NOFOLLOW,
        )
    };
    if root_fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    let mut current = unsafe { OwnedFd::from_raw_fd(root_fd) };

    for component in path.components() {
        let name = match component {
            std::path::Component::RootDir | std::path::Component::CurDir => continue,
            std::path::Component::Normal(name) => name,
            std::path::Component::ParentDir | std::path::Component::Prefix(_) => {
                return Err(CoreError::Config("unsafe directory path component".into()));
            }
        };
        let name = CString::new(name.as_bytes())
            .map_err(|_| CoreError::Config("directory path contains a NUL byte".into()))?;
        let flags = libc::O_RDONLY
            | libc::O_DIRECTORY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK;
        let mut next_fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        if next_fd < 0
            && create
            && std::io::Error::last_os_error().raw_os_error() == Some(libc::ENOENT)
        {
            let mkdir_result = unsafe { libc::mkdirat(current.as_raw_fd(), name.as_ptr(), 0o700) };
            if mkdir_result < 0 {
                let error = std::io::Error::last_os_error();
                if error.raw_os_error() != Some(libc::EEXIST) {
                    return Err(error.into());
                }
            }
            next_fd = unsafe { libc::openat(current.as_raw_fd(), name.as_ptr(), flags) };
        }
        if next_fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        current = unsafe { OwnedFd::from_raw_fd(next_fd) };
    }

    if private_final && unsafe { libc::fchmod(current.as_raw_fd(), 0o700) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(current)
}

#[cfg(target_os = "linux")]
fn read_config(path: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    use std::ffi::CString;
    use std::io::Read;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;

    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Config("config path has no parent".into()))?;
    let directory = match open_directory_linux(parent, false, false) {
        Ok(directory) => directory,
        Err(CoreError::Io(error)) if error.raw_os_error() == Some(libc::ENOENT) => {
            return Ok(None);
        }
        Err(error) => return Err(error),
    };
    let leaf = path
        .file_name()
        .ok_or_else(|| CoreError::Config("invalid config filename".into()))?;
    let leaf = CString::new(leaf.as_bytes())
        .map_err(|_| CoreError::Config("config filename contains a NUL byte".into()))?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            leaf.as_ptr(),
            libc::O_RDONLY | libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK,
        )
    };
    if fd < 0 {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() == Some(libc::ENOENT) {
            return Ok(None);
        }
        if error.raw_os_error() == Some(libc::ELOOP) {
            return Err(CoreError::Config(
                "refusing to read a symlinked config file".into(),
            ));
        }
        return Err(error.into());
    }
    let file = unsafe { std::fs::File::from_raw_fd(fd) };
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(CoreError::Config(
            "config path is not a regular file".into(),
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(CoreError::Config(
            "config exceeds 1 MiB and was left untouched".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(CoreError::Config(
            "config grew beyond 1 MiB while being read".into(),
        ));
    }
    Ok(Some(bytes))
}

#[cfg(not(target_os = "linux"))]
fn read_config(path: &Path) -> Result<Option<Vec<u8>>, CoreError> {
    use std::io::Read;
    let metadata = match path.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(CoreError::Config(
            "config path must be a regular, non-symlink file".into(),
        ));
    }
    if metadata.len() > MAX_CONFIG_BYTES as u64 {
        return Err(CoreError::Config(
            "config exceeds 1 MiB and was left untouched".into(),
        ));
    }
    let mut options = std::fs::OpenOptions::new();
    options.read(true);
    let file = options.open(path)?;
    if !file.metadata()?.is_file() {
        return Err(CoreError::Config(
            "config path is not a regular file".into(),
        ));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take((MAX_CONFIG_BYTES + 1) as u64)
        .read_to_end(&mut bytes)?;
    if bytes.len() > MAX_CONFIG_BYTES {
        return Err(CoreError::Config(
            "config grew beyond 1 MiB while being read".into(),
        ));
    }
    Ok(Some(bytes))
}

#[cfg(target_os = "linux")]
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    atomic_write_mode(path, bytes, 0o600)
}

#[cfg(target_os = "linux")]
fn atomic_write_mode(path: &Path, bytes: &[u8], file_mode: libc::mode_t) -> Result<(), CoreError> {
    use std::ffi::CString;
    use std::io::Write;
    use std::os::fd::{AsRawFd, FromRawFd};
    use std::os::unix::ffi::OsStrExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    static TEMP_ID: AtomicU64 = AtomicU64::new(0);
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Config("config path has no parent".into()))?;
    let directory = open_directory_linux(parent, true, true)?;
    let leaf = path
        .file_name()
        .ok_or_else(|| CoreError::Config("invalid config filename".into()))?;
    let leaf = CString::new(leaf.as_bytes())
        .map_err(|_| CoreError::Config("config filename contains a NUL byte".into()))?;

    // Reject symlink, FIFO, device and directory targets instead of replacing
    // or blocking on them. The parent directory descriptor pins the directory
    // inode for the complete create/write/rename operation.
    let mut existing = std::mem::MaybeUninit::<libc::stat>::uninit();
    let status = unsafe {
        libc::fstatat(
            directory.as_raw_fd(),
            leaf.as_ptr(),
            existing.as_mut_ptr(),
            libc::AT_SYMLINK_NOFOLLOW,
        )
    };
    if status == 0 {
        let metadata = unsafe { existing.assume_init() };
        let kind = metadata.st_mode & libc::S_IFMT;
        if kind == libc::S_IFLNK {
            return Err(CoreError::Config(
                "refusing to replace a symlinked file".into(),
            ));
        }
        if kind != libc::S_IFREG {
            return Err(CoreError::Config(
                "destination is not a regular file".into(),
            ));
        }
        if metadata.st_uid != unsafe { libc::geteuid() } {
            return Err(CoreError::Config(
                "refusing to replace a file not owned by this user".into(),
            ));
        }
        if metadata.st_nlink != 1 {
            return Err(CoreError::Config(
                "refusing to replace a multiply-linked config file".into(),
            ));
        }
    } else {
        let error = std::io::Error::last_os_error();
        if error.raw_os_error() != Some(libc::ENOENT) {
            return Err(error.into());
        }
    }

    let sequence = TEMP_ID.fetch_add(1, Ordering::Relaxed);
    let temporary_name = format!(
        ".{}.tmp-{}-{}",
        leaf.to_string_lossy(),
        std::process::id(),
        sequence
    );
    let temporary = CString::new(temporary_name)
        .map_err(|_| CoreError::Config("invalid temporary filename".into()))?;
    let fd = unsafe {
        libc::openat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            libc::O_WRONLY | libc::O_CREAT | libc::O_EXCL | libc::O_CLOEXEC | libc::O_NOFOLLOW,
            file_mode,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    if unsafe { libc::fchmod(fd, file_mode) } != 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::close(fd);
            libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0);
        }
        return Err(error.into());
    }
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    let write_result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = write_result {
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0);
        }
        return Err(error.into());
    }

    let rename_result = unsafe {
        libc::renameat(
            directory.as_raw_fd(),
            temporary.as_ptr(),
            directory.as_raw_fd(),
            leaf.as_ptr(),
        )
    };
    if rename_result != 0 {
        let error = std::io::Error::last_os_error();
        unsafe {
            libc::unlinkat(directory.as_raw_fd(), temporary.as_ptr(), 0);
        }
        return Err(error.into());
    }
    if unsafe { libc::fsync(directory.as_raw_fd()) } != 0 {
        return Err(std::io::Error::last_os_error().into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    use std::io::Write;
    let parent = path
        .parent()
        .ok_or_else(|| CoreError::Config("config path has no parent".into()))?;
    secure_create_dir(parent)?;
    match path.symlink_metadata() {
        Ok(metadata) => {
            if metadata.file_type().is_symlink() || !metadata.is_file() {
                return Err(CoreError::Config(
                    "refusing a non-regular config destination".into(),
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }

    let name = path
        .file_name()
        .ok_or_else(|| CoreError::Config("invalid config filename".into()))?;
    let temporary = parent.join(format!(
        ".{}.tmp-{}-{}",
        name.to_string_lossy(),
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    ));
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.share_mode(0);
    }
    let mut file = options.open(&temporary)?;
    let result = file.write_all(bytes).and_then(|()| file.sync_all());
    drop(file);
    if let Err(error) = result {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let from: Vec<u16> = temporary.as_os_str().encode_wide().chain(Some(0)).collect();
        let to: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let moved = unsafe {
            windows_sys::Win32::Storage::FileSystem::MoveFileExW(
                from.as_ptr(),
                to.as_ptr(),
                windows_sys::Win32::Storage::FileSystem::MOVEFILE_REPLACE_EXISTING
                    | windows_sys::Win32::Storage::FileSystem::MOVEFILE_WRITE_THROUGH,
            )
        };
        if moved == 0 {
            let error = std::io::Error::last_os_error();
            let _ = std::fs::remove_file(&temporary);
            return Err(error.into());
        }
    }
    #[cfg(not(windows))]
    if let Err(error) = std::fs::rename(&temporary, path) {
        let _ = std::fs::remove_file(&temporary);
        return Err(error.into());
    }
    Ok(())
}

#[cfg(not(target_os = "linux"))]
fn secure_create_dir(path: &Path) -> Result<(), CoreError> {
    std::fs::create_dir_all(path)?;
    match path.symlink_metadata() {
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => Ok(()),
        Ok(_) => Err(CoreError::Config("unsafe config directory".into())),
        Err(error) => Err(error.into()),
    }
}

/// Preflight every destination before a multi-file install starts. Parent
/// directories are created/pinned without symlink traversal and made private;
/// existing targets must be single-link regular files owned by this user.
#[cfg(target_os = "linux")]
pub fn validate_user_file_targets(paths: &[&Path]) -> Result<(), CoreError> {
    use std::ffi::CString;
    use std::os::fd::AsRawFd;
    use std::os::unix::ffi::OsStrExt;

    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::Config("file path has no parent".into()))?;
        let directory = match open_directory_linux(parent, false, false) {
            Ok(directory) => directory,
            Err(CoreError::Io(error)) if error.raw_os_error() == Some(libc::ENOENT) => continue,
            Err(error) => return Err(error),
        };
        let mut parent_metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(directory.as_raw_fd(), parent_metadata.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let parent_metadata = unsafe { parent_metadata.assume_init() };
        if parent_metadata.st_uid != unsafe { libc::geteuid() } {
            return Err(CoreError::Config(
                "install destination directory is not owned by this user".into(),
            ));
        }

        let leaf = path
            .file_name()
            .ok_or_else(|| CoreError::Config("invalid filename".into()))?;
        let leaf = CString::new(leaf.as_bytes())
            .map_err(|_| CoreError::Config("filename contains a NUL byte".into()))?;

        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let status = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                leaf.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            return Err(error.into());
        }

        let metadata = unsafe { metadata.assume_init() };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(CoreError::Config(
                "refusing to replace a symlink or non-regular file".into(),
            ));
        }
        if metadata.st_uid != unsafe { libc::geteuid() } {
            return Err(CoreError::Config(
                "refusing to replace a file not owned by this user".into(),
            ));
        }
        if metadata.st_nlink != 1 {
            return Err(CoreError::Config(
                "refusing to replace a multiply-linked file".into(),
            ));
        }
    }
    Ok(())
}

/// Atomically write a user-owned application file using pinned directory
/// handles, no-follow traversal and owner-private file/directory permissions.
#[cfg(target_os = "linux")]
pub fn atomic_write_user_file(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    atomic_write(path, bytes)
}

/// Atomically install an owner-only executable (0700) under a private
/// directory. Suitable for per-user application binaries in XDG_BIN_HOME.
#[cfg(target_os = "linux")]
pub fn atomic_write_user_executable(path: &Path, bytes: &[u8]) -> Result<(), CoreError> {
    atomic_write_mode(path, bytes, 0o700)
}

/// Remove one user-owned regular file without following directory or file
/// symlinks. Hard-linked and non-regular destinations are refused.
#[cfg(target_os = "linux")]
pub fn remove_user_file(path: &Path) -> Result<bool, CoreError> {
    Ok(remove_user_files(&[path])? > 0)
}

/// Validate every target before removing any of them. Each parent directory is
/// opened without following symlinks and pinned until its target is unlinked.
/// Shared/group-writable parents and files not owned solely by this user are
/// rejected. Missing parents/files are treated as already removed.
#[cfg(target_os = "linux")]
pub fn remove_user_files(paths: &[&Path]) -> Result<usize, CoreError> {
    use std::ffi::CString;
    use std::os::fd::{AsRawFd, OwnedFd};
    use std::os::unix::ffi::OsStrExt;

    let mut targets: Vec<(OwnedFd, CString)> = Vec::new();
    for path in paths {
        let parent = path
            .parent()
            .ok_or_else(|| CoreError::Config("file path has no parent".into()))?;
        let directory = match open_directory_linux(parent, false, false) {
            Ok(directory) => directory,
            Err(CoreError::Io(error)) if error.raw_os_error() == Some(libc::ENOENT) => {
                continue;
            }
            Err(error) => return Err(error),
        };

        let mut directory_metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(directory.as_raw_fd(), directory_metadata.as_mut_ptr()) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let directory_metadata = unsafe { directory_metadata.assume_init() };
        if directory_metadata.st_mode & libc::S_IFMT != libc::S_IFDIR
            || directory_metadata.st_uid != unsafe { libc::geteuid() }
            || directory_metadata.st_mode & 0o022 != 0
        {
            return Err(CoreError::Config(
                "refusing removal from a directory not privately controlled by this user".into(),
            ));
        }

        let leaf = path
            .file_name()
            .ok_or_else(|| CoreError::Config("invalid filename".into()))?;
        let leaf = CString::new(leaf.as_bytes())
            .map_err(|_| CoreError::Config("filename contains a NUL byte".into()))?;

        let mut metadata = std::mem::MaybeUninit::<libc::stat>::uninit();
        let status = unsafe {
            libc::fstatat(
                directory.as_raw_fd(),
                leaf.as_ptr(),
                metadata.as_mut_ptr(),
                libc::AT_SYMLINK_NOFOLLOW,
            )
        };
        if status != 0 {
            let error = std::io::Error::last_os_error();
            if error.raw_os_error() == Some(libc::ENOENT) {
                continue;
            }
            return Err(error.into());
        }

        let metadata = unsafe { metadata.assume_init() };
        if metadata.st_mode & libc::S_IFMT != libc::S_IFREG {
            return Err(CoreError::Config(
                "refusing to remove a symlink or non-regular file".into(),
            ));
        }
        if metadata.st_uid != unsafe { libc::geteuid() } {
            return Err(CoreError::Config(
                "refusing to remove a file not owned by this user".into(),
            ));
        }
        if metadata.st_nlink != 1 {
            return Err(CoreError::Config(
                "refusing to remove a multiply-linked file".into(),
            ));
        }

        targets.push((directory, leaf));
    }

    let count = targets.len();
    for (directory, leaf) in targets {
        if unsafe { libc::unlinkat(directory.as_raw_fd(), leaf.as_ptr(), 0) } != 0 {
            return Err(std::io::Error::last_os_error().into());
        }
    }
    Ok(count)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_python_config_loads_and_receives_defaults_for_new_fields() {
        let old = r##"{"enabled":false,"shape":"cross","opacity":0.9,"size":8,"thickness":2,"gap":4,"color":"#12abef","outline":{"enabled":true,"color":"#000000","opacity":0.8,"thickness":1},"shadow":{"enabled":false,"color":"#000000","opacity":0.35,"offset_x":1,"offset_y":1},"monitor_mode":"name","monitor_index":0,"monitor_name":"DP-1","monitor_name_ordinal":1,"preview_bg":"#222222","auto_save":true}"##;
        let config = Config::from_json(old).unwrap();
        assert_eq!(config.shape, "cross");
        assert_eq!(config.reference_dot, ReferenceDot::default());
        assert_eq!(config.offset_x, 0);
    }

    #[test]
    fn default_profile_code_round_trips() {
        let config = Config::default();
        let code = config.export_code().unwrap();
        assert!(code.starts_with("CH1-"));
        assert_eq!(
            code.len(),
            52,
            "profile codes should remain compact and copyable"
        );
        let mut imported = Config::default();
        imported.import_code(&code).unwrap();
        assert_eq!(config.color, imported.color);
        assert_eq!(config.shape, imported.shape);
        assert_eq!(config.reference_dot.enabled, imported.reference_dot.enabled);
        assert_eq!(config.reference_dot.size, imported.reference_dot.size);
        assert_eq!(
            config.reference_dot.offset_x,
            imported.reference_dot.offset_x
        );
        assert_eq!(
            config.reference_dot.offset_y,
            imported.reference_dot.offset_y
        );
        assert!((config.reference_dot.opacity - imported.reference_dot.opacity).abs() < 0.005);
    }

    #[test]
    fn profile_round_trip_preserves_appearance_but_not_machine_preferences() {
        let mut a = Config::default();
        a.shape = "x_dot".into();
        a.color = "#12abef".into();
        a.opacity = 0.73;
        a.size = 22;
        a.thickness = 7;
        a.gap = 10;
        a.offset_x = -27;
        a.offset_y = 49;
        a.shadow.enabled = true;
        a.shadow.offset_x = -8;
        a.shadow.offset_y = 15;
        a.reference_dot.enabled = true;
        a.reference_dot.offset_y = 123;
        let code = a.export_code().unwrap();

        let mut b = Config::default();
        b.enabled = false;
        b.monitor_mode = "name".into();
        b.monitor_name = "DP-2".into();
        b.import_code(&code).unwrap();

        assert_eq!(b.shape, a.shape);
        assert_eq!(b.color, a.color);
        assert_eq!(b.offset_x, a.offset_x);
        assert_eq!(b.reference_dot.offset_y, a.reference_dot.offset_y);
        assert!(!b.enabled);
        assert_eq!(b.monitor_mode, "name");
        assert_eq!(b.monitor_name, "DP-2");
        assert!((b.opacity - a.opacity).abs() < 0.005);
    }

    #[test]
    fn profile_detects_typo_before_mutating_settings() {
        let mut target = Config::default();
        let original = target.clone();
        let mut code = target.export_code().unwrap();
        code.pop();
        assert!(target.import_code(&code).is_err());
        assert_eq!(target, original);
    }

    #[test]
    fn profile_does_not_accept_invalid_shape_or_corrupt_checksum() {
        let config = Config::default();
        let code = config.export_code().unwrap();
        let mut changed = code.into_bytes();
        let last = changed.len() - 1;
        changed[last] = if changed[last] == b'A' { b'B' } else { b'A' };
        let changed = String::from_utf8(changed).unwrap();
        assert!(Config::default().import_code(&changed).is_err());
    }

    #[test]
    fn normalize_rejects_non_finite_opacity_and_bad_colors() {
        let mut config = Config::default();
        config.opacity = f32::NAN;
        config.color = "bad".into();
        config.normalize();
        assert_eq!(config.opacity, 0.95);
        assert_eq!(config.color, "#a000ff");
    }

    #[test]
    fn save_and_load_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crosshair/config.json");
        let config = Config::default();
        config.save_atomic(&path).unwrap();
        assert_eq!(Config::load(&path).unwrap(), config);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn config_directory_and_file_are_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("private/config.json");
        Config::default().save_atomic(&path).unwrap();
        assert_eq!(
            path.parent()
                .unwrap()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn user_file_write_is_atomic_private_and_removable() {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let executable = dir.path().join("bin/crosshair");
        atomic_write_user_executable(&executable, b"#!/bin/sh\nexit 0\n").unwrap();
        assert_eq!(
            executable.metadata().unwrap().permissions().mode() & 0o777,
            0o700
        );

        let path = dir.path().join("config/autostart/crosshair.desktop");
        atomic_write_user_file(&path, b"[Desktop Entry]\nName=Crosshair\n").unwrap();

        assert_eq!(
            path.parent()
                .unwrap()
                .metadata()
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(path.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"[Desktop Entry]\nName=Crosshair\n"
        );

        atomic_write_user_file(&path, b"[Desktop Entry]\nName=Updated\n").unwrap();
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"[Desktop Entry]\nName=Updated\n"
        );
        assert!(remove_user_file(&path).unwrap());
        assert!(!remove_user_file(&path).unwrap());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn user_file_removal_refuses_symlinks_and_hard_links() {
        use std::os::unix::fs::symlink;

        let dir = tempfile::tempdir().unwrap();
        let parent = dir.path().join("autostart");
        std::fs::create_dir(&parent).unwrap();

        let target = dir.path().join("outside.desktop");
        std::fs::write(&target, b"preserve").unwrap();
        let linked = parent.join("crosshair.desktop");
        symlink(&target, &linked).unwrap();
        assert!(remove_user_file(&linked).is_err());
        assert!(linked.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&target).unwrap(), b"preserve");
        std::fs::remove_file(&linked).unwrap();

        atomic_write_user_file(&linked, b"owned").unwrap();
        let second = dir.path().join("second-link");
        std::fs::hard_link(&linked, &second).unwrap();
        assert!(remove_user_file(&linked).is_err());
        assert_eq!(std::fs::read(&linked).unwrap(), b"owned");
        assert_eq!(std::fs::read(&second).unwrap(), b"owned");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn user_file_batch_removal_preflights_all_targets_and_rejects_shared_directories() {
        use std::os::unix::fs::{PermissionsExt, symlink};

        let dir = tempfile::tempdir().unwrap();
        let first_dir = dir.path().join("first");
        let second_dir = dir.path().join("second");
        std::fs::create_dir(&first_dir).unwrap();
        std::fs::create_dir(&second_dir).unwrap();
        let first = first_dir.join("crosshair");
        let outside = dir.path().join("outside");
        let second = second_dir.join("autostart.desktop");
        std::fs::write(&first, b"keep-until-all-validated").unwrap();
        std::fs::write(&outside, b"outside").unwrap();
        symlink(&outside, &second).unwrap();

        assert!(remove_user_files(&[&first, &second]).is_err());
        assert_eq!(std::fs::read(&first).unwrap(), b"keep-until-all-validated");
        assert!(second.symlink_metadata().unwrap().file_type().is_symlink());
        assert_eq!(std::fs::read(&outside).unwrap(), b"outside");

        std::fs::remove_file(&second).unwrap();
        std::fs::write(&second, b"owned").unwrap();
        std::fs::set_permissions(&second_dir, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(remove_user_file(&second).is_err());
        assert_eq!(std::fs::read(&second).unwrap(), b"owned");
    }

    #[cfg(unix)]
    #[test]
    fn config_reads_and_writes_refuse_symlink_targets() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("outside.json");
        std::fs::write(&target, b"{\"sentinel\":\"preserve\"}").unwrap();
        let config_path = dir.path().join("crosshair/config.json");
        std::fs::create_dir_all(config_path.parent().unwrap()).unwrap();
        symlink(&target, &config_path).unwrap();

        assert!(Config::load(&config_path).is_err());
        assert!(Config::default().save_atomic(&config_path).is_err());
        assert_eq!(
            std::fs::read_to_string(target).unwrap(),
            "{\"sentinel\":\"preserve\"}"
        );
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn config_refuses_symlinked_parent_components() {
        use std::os::unix::fs::symlink;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let alias = dir.path().join("alias");
        std::fs::create_dir(&real).unwrap();
        symlink(&real, &alias).unwrap();
        let path = alias.join("crosshair/config.json");

        assert!(Config::default().save_atomic(&path).is_err());
        assert!(!real.join("crosshair").exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn config_refuses_non_regular_files_without_blocking() {
        use std::ffi::CString;
        use std::os::fd::AsRawFd;
        use std::os::unix::ffi::OsStrExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crosshair/config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let name = CString::new(path.file_name().unwrap().as_bytes()).unwrap();
        let parent = open_directory_linux(path.parent().unwrap(), true, true).unwrap();
        let status = unsafe { libc::mkfifoat(parent.as_raw_fd(), name.as_ptr(), 0o600) };
        assert_eq!(status, 0);
        assert!(Config::load(&path).is_err());
        assert!(Config::default().save_atomic(&path).is_err());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn oversized_config_is_rejected_and_left_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("crosshair/config.json");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let oversized = vec![b' '; MAX_CONFIG_BYTES + 1];
        std::fs::write(&path, &oversized).unwrap();
        assert!(Config::load(&path).is_err());
        assert_eq!(
            std::fs::metadata(&path).unwrap().len(),
            (MAX_CONFIG_BYTES + 1) as u64
        );
    }
}

#[cfg(test)]
mod ipc_framing_tests {
    use super::{MAX_IPC_MESSAGE_BYTES, read_bounded_line};
    use std::io::Cursor;

    #[test]
    fn accepts_a_bounded_newline_terminated_request() {
        let request = br#"{"cmd":"status"}"#.to_vec();
        let mut expected = request;
        expected.push(b'\n');

        assert_eq!(
            read_bounded_line(Cursor::new(expected.clone())).unwrap(),
            expected
        );
    }

    #[test]
    fn accepts_a_request_exactly_at_the_size_limit() {
        let mut request = br#"{"cmd":"status"}"#.to_vec();
        request.resize(MAX_IPC_MESSAGE_BYTES - 1, b' ');
        request.push(b'\n');
        assert_eq!(request.len(), MAX_IPC_MESSAGE_BYTES);

        assert_eq!(
            read_bounded_line(Cursor::new(request.clone())).unwrap(),
            request
        );
    }

    #[test]
    fn rejects_an_oversized_request_even_when_its_json_prefix_is_valid() {
        let mut request = br#"{"cmd":"status"}"#.to_vec();
        request.resize(MAX_IPC_MESSAGE_BYTES + 1, b' ');

        let error = read_bounded_line(Cursor::new(request)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("exceeds 64 KiB"));
    }

    #[test]
    fn rejects_a_complete_json_object_without_its_newline_delimiter() {
        let request = br#"{"cmd":"status"}"#.to_vec();

        let error = read_bounded_line(Cursor::new(request)).unwrap_err();
        assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
        assert!(error.to_string().contains("missing its newline delimiter"));
    }
}
