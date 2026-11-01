use crosshair_core::render::{Raster, render_crosshair};
use crosshair_core::{APP_VERSION, AppPaths, Config};
use std::{
    collections::HashMap,
    io::{BufRead, BufReader, Read, Write},
    os::fd::AsRawFd,
    os::unix::net::UnixStream,
    time::Duration,
};
use x11rb::{
    connection::Connection as _,
    properties::WmHints,
    protocol::{
        Event, randr, render, shape,
        xproto::{
            self, AtomEnum, ClipOrdering, ConnectionExt as _, CreateWindowAux, EventMask,
            ImageFormat, PropMode, StackMode, WindowClass,
        },
    },
    rust_connection::RustConnection,
    wrapper::ConnectionExt as WrapperExt,
};

#[derive(Clone, Debug)]
struct Output {
    id: u32,
    name: String,
    primary: bool,
    x: i32,
    y: i32,
    width: u32,
    height: u32,
}

struct Surface {
    window: u32,
    gc: u32,
    colormap: u32,
    raster: Raster,
    name: String,
}

struct X11Runtime {
    conn: RustConnection,
    screen_num: usize,
    root: u32,
    argb_visual: u32,
    argb_depth: u8,
    config: Config,
    paths: AppPaths,
    surfaces: HashMap<u32, Surface>,
    outputs: Vec<Output>,
    active_output: Option<u32>,
    outputs_dirty: bool,
    redraw_dirty: bool,
    quit: bool,
}

impl X11Runtime {
    fn enumerate_outputs(&self) -> Result<Vec<Output>, String> {
        let cookie = randr::get_monitors(&self.conn, self.root, true)
            .map_err(|error| format!("Could not request RandR monitors: {error}"))?;
        let reply = cookie
            .reply()
            .map_err(|error| format!("Could not read RandR monitor reply: {error}"))?;
        let mut outputs = Vec::new();
        for (index, monitor) in reply.monitors.into_iter().enumerate() {
            let name = self
                .conn
                .get_atom_name(monitor.name)
                .map_err(|error| format!("Could not request monitor name: {error}"))?
                .reply()
                .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
                .unwrap_or_else(|_| format!("Monitor {}", index + 1));
            outputs.push(Output {
                id: index as u32,
                name,
                primary: monitor.primary,
                x: monitor.x as i32,
                y: monitor.y as i32,
                width: monitor.width as u32,
                height: monitor.height as u32,
            });
        }
        outputs.sort_by_key(|output| (output.x, output.y, output.id));
        if outputs.is_empty() {
            let screen = &self.conn.setup().roots[self.screen_num];
            outputs.push(Output {
                id: 0,
                name: "Screen 1".into(),
                primary: true,
                x: 0,
                y: 0,
                width: screen.width_in_pixels as u32,
                height: screen.height_in_pixels as u32,
            });
        }
        Ok(outputs)
    }

    fn pointer_output(&self, outputs: &[Output]) -> Option<u32> {
        let pointer = xproto::query_pointer(&self.conn, self.root)
            .ok()?
            .reply()
            .ok()?;
        outputs
            .iter()
            .find(|output| {
                pointer.root_x as i32 >= output.x
                    && pointer.root_y as i32 >= output.y
                    && (pointer.root_x as i32) < output.x + output.width as i32
                    && (pointer.root_y as i32) < output.y + output.height as i32
            })
            .map(|output| output.id)
    }

    fn selected_outputs(&mut self) -> Result<Vec<Output>, String> {
        self.outputs = self.enumerate_outputs()?;
        if self.config.monitor_mode == "active" {
            self.active_output = self.pointer_output(&self.outputs);
        }
        let selected = match self.config.monitor_mode.as_str() {
            "primary" => self
                .outputs
                .iter()
                .find(|output| output.primary)
                .or_else(|| self.outputs.first())
                .cloned()
                .into_iter()
                .collect(),
            "name" => {
                let ordinal = self.config.monitor_name_ordinal as usize;
                self.outputs
                    .iter()
                    .filter(|output| output.name == self.config.monitor_name)
                    .nth(ordinal)
                    .cloned()
                    .into_iter()
                    .collect()
            }
            "index" => self
                .outputs
                .get(self.config.monitor_index as usize)
                .cloned()
                .into_iter()
                .collect(),
            "active" => self
                .active_output
                .and_then(|id| self.outputs.iter().find(|output| output.id == id).cloned())
                .into_iter()
                .collect(),
            _ => self.outputs.clone(),
        };
        if selected.is_empty() {
            return Err(format!(
                "No X11 monitor matched selection mode '{}' (name '{}').",
                self.config.monitor_mode, self.config.monitor_name
            ));
        }
        Ok(selected)
    }

