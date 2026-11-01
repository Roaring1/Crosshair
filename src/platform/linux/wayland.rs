use calloop::{EventLoop, Interest, Mode, PostAction, generic::Generic};
use calloop_wayland_source::WaylandSource;
use crosshair_core::render::Raster;
use crosshair_core::{APP_VERSION, AppPaths, Config};
use smithay_client_toolkit::{
    compositor::{CompositorHandler, CompositorState},
    delegate_compositor, delegate_layer, delegate_output, delegate_registry, delegate_shm,
    output::{OutputHandler, OutputState},
    registry::{ProvidesRegistryState, RegistryState},
    registry_handlers,
    shell::{
        WaylandSurface,
        wlr_layer::{
            Anchor, KeyboardInteractivity, Layer, LayerShell, LayerShellHandler, LayerSurface,
            LayerSurfaceConfigure,
        },
    },
    shm::{Shm, ShmHandler, slot::SlotPool},
};
use std::{
    collections::HashMap,
    fs::{File, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{FileTypeExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    time::Duration,
};
#[path = "output_layout.rs"]
mod output_layout;

use wayland_client::{
    Connection, QueueHandle,
    globals::registry_queue_init,
    protocol::{wl_output, wl_region, wl_shm, wl_surface},
};

pub fn run(config: Config, paths: AppPaths) -> Result<(), String> {
    let _lock = acquire_daemon_lock(&paths)?;
    let conn = Connection::connect_to_env()
        .map_err(|error| format!("Could not connect to the Wayland compositor: {error}"))?;
    let (globals, mut queue) = registry_queue_init(&conn)
        .map_err(|error| format!("Could not read Wayland globals: {error}"))?;
    let qh = queue.handle();

    let compositor = CompositorState::bind(&globals, &qh)
        .map_err(|error| format!("Wayland compositor protocol unavailable: {error}"))?;
    let layer_shell = LayerShell::bind(&globals, &qh)
        .map_err(|_| "This Wayland compositor does not advertise wlr-layer-shell; no unsafe normal-window fallback will be used".to_string())?;
    let shm = Shm::bind(&globals, &qh)
        .map_err(|error| format!("Wayland shared-memory buffers unavailable: {error}"))?;

    let listener = prepare_listener(&paths)?;
    let _socket_guard = SocketPathGuard {
        path: paths.socket_file.clone(),
    };
    let mut state = Runtime {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
        compositor,
        layer_shell,
        shm,
        pool: None,
        config,
        paths: paths.clone(),
        _lock,
        surfaces: HashMap::new(),
        outputs_dirty: true,
        redraw_dirty: false,
        quit: false,
        first_error: None,
    };

    queue
        .roundtrip(&mut state)
        .map_err(|error| format!("Initial Wayland roundtrip failed: {error}"))?;
    let mut event_loop: EventLoop<'static, Runtime> =
        EventLoop::try_new().map_err(|error| format!("Could not create event loop: {error}"))?;
    let handle = event_loop.handle();

    WaylandSource::new(conn.clone(), queue)
        .insert(handle.clone())
        .map_err(|_| "Could not register Wayland connection with event loop".to_string())?;

    handle
        .insert_source(
            Generic::new(listener, Interest::READ, Mode::Level),
            |_, listener, runtime| {
                loop {
                    match listener.accept() {
                        Ok((stream, _)) => {
                            if let Err(error) = handle_request(stream, runtime) {
                                runtime.first_error = Some(error);
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(error) => {
                            runtime.first_error = Some(format!("IPC accept failed: {error}"));
                            break;
                        }
                    }
                }
                Ok(PostAction::Continue)
            },
        )
        .map_err(|_| "Could not register the Crosshair control socket".to_string())?;

    state.reconcile_outputs(&qh)?;

    while !state.quit {
        event_loop
            .dispatch(None, &mut state)
            .map_err(|error| format!("Crosshair event loop failed: {error}"))?;
        if let Some(error) = state.first_error.take() {
            eprintln!("Crosshair: {error}");
        }
        if state.outputs_dirty {
            state.reconcile_outputs(&qh)?;
        } else if state.redraw_dirty {
            state.redraw_existing()?;
        }
        if !state.config.enabled && state.surfaces.is_empty() {
            state.quit = true;
        }
    }

    state.surfaces.clear();
    conn.flush().ok();
    drop(_socket_guard);
    Ok(())
}

struct SurfaceEntry {
    layer: LayerSurface,
    name: String,
    raster: Raster,
    buffer: Option<smithay_client_toolkit::shm::slot::Buffer>,
}

struct Runtime {
    registry_state: RegistryState,
    output_state: OutputState,
    compositor: CompositorState,
    layer_shell: LayerShell,
    shm: Shm,
    pool: Option<SlotPool>,
    config: Config,
    paths: AppPaths,
    _lock: File,
    surfaces: HashMap<u32, SurfaceEntry>,
    outputs_dirty: bool,
    redraw_dirty: bool,
    quit: bool,
    first_error: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct OutputSelection {
    id: u32,
    name: String,
    position: (i32, i32),
}

fn select_output_ids(config: &Config, outputs: &[OutputSelection]) -> Result<Vec<u32>, String> {
    let mut ordered = outputs.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|output| (output.position, output.id));

    let selected = match config.monitor_mode.as_str() {
        // wl_output does not define a standard primary-output flag. In the absence of
        // a compositor-specific adapter, the top-left-most output is the deterministic fallback.
        "primary" => ordered
            .into_iter()
            .take(1)
            .map(|output| output.id)
            .collect(),
        "name" => ordered
            .into_iter()
            .filter(|output| output.name == config.monitor_name)
            .nth(config.monitor_name_ordinal as usize)
            .map(|output| vec![output.id])
            .unwrap_or_default(),
        "index" => ordered
            .into_iter()
            .nth(config.monitor_index as usize)
            .map(|output| vec![output.id])
            .unwrap_or_default(),
        "active" => {
            return Err("Cursor-active monitor selection is not exposed to ordinary Wayland clients on this compositor. Choose all, primary, or a named monitor.".into());
        }
        _ => ordered.into_iter().map(|output| output.id).collect(),
    };
    Ok(selected)
}

impl Runtime {
    fn selected_outputs(
        &self,
    ) -> Result<
        Vec<(
            wl_output::WlOutput,
            smithay_client_toolkit::output::OutputInfo,
        )>,
        String,
    > {
        let mut outputs = self
            .output_state
            .outputs()
            .filter_map(|output| self.output_state.info(&output).map(|info| (output, info)))
            .collect::<Vec<_>>();
        outputs.sort_by_key(|(_, info)| (info.logical_position.unwrap_or(info.location), info.id));
        let candidates = outputs
            .iter()
            .map(|(_, info)| OutputSelection {
                id: info.id,
                name: info
                    .name
                    .as_deref()
                    .filter(|name| !name.is_empty())
                    .map(str::to_string)
                    .unwrap_or_else(|| format!("Output {}", info.id)),
                position: info.logical_position.unwrap_or(info.location),
            })
            .collect::<Vec<_>>();
        let selected_ids = select_output_ids(&self.config, &candidates)?;
        Ok(outputs
            .into_iter()
            .filter(|(_, info)| selected_ids.contains(&info.id))
            .collect())
    }

    fn reconcile_outputs(&mut self, qh: &QueueHandle<Self>) -> Result<(), String> {
        self.outputs_dirty = false;
        self.surfaces.clear();
        if !self.config.enabled {
            return Ok(());
        }

        let selected = self.selected_outputs()?;
        if selected.is_empty() {
            return Err("No display matched the selected Crosshair monitor.".into());
        }
        for (output, info) in selected {
            let layout = output_layout::build(&self.config, &info)?;
            let surface = self.compositor.create_surface(qh);
            let layer = self.layer_shell.create_layer_surface(
                qh,
                surface,
                Layer::Overlay,
                Some("crosshair"),
                Some(&output),
            );
            layer.set_anchor(Anchor::TOP | Anchor::LEFT);
            layer.set_keyboard_interactivity(KeyboardInteractivity::None);
            layer.set_exclusive_zone(-1);
            layer.set_size(layout.logical_width, layout.logical_height);
            layer.set_buffer_scale(layout.scale).ok();
            layer.set_margin(layout.top, 0, 0, layout.left);

            // An empty input region keeps the overlay completely click-through.
            let empty_region = self.compositor.wl_compositor().create_region(qh, ());
            layer.wl_surface().set_input_region(Some(&empty_region));
            layer.commit();
            empty_region.destroy();

            self.surfaces.insert(
                info.id,
                SurfaceEntry {
                    layer,
                    name: layout.name,
                    raster: layout.raster,
                    buffer: None,
                },
            );
        }
        self.redraw_dirty = false;
        Ok(())
    }

    fn redraw_existing(&mut self) -> Result<(), String> {
        self.redraw_dirty = false;
        if !self.config.enabled {
            self.outputs_dirty = true;
            return Ok(());
        }
        let selected = self.selected_outputs()?;
        if selected.len() != self.surfaces.len()
            || selected
                .iter()
                .any(|(_, info)| !self.surfaces.contains_key(&info.id))
        {
            self.outputs_dirty = true;
            return Ok(());
        }

        for (_output, info) in selected {
            let layout = output_layout::build(&self.config, &info)?;
            let Some(entry) = self.surfaces.get_mut(&info.id) else {
                self.outputs_dirty = true;
                return Ok(());
            };
            entry.name = layout.name;
            entry.raster = layout.raster;
            entry
                .layer
                .set_size(layout.logical_width, layout.logical_height);
            entry.layer.set_buffer_scale(layout.scale).ok();
            entry.layer.set_margin(layout.top, 0, 0, layout.left);
            self.render_entry(info.id)?;
        }
        Ok(())
    }

    fn render_entry(&mut self, output_id: u32) -> Result<(), String> {
        let raster = self
            .surfaces
            .get(&output_id)
            .ok_or_else(|| "Overlay surface disappeared during render".to_string())?
            .raster
            .clone();
        if self.pool.is_none() {
            self.pool = Some(
                SlotPool::new(raster.bgra_premultiplied.len().max(4), &self.shm)
                    .map_err(|error| format!("Could not allocate overlay buffer pool: {error}"))?,
            );
        }
        let pool = self.pool.as_mut().expect("pool initialized");
        let (buffer, canvas) = pool
            .create_buffer(
                raster.width as i32,
                raster.height as i32,
                raster.stride(),
                wl_shm::Format::Argb8888,
            )
            .map_err(|error| format!("Could not allocate crosshair surface: {error}"))?;
        canvas.copy_from_slice(&raster.bgra_premultiplied);
        let entry = self
            .surfaces
            .get_mut(&output_id)
            .ok_or_else(|| "Overlay surface disappeared during attach".to_string())?;
        buffer
            .attach_to(entry.layer.wl_surface())
            .map_err(|error| format!("Could not attach crosshair pixels: {error}"))?;
        entry
            .layer
            .wl_surface()
            .damage_buffer(0, 0, raster.width as i32, raster.height as i32);
        entry.layer.commit();
        entry.buffer = Some(buffer);
        Ok(())
    }

    fn render_configure(&mut self, layer: &LayerSurface) -> Result<(), String> {
        let Some(output_id) = self.surfaces.iter().find_map(|(id, entry)| {
            if entry.layer == *layer {
                Some(*id)
            } else {
                None
            }
        }) else {
            return Ok(());
        };
        self.render_entry(output_id)
    }
}

impl CompositorHandler for Runtime {
    fn scale_factor_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: i32,
    ) {
        self.outputs_dirty = true;
    }
    fn transform_changed(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: wl_output::Transform,
    ) {
        self.outputs_dirty = true;
    }
    fn frame(&mut self, _: &Connection, _: &QueueHandle<Self>, _: &wl_surface::WlSurface, _: u32) {}
    fn surface_enter(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
    fn surface_leave(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        _: &wl_surface::WlSurface,
        _: &wl_output::WlOutput,
    ) {
    }
}

impl OutputHandler for Runtime {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }
    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_dirty = true;
    }
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_dirty = true;
    }
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {
        self.outputs_dirty = true;
    }
}

impl LayerShellHandler for Runtime {
    fn closed(&mut self, _: &Connection, _: &QueueHandle<Self>, layer: &LayerSurface) {
        self.surfaces.retain(|_, entry| entry.layer != *layer);
        if self.surfaces.is_empty() {
            self.quit = true;
        }
    }

    fn configure(
        &mut self,
        _: &Connection,
        _: &QueueHandle<Self>,
        layer: &LayerSurface,
        _: LayerSurfaceConfigure,
        _: u32,
    ) {
        if let Err(error) = self.render_configure(layer) {
            self.first_error = Some(error);
            self.quit = true;
        }
    }
}

impl ShmHandler for Runtime {
    fn shm_state(&mut self) -> &mut Shm {
        &mut self.shm
    }
}

impl ProvidesRegistryState for Runtime {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }
    registry_handlers![OutputState];
}