    fn reconcile(&mut self) -> Result<(), String> {
        self.outputs_dirty = false;
        self.destroy_surfaces();
        if !self.config.enabled {
            return Ok(());
        }
        let selected = self.selected_outputs()?;
        for output in selected {
            let raster = render_crosshair(&self.config).map_err(|error| error.to_string())?;
            let width = u16::try_from(raster.width)
                .map_err(|_| "Crosshair surface width exceeds X11 limits".to_string())?;
            let height = u16::try_from(raster.height)
                .map_err(|_| "Crosshair surface height exceeds X11 limits".to_string())?;
            let x = output.x + (output.width / 2) as i32 + raster.origin_x;
            let y = output.y + (output.height / 2) as i32 + raster.origin_y;
            let x = x.clamp(i16::MIN as i32, i16::MAX as i32) as i16;
            let y = y.clamp(i16::MIN as i32, i16::MAX as i32) as i16;

            let colormap = self.conn.generate_id().map_err(|error| error.to_string())?;
            xproto::create_colormap(
                &self.conn,
                xproto::ColormapAlloc::NONE,
                colormap,
                self.root,
                self.argb_visual,
            )
            .map_err(|error| error.to_string())?;
            let window = self.conn.generate_id().map_err(|error| error.to_string())?;
            let attributes = CreateWindowAux::new()
                .background_pixel(0)
                .border_pixel(0)
                .colormap(colormap)
                .event_mask(EventMask::EXPOSURE | EventMask::STRUCTURE_NOTIFY);
            xproto::create_window(
                &self.conn,
                self.argb_depth,
                window,
                self.root,
                x,
                y,
                width,
                height,
                0,
                WindowClass::INPUT_OUTPUT,
                self.argb_visual,
                &attributes,
            )
            .map_err(|error| format!("Could not create ARGB overlay window: {error}"))?;

            let gc = self.conn.generate_id().map_err(|error| error.to_string())?;
            xproto::create_gc(&self.conn, gc, window, &xproto::CreateGCAux::new())
                .map_err(|error| error.to_string())?;

            // ICCCM input=false tells a window manager not to focus this utility.
            WmHints {
                input: Some(false),
                ..WmHints::new()
            }
            .set(&self.conn, window)
            .map_err(|error| error.to_string())?;

            let wm_name = intern(&self.conn, "WM_NAME")?;
            self.conn
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    wm_name,
                    AtomEnum::STRING,
                    b"Crosshair",
                )
                .map_err(|error| error.to_string())?;
            let wm_class = intern(&self.conn, "WM_CLASS")?;
            self.conn
                .change_property8(
                    PropMode::REPLACE,
                    window,
                    wm_class,
                    AtomEnum::STRING,
                    b"crosshair\0Crosshair\0",
                )
                .map_err(|error| error.to_string())?;
            let utility = intern(&self.conn, "_NET_WM_WINDOW_TYPE_UTILITY")?;
            let window_type = intern(&self.conn, "_NET_WM_WINDOW_TYPE")?;
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    window_type,
                    AtomEnum::ATOM,
                    &[utility],
                )
                .map_err(|error| error.to_string())?;
            let state_atom = intern(&self.conn, "_NET_WM_STATE")?;
            let above = intern(&self.conn, "_NET_WM_STATE_ABOVE")?;
            let skip_taskbar = intern(&self.conn, "_NET_WM_STATE_SKIP_TASKBAR")?;
            let skip_pager = intern(&self.conn, "_NET_WM_STATE_SKIP_PAGER")?;
            self.conn
                .change_property32(
                    PropMode::REPLACE,
                    window,
                    state_atom,
                    AtomEnum::ATOM,
                    &[above, skip_taskbar, skip_pager],
                )
                .map_err(|error| error.to_string())?;

            // Shape INPUT to an empty region: pointer clicks and wheel events pass through.
            shape::rectangles(
                &self.conn,
                shape::SO::SET,
                shape::SK::INPUT,
                ClipOrdering::UNSORTED,
                window,
                0,
                0,
                &[],
            )
            .map_err(|error| error.to_string())?;