delegate_compositor!(Runtime);
delegate_output!(Runtime);
delegate_shm!(Runtime);
delegate_layer!(Runtime);
delegate_registry!(Runtime);

impl wayland_client::Dispatch<wl_region::WlRegion, ()> for Runtime {
    fn event(
        _state: &mut Self,
        _proxy: &wl_region::WlRegion,
        _event: wl_region::Event,
        _data: &(),
        _conn: &Connection,
        _qh: &QueueHandle<Self>,
    ) {
    }
}

pub(super) fn acquire_daemon_lock(paths: &AppPaths) -> Result<File, String> {
    paths
        .ensure_runtime_dir()
        .map_err(|error| error.to_string())?;
    let mut options = OpenOptions::new();
    options
        .read(true)
        .write(true)
        .create(true)
        .mode(0o600)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW | libc::O_NONBLOCK);
    let file = options
        .open(&paths.lock_file)
        .map_err(|error| format!("Could not open the daemon lock: {error}"))?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.uid() != unsafe { libc::getuid() } || metadata.nlink() != 1 {
        return Err("Daemon lock is not a regular, singly-linked file owned by this user".into());
    }
    if unsafe {
        libc::flock(
            std::os::fd::AsRawFd::as_raw_fd(&file),
            libc::LOCK_EX | libc::LOCK_NB,
        )
    } != 0
    {
        return Err("Crosshair overlay is already running".into());
    }
    Ok(file)
}

pub(super) struct SocketPathGuard {
    pub(super) path: std::path::PathBuf,
}

impl Drop for SocketPathGuard {
    fn drop(&mut self) {
        if let Ok(metadata) = self.path.symlink_metadata() {
            if metadata.file_type().is_socket() && metadata.uid() == unsafe { libc::getuid() } {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

pub(super) fn prepare_listener(paths: &AppPaths) -> Result<UnixListener, String> {
    paths
        .ensure_runtime_dir()
        .map_err(|error| error.to_string())?;
    match paths.socket_file.symlink_metadata() {
        Ok(meta) if meta.file_type().is_socket() && meta.uid() == unsafe { libc::getuid() } => {
            std::fs::remove_file(&paths.socket_file)
                .map_err(|error| format!("Could not remove stale control socket: {error}"))?;
        }
        Ok(_) => return Err("Refusing to remove an unsafe control socket path".into()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let listener = UnixListener::bind(&paths.socket_file)
        .map_err(|error| format!("Could not create control socket: {error}"))?;
    std::fs::set_permissions(&paths.socket_file, std::fs::Permissions::from_mode(0o600))
        .map_err(|error| format!("Could not secure control socket: {error}"))?;
    listener
        .set_nonblocking(true)
        .map_err(|error| format!("Could not configure control socket: {error}"))?;
    Ok(listener)
}

fn peer_is_current_user(stream: &UnixStream) -> bool {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            std::os::fd::AsRawFd::as_raw_fd(stream),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 || size as usize != std::mem::size_of::<libc::ucred>() {
        return false;
    }
    let credentials = unsafe { credentials.assume_init() };
    credentials.uid == unsafe { libc::getuid() }
}

fn handle_request(mut stream: UnixStream, runtime: &mut Runtime) -> Result<(), String> {
    if !peer_is_current_user(&stream) {
        return Err("Rejected IPC client with a different user ID".into());
    }
    stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(2))).ok();
    let mut line = Vec::with_capacity(1024);
    {
        let reader = BufReader::new(&stream);
        reader
            .take(64 * 1024)
            .read_until(b'\n', &mut line)
            .map_err(|error| format!("Could not read IPC request: {error}"))?;
    }
    if line.len() > 64 * 1024 {
        return Err("IPC request exceeds 64 KiB".into());
    }
    let request: serde_json::Value =
        serde_json::from_slice(&line).map_err(|_| "Invalid JSON in IPC request".to_string())?;
    let command = request
        .get("cmd")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "IPC request is missing a command".to_string())?;
    let mut response = serde_json::json!({"ok": true});
    match command {
        "status" => {
            response["enabled"] = serde_json::Value::Bool(runtime.config.enabled);
            response["version"] = serde_json::Value::String(APP_VERSION.into());
            response["monitors"] = serde_json::Value::Array(
                runtime
                    .surfaces
                    .values()
                    .map(|entry| serde_json::Value::String(entry.name.clone()))
                    .collect(),
            );
        }
        "toggle" => {
            runtime.config.enabled = !runtime.config.enabled;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| format!("Could not save Crosshair state: {error}"))?;
            runtime.outputs_dirty = true;
        }
        "show" => {
            runtime.config.enabled = true;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| format!("Could not save Crosshair state: {error}"))?;
            runtime.outputs_dirty = true;
        }
        "hide" => {
            runtime.config.enabled = false;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| format!("Could not save Crosshair state: {error}"))?;
            runtime.outputs_dirty = true;
        }
        "preview" | "set" => {
            let payload = request
                .get("payload")
                .ok_or_else(|| "IPC settings request is missing a payload".to_string())?;
            let encoded = serde_json::to_string(payload).map_err(|error| error.to_string())?;
            let candidate = Config::from_json(&encoded).map_err(|error| error.to_string())?;
            if candidate.monitor_mode == "active" {
                response = serde_json::json!({
                    "ok": false,
                    "message": "Cursor-active monitor targeting is not exposed to ordinary Wayland clients by this compositor. Choose all, primary, or a named monitor."
                });
            } else {
                let placement_changed = runtime.config.enabled != candidate.enabled
                    || runtime.config.monitor_mode != candidate.monitor_mode
                    || runtime.config.monitor_index != candidate.monitor_index
                    || runtime.config.monitor_name != candidate.monitor_name
                    || runtime.config.monitor_name_ordinal != candidate.monitor_name_ordinal;
                if command == "set" {
                    candidate
                        .save_atomic(&runtime.paths.config_file)
                        .map_err(|error| format!("Could not save Crosshair settings: {error}"))?;
                }
                runtime.config = candidate;
                if placement_changed {
                    runtime.outputs_dirty = true;
                } else {
                    runtime.redraw_dirty = true;
                }
            }
        }
        "reload" => {
            let candidate = Config::load(&runtime.paths.config_file)
                .map_err(|error| format!("Could not reload config: {error}"))?;
            let placement_changed = runtime.config.enabled != candidate.enabled
                || runtime.config.monitor_mode != candidate.monitor_mode
                || runtime.config.monitor_index != candidate.monitor_index
                || runtime.config.monitor_name != candidate.monitor_name
                || runtime.config.monitor_name_ordinal != candidate.monitor_name_ordinal;
            runtime.config = candidate;
            if placement_changed {
                runtime.outputs_dirty = true;
            } else {
                runtime.redraw_dirty = true;
            }
        }
        "quit" => runtime.quit = true,
        "restart" => {
            response["restart"] = serde_json::Value::Bool(true);
            runtime.quit = true;
        }
        _ => {
            response = serde_json::json!({"ok": false, "message": "Unknown IPC command"});
        }
    }
    let mut bytes = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| format!("Could not send IPC response: {error}"))?;
    Ok(())
}