            xproto::map_window(&self.conn, window).map_err(|error| error.to_string())?;
            xproto::configure_window(
                &self.conn,
                window,
                &xproto::ConfigureWindowAux::new().stack_mode(StackMode::ABOVE),
            )
            .map_err(|error| error.to_string())?;
            let name = output.name.clone();
            self.surfaces.insert(
                output.id,
                Surface {
                    window,
                    gc,
                    colormap,
                    raster,
                    name,
                },
            );
        }
        self.conn.flush().map_err(|error| error.to_string())?;
        for id in self.surfaces.keys().copied().collect::<Vec<_>>() {
            self.draw_surface(id)?;
        }
        Ok(())
    }

    fn destroy_surfaces(&mut self) {
        for (_, surface) in self.surfaces.drain() {
            let _ = xproto::destroy_window(&self.conn, surface.window);
            let _ = xproto::free_gc(&self.conn, surface.gc);
            let _ = xproto::free_colormap(&self.conn, surface.colormap);
        }
    }

    fn draw_surface(&self, output_id: u32) -> Result<(), String> {
        let surface = self
            .surfaces
            .get(&output_id)
            .ok_or_else(|| "Crosshair output surface disappeared".to_string())?;
        let raster = &surface.raster;
        let bytes_per_row = raster.width as usize * 4;
        let mut converted;
        let data = if self.conn.setup().image_byte_order == xproto::ImageOrder::LSB_FIRST {
            &raster.bgra_premultiplied
        } else {
            converted = vec![0u8; raster.bgra_premultiplied.len()];
            for (source, target) in raster
                .bgra_premultiplied
                .chunks_exact(4)
                .zip(converted.chunks_exact_mut(4))
            {
                target.copy_from_slice(&[source[3], source[2], source[1], source[0]]);
            }
            &converted
        };
        // Keep requests comfortably below the core protocol maximum. Each request
        // updates a few scanlines of the same small, static crosshair buffer.
        const ROWS_PER_REQUEST: usize = 12;
        for row in (0..raster.height as usize).step_by(ROWS_PER_REQUEST) {
            let rows = (raster.height as usize - row).min(ROWS_PER_REQUEST);
            let start = row * bytes_per_row;
            let end = start + rows * bytes_per_row;
            xproto::put_image(
                &self.conn,
                ImageFormat::Z_PIXMAP,
                surface.window,
                surface.gc,
                raster.width as u16,
                rows as u16,
                0,
                row as i16,
                0,
                self.argb_depth,
                &data[start..end],
            )
            .map_err(|error| format!("Could not draw Crosshair pixels: {error}"))?;
        }
        self.conn.flush().map_err(|error| error.to_string())?;
        Ok(())
    }

    fn redraw(&mut self) -> Result<(), String> {
        self.redraw_dirty = false;
        if !self.config.enabled {
            self.outputs_dirty = true;
            return Ok(());
        }
        let selected = self.selected_outputs()?;
        if selected.len() != self.surfaces.len()
            || selected
                .iter()
                .any(|output| !self.surfaces.contains_key(&output.id))
        {
            self.outputs_dirty = true;
            return Ok(());
        }
        for output in selected {
            let raster = render_crosshair(&self.config).map_err(|error| error.to_string())?;
            let x = output.x + (output.width / 2) as i32 + raster.origin_x;
            let y = output.y + (output.height / 2) as i32 + raster.origin_y;
            let width = raster.width as u16;
            let height = raster.height as u16;
            if let Some(surface) = self.surfaces.get_mut(&output.id) {
                xproto::configure_window(
                    &self.conn,
                    surface.window,
                    &xproto::ConfigureWindowAux::new()
                        .x(x)
                        .y(y)
                        .width(width as u32)
                        .height(height as u32),
                )
                .map_err(|error| error.to_string())?;
                surface.raster = raster;
            }
            self.draw_surface(output.id)?;
        }
        Ok(())
    }
}

fn intern(conn: &RustConnection, name: &str) -> Result<u32, String> {
    conn.intern_atom(false, name.as_bytes())
        .map_err(|error| format!("Could not request X11 atom {name}: {error}"))?
        .reply()
        .map(|reply| reply.atom)
        .map_err(|error| format!("Could not read X11 atom {name}: {error}"))
}