pub fn list_outputs() -> Result<Vec<crosshair_core::MonitorDescriptor>, String> {
    let conn = Connection::connect_to_env()
        .map_err(|error| format!("Could not connect to Wayland to list monitors: {error}"))?;
    let (globals, mut queue) = registry_queue_init(&conn)
        .map_err(|error| format!("Could not read Wayland globals: {error}"))?;
    let qh = queue.handle();
    let mut state = OutputQuery {
        registry_state: RegistryState::new(&globals),
        output_state: OutputState::new(&globals, &qh),
    };
    queue
        .roundtrip(&mut state)
        .map_err(|error| format!("Could not enumerate Wayland outputs: {error}"))?;

    let mut outputs = state
        .output_state
        .outputs()
        .filter_map(|output| state.output_state.info(&output))
        .collect::<Vec<_>>();
    outputs.sort_by_key(|info| (info.logical_position.unwrap_or(info.location), info.id));
    let mut occurrences = HashMap::<String, u8>::new();
    let mut result = Vec::with_capacity(outputs.len());
    for info in outputs {
        let name = info
            .name
            .filter(|name| !name.is_empty())
            .unwrap_or_else(|| format!("Output {}", info.id));
        let ordinal = occurrences.entry(name.clone()).or_insert(0);
        result.push(crosshair_core::MonitorDescriptor {
            name,
            ordinal: *ordinal,
        });
        *ordinal = ordinal.saturating_add(1);
    }
    Ok(result)
}