pub fn run(config: Config, paths: AppPaths) -> Result<(), String> {
    let _lock = crate::wayland::acquire_daemon_lock(&paths)?;
    let (conn, screen_num) =
        x11rb::connect(None).map_err(|error| format!("Could not connect to X11: {error}"))?;
    let screen = &conn.setup().roots[screen_num];
    let root = screen.root;

    let pict_cookie = render::query_pict_formats(&conn)
        .map_err(|error| format!("X11 RENDER extension is not available: {error}"))?;
    let pict = pict_cookie
        .reply()
        .map_err(|error| format!("Could not read X11 RENDER formats: {error}"))?;
    let pict_screen = pict
        .screens
        .get(screen_num)
        .ok_or_else(|| "X11 RENDER did not return formats for this screen".to_string())?;
    let argb_format = pict
        .formats
        .iter()
        .find(|format| {
            format.type_ == render::PictType::DIRECT
                && format.depth == 32
                && format.direct.red_shift == 16
                && format.direct.red_mask == 0xff
                && format.direct.green_shift == 8
                && format.direct.green_mask == 0xff
                && format.direct.blue_shift == 0
                && format.direct.blue_mask == 0xff
                && format.direct.alpha_shift == 24
                && format.direct.alpha_mask == 0xff
        })
        .ok_or_else(|| "X11 server does not offer a standard 32-bit ARGB visual.".to_string())?;
    let argb_visual = pict_screen
        .depths
        .iter()
        .filter(|depth| depth.depth == 32)
        .flat_map(|depth| depth.visuals.iter())
        .find(|visual| visual.format == argb_format.id)
        .map(|visual| visual.visual)
        .ok_or_else(|| {
            "X11 server has ARGB_32 picture format but no matching depth-32 visual.".to_string()
        })?;

    let randr_cookie = randr::query_version(&conn, 1, 5)
        .map_err(|error| format!("Could not initialize RandR monitor support: {error}"))?;
    let randr_version = randr_cookie
        .reply()
        .map_err(|error| format!("Could not read RandR version: {error}"))?;
    let _ = randr_version;
    let _ = randr::select_input(
        &conn,
        root,
        randr::NotifyMask::SCREEN_CHANGE
            | randr::NotifyMask::CRTC_CHANGE
            | randr::NotifyMask::OUTPUT_CHANGE
            | randr::NotifyMask::OUTPUT_PROPERTY
            | randr::NotifyMask::RESOURCE_CHANGE,
    );

    let listener = crate::wayland::prepare_listener(&paths)?;
    let _socket_guard = crate::wayland::SocketPathGuard {
        path: paths.socket_file.clone(),
    };
    let mut runtime = X11Runtime {
        conn,
        screen_num,
        root,
        argb_visual,
        argb_depth: 32,
        config,
        paths,
        surfaces: HashMap::new(),
        outputs: Vec::new(),
        active_output: None,
        outputs_dirty: true,
        redraw_dirty: false,
        quit: false,
    };
    runtime.reconcile()?;

    while !runtime.quit {
        let active_mode = runtime.config.monitor_mode == "active";
        let timeout_ms = if active_mode { 400 } else { -1 };
        let mut descriptors = [
            libc::pollfd {
                fd: runtime.conn.stream().as_raw_fd(),
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: std::os::fd::AsRawFd::as_raw_fd(&listener),
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let polled =
            unsafe { libc::poll(descriptors.as_mut_ptr(), descriptors.len() as _, timeout_ms) };
        if polled < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                continue;
            }
            return Err(format!("X11 event wait failed: {error}"));
        }
        if polled == 0 && active_mode {
            if runtime.pointer_output(&runtime.outputs) != runtime.active_output {
                runtime.outputs_dirty = true;
            }
        }
        if descriptors[1].revents & libc::POLLIN != 0 {
            loop {
                match listener.accept() {
                    Ok((stream, _)) => handle_request(stream, &mut runtime)?,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(error) => return Err(format!("X11 IPC accept failed: {error}")),
                }
            }
        }
        if descriptors[0].revents & libc::POLLIN != 0 {
            while let Some(event) = runtime
                .conn
                .poll_for_event()
                .map_err(|error| error.to_string())?
            {
                match event {
                    Event::RandrNotify(_) | Event::RandrScreenChangeNotify(_) => {
                        runtime.outputs_dirty = true
                    }
                    Event::Expose(expose) => {
                        if expose.count == 0 {
                            if let Some(id) = runtime.surfaces.iter().find_map(|(id, surface)| {
                                if surface.window == expose.window {
                                    Some(*id)
                                } else {
                                    None
                                }
                            }) {
                                runtime.draw_surface(id)?;
                            }
                        }
                    }
                    _ => {}
                }
            }
        }
        if runtime.outputs_dirty {
            runtime.reconcile()?;
        } else if runtime.redraw_dirty {
            runtime.redraw()?;
        }
        if !runtime.config.enabled && runtime.surfaces.is_empty() {
            runtime.quit = true;
        }
    }

    runtime.destroy_surfaces();
    runtime.conn.flush().ok();
    drop(_socket_guard);
    Ok(())
}