struct OutputQuery {
    registry_state: RegistryState,
    output_state: OutputState,
}

impl OutputHandler for OutputQuery {
    fn output_state(&mut self) -> &mut OutputState {
        &mut self.output_state
    }

    fn new_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn update_output(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
    fn output_destroyed(&mut self, _: &Connection, _: &QueueHandle<Self>, _: wl_output::WlOutput) {}
}

impl ProvidesRegistryState for OutputQuery {
    fn registry(&mut self) -> &mut RegistryState {
        &mut self.registry_state
    }

    registry_handlers![OutputState];
}

delegate_output!(OutputQuery);
delegate_registry!(OutputQuery);

#[cfg(test)]
mod output_selection_tests {
    use super::*;

    fn output(id: u32, name: &str, x: i32, y: i32) -> OutputSelection {
        OutputSelection {
            id,
            name: name.into(),
            position: (x, y),
        }
    }

    fn fixture_outputs() -> Vec<OutputSelection> {
        vec![
            output(3, "HDMI-A-2", 1920, 0),
            output(2, "DP-1", -1920, 0),
            output(1, "DP-1", 0, 0),
        ]
    }

    #[test]
    fn all_outputs_are_returned_in_stable_position_then_id_order() {
        let mut config = Config::default();
        config.monitor_mode = "all".into();

        assert_eq!(
            select_output_ids(&config, &fixture_outputs()).unwrap(),
            [2, 1, 3]
        );
    }

    #[test]
    fn primary_fallback_is_the_top_left_most_output() {
        let mut config = Config::default();
        config.monitor_mode = "primary".into();

        assert_eq!(select_output_ids(&config, &fixture_outputs()).unwrap(), [2]);
    }

    #[test]
    fn index_selection_uses_the_same_stable_output_order() {
        let mut config = Config::default();
        config.monitor_mode = "index".into();
        config.monitor_index = 1;

        assert_eq!(select_output_ids(&config, &fixture_outputs()).unwrap(), [1]);
    }

    #[test]
    fn duplicate_connector_names_use_the_saved_ordinal_after_position_sorting() {
        let mut config = Config::default();
        config.monitor_mode = "name".into();
        config.monitor_name = "DP-1".into();
        config.monitor_name_ordinal = 1;

        assert_eq!(select_output_ids(&config, &fixture_outputs()).unwrap(), [1]);
    }

    #[test]
    fn missing_named_output_returns_no_match_instead_of_another_monitor() {
        let mut config = Config::default();
        config.monitor_mode = "name".into();
        config.monitor_name = "DP-3".into();

        assert!(
            select_output_ids(&config, &fixture_outputs())
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn cursor_active_selection_fails_with_an_actionable_wayland_message() {
        let mut config = Config::default();
        config.monitor_mode = "active".into();

        let error = select_output_ids(&config, &fixture_outputs()).unwrap_err();
        assert!(error.contains("Cursor-active monitor selection"));
        assert!(error.contains("named monitor"));
    }
}