fn peer_is_current_user(stream: &UnixStream) -> bool {
    let mut credentials = std::mem::MaybeUninit::<libc::ucred>::uninit();
    let mut size = std::mem::size_of::<libc::ucred>() as libc::socklen_t;
    let result = unsafe {
        libc::getsockopt(
            stream.as_raw_fd(),
            libc::SOL_SOCKET,
            libc::SO_PEERCRED,
            credentials.as_mut_ptr().cast(),
            &mut size,
        )
    };
    if result != 0 || size as usize != std::mem::size_of::<libc::ucred>() {
        return false;
    }
    unsafe { credentials.assume_init() }.uid == unsafe { libc::getuid() }
}

fn handle_request(mut stream: UnixStream, runtime: &mut X11Runtime) -> Result<(), String> {
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
                    .map(|surface| serde_json::Value::String(surface.name.clone()))
                    .collect(),
            );
        }
        "toggle" => {
            runtime.config.enabled = !runtime.config.enabled;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| error.to_string())?;
            runtime.outputs_dirty = true;
        }
        "show" => {
            runtime.config.enabled = true;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| error.to_string())?;
            runtime.outputs_dirty = true;
        }
        "hide" => {
            runtime.config.enabled = false;
            runtime
                .config
                .save_atomic(&runtime.paths.config_file)
                .map_err(|error| error.to_string())?;
            runtime.outputs_dirty = true;
        }
        "preview" | "set" => {
            let payload = request
                .get("payload")
                .ok_or_else(|| "Missing settings payload".to_string())?;
            let encoded = serde_json::to_string(payload).map_err(|error| error.to_string())?;
            let candidate = Config::from_json(&encoded).map_err(|error| error.to_string())?;
            let placement_changed = runtime.config.enabled != candidate.enabled
                || runtime.config.monitor_mode != candidate.monitor_mode
                || runtime.config.monitor_index != candidate.monitor_index
                || runtime.config.monitor_name != candidate.monitor_name
                || runtime.config.monitor_name_ordinal != candidate.monitor_name_ordinal;
            if command == "set" {
                candidate
                    .save_atomic(&runtime.paths.config_file)
                    .map_err(|error| error.to_string())?;
            }
            runtime.config = candidate;
            if placement_changed {
                runtime.outputs_dirty = true;
            } else {
                runtime.redraw_dirty = true;
            }
        }
        "reload" => {
            let candidate =
                Config::load(&runtime.paths.config_file).map_err(|error| error.to_string())?;
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
        "quit" | "restart" => runtime.quit = true,
        _ => response = serde_json::json!({"ok": false, "message": "Unknown IPC command"}),
    }
    let mut encoded = serde_json::to_vec(&response).map_err(|error| error.to_string())?;
    encoded.push(b'\n');
    stream
        .write_all(&encoded)
        .map_err(|error| error.to_string())?;
    Ok(())
}

pub fn list_outputs() -> Result<Vec<crosshair_core::MonitorDescriptor>, String> {
    let (conn, screen_num) = x11rb::connect(None)
        .map_err(|error| format!("Could not connect to X11 to list monitors: {error}"))?;
    let root = conn.setup().roots[screen_num].root;
    let cookie = randr::get_monitors(&conn, root, true)
        .map_err(|error| format!("Could not request RandR monitors: {error}"))?;
    let reply = cookie
        .reply()
        .map_err(|error| format!("Could not read RandR monitor list: {error}"))?;
    let mut named_outputs = Vec::with_capacity(reply.monitors.len());
    for (index, monitor) in reply.monitors.into_iter().enumerate() {
        let name = conn
            .get_atom_name(monitor.name)
            .map_err(|error| format!("Could not request monitor name: {error}"))?
            .reply()
            .map(|reply| String::from_utf8_lossy(&reply.name).into_owned())
            .unwrap_or_else(|_| format!("Monitor {}", index + 1));
        named_outputs.push((monitor.x, monitor.y, name));
    }
    named_outputs.sort_by_key(|(x, y, _)| (*x, *y));

    let mut occurrences = HashMap::<String, u8>::new();
    let mut result = Vec::with_capacity(named_outputs.len());
    for (_, _, name) in named_outputs {
        let ordinal = occurrences.entry(name.clone()).or_insert(0);
        result.push(crosshair_core::MonitorDescriptor {
            name,
            ordinal: *ordinal,
        });
        *ordinal = ordinal.saturating_add(1);
    }
    if result.is_empty() {
        result.push(crosshair_core::MonitorDescriptor {
            name: format!("Screen {}", screen_num + 1),
            ordinal: 0,
        });
    }
    Ok(result)
}
