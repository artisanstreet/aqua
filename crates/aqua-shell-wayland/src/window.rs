use crate::decoration::{self, Frame, Negotiation};
use crate::lease::{SurfaceCore, SurfaceGeometry, SurfaceLease};
use crate::looping::{emit, Waker};
use crate::output::Monitor;
use crate::scale::{Scale, Scaling};
use crate::state::{self, Shell, Spec, WindowState};
use crate::toplevel::{self, Role, States};
use crate::unsupported::Unsupported;
use aqua_core::{ClickSettings, CursorIcon, Point};
use aqua_protocol::chrome::DragRects;
use aqua_protocol::{DragRegions, MaterialCapability, WindowId, WindowMaterial};
use aqua_shell::{Chrome, ShellError, ShellEvent, WindowKind, WindowOptions};
use std::sync::{Arc, Mutex, Once};
use std::time::Duration;
use wayland_client::delegate_noop;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;
use wayland_protocols::wp::viewporter::client::wp_viewport::WpViewport;

const CONFIGURE_LIMIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Window {
    id: WindowId,
}

pub(crate) fn default_click_settings() -> ClickSettings {
    ClickSettings::new(Duration::from_millis(400), Point::new(5.0, 5.0)).unwrap_or_default()
}

fn executable_stem() -> String {
    std::env::current_exe()
        .ok()
        .and_then(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .unwrap_or_else(|| "aqua".into())
}

static TOPMOST_LOGGED: Once = Once::new();
static POSITION_LOGGED: Once = Once::new();

fn rounded(logical: (f64, f64)) -> (i32, i32) {
    (
        logical.0.round().max(1.0) as i32,
        logical.1.round().max(1.0) as i32,
    )
}

fn initial_frame(shell: &Shell, spec: &Spec) -> Frame {
    match shell.globals.decoration.is_some() {
        true => match decoration::requested(spec) {
            wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::Mode::ServerSide => {
                Frame::Server
            }
            _ => Frame::Client { band: None },
        },
        false => decoration::negotiated(spec, Negotiation::Unavailable),
    }
}

fn insert(shell: &mut Shell, options: &WindowOptions) -> WindowId {
    let id = WindowId::new(shell.next_window).expect("window ids start at one and only grow");
    shell.next_window += 1;
    let qh = shell.qh.clone();
    let compositor = shell
        .globals
        .compositor
        .as_ref()
        .expect("connect refuses compositors without wl_compositor");
    let surface = compositor.create_surface(&qh, id);
    let (viewport, fractional) = match (&shell.globals.viewporter, &shell.globals.fractional) {
        (Some(viewporter), Some(manager)) => (
            Some(viewporter.get_viewport(&surface, &qh, ())),
            Some(manager.get_fractional_scale(&surface, &qh, id)),
        ),
        _ => (None, None),
    };
    let logical = rounded(options.logical_size);
    let scaling = match viewport.is_some() {
        true => Scaling::Viewport,
        false => Scaling::BufferScale(1),
    };
    let core = Arc::new(SurfaceCore {
        conn: shell.conn.clone(),
        surface,
        viewport,
        fractional,
        presentation: shell.globals.presentation.clone(),
        geometry: Mutex::new(SurfaceGeometry {
            logical: (logical.0 as u32, logical.1 as u32),
            physical: (logical.0 as u32, logical.1 as u32),
            scale: 1.0,
            scaling,
            refresh: None,
        }),
        window: id,
        commits: std::sync::atomic::AtomicU64::new(0),
    });
    let spec = Spec {
        title: options.title.clone(),
        app_id: options
            .app_id
            .as_ref()
            .map_or_else(executable_stem, |app_id| app_id.as_str().to_owned()),
        chrome: options.chrome,
        kind: options.kind,
        min_logical: options.min_logical_size.map(rounded),
        resizable: options.resizable && options.kind == WindowKind::Normal,
        minimizable: options.minimizable,
        maximizable: options.maximizable,
        alpha_surface: options.alpha_surface,
        owner: options.owner,
    };
    let frame = initial_frame(shell, &spec);
    shell.windows.insert(
        id,
        WindowState {
            id,
            core,
            role: Role::Unmapped,
            spec,
            logical,
            restore: logical,
            scale: Scale {
                fractional: None,
                preferred_buffer: None,
                value: 1.0,
            },
            states: States::default(),
            frame,
            outputs: Vec::new(),
            regions: DragRegions::default(),
            rects: DragRects::default(),
            cursor: CursorIcon::Default,
            redraw_pending: true,
            covered: false,
            starved: false,
            probe: crate::occlusion::Probe::Idle,
            announced: None,
            file_drop: false,
        },
    );
    toplevel::rescale(shell, id);
    id
}

pub(crate) fn toggle_maximize_of(shell: &mut Shell, id: WindowId) {
    if let Some(window) = shell.windows.get(&id)
        && let Some(toplevel) = window.role.toplevel()
    {
        match window.states.maximized {
            true => toplevel.toplevel.unset_maximized(),
            false => toplevel.toplevel.set_maximized(),
        }
    }
}

pub(crate) fn minimize_of(shell: &mut Shell, id: WindowId) -> Result<(), Unsupported> {
    let Some(toplevel) = shell
        .windows
        .get(&id)
        .and_then(|window| window.role.toplevel())
    else {
        return Ok(());
    };
    match toplevel.capabilities {
        Some(capabilities) if !capabilities.minimize => Err(Unsupported::Minimize),
        _ => {
            toplevel.toplevel.set_minimized();
            Ok(())
        }
    }
}

impl Window {
    pub fn create(options: &WindowOptions) -> Result<Window, ShellError> {
        state::ensure()?;
        if options.topmost {
            TOPMOST_LOGGED.call_once(|| {
                eprintln!("aqua: {}", Unsupported::Topmost);
            });
        }
        if options.position.is_some() {
            POSITION_LOGGED.call_once(|| {
                eprintln!("aqua: {}", Unsupported::Position);
            });
        }
        let id = state::with(|shell| insert(shell, options));
        let window = Window { id };
        match options.visible {
            true => window.show()?,
            false => {
                state::with(|shell| toplevel::announce(shell, id));
            }
        }
        Ok(window)
    }

    pub fn id(self) -> WindowId {
        self.id
    }

    pub fn enable_file_drop(self) {
        state::window(self.id, |window| window.file_drop = true);
    }

    pub fn waker(self) -> Waker {
        Waker::new()
    }

    pub fn scale(self) -> f64 {
        state::window(self.id, |window| window.scale.value).unwrap_or(1.0)
    }

    pub fn inner_size(self) -> (u32, u32) {
        state::window(self.id, |window| window.physical()).unwrap_or((0, 0))
    }

    pub fn logical_size(self) -> (u32, u32) {
        state::window(self.id, |window| {
            (window.logical.0 as u32, window.logical.1 as u32)
        })
        .unwrap_or((0, 0))
    }

    pub fn outer_rect(self) -> Result<(i32, i32, i32, i32), Unsupported> {
        Err(Unsupported::Position)
    }

    pub fn monitor(self) -> Option<Monitor> {
        state::try_with(|shell| {
            let entered = shell
                .windows
                .get(&self.id)
                .map(|window| window.outputs.clone())?;
            crate::output::monitor_of(shell, &entered)
        })
        .flatten()
    }

    pub fn refresh_interval(self) -> Option<Duration> {
        self.monitor().and_then(|monitor| monitor.refresh)
    }

    pub fn click_settings(self) -> ClickSettings {
        default_click_settings()
    }

    pub fn set_title(self, title: &str) {
        state::window(self.id, |window| {
            window.spec.title = title.to_owned();
            if let Some(toplevel) = window.role.toplevel() {
                toplevel.toplevel.set_title(title.to_owned());
            }
        });
    }

    pub fn title(self) -> String {
        state::window(self.id, |window| window.spec.title.clone()).unwrap_or_default()
    }

    pub fn request_redraw(self) {
        state::window(self.id, |window| window.redraw_pending = true);
    }

    pub fn set_drag_regions(self, regions: &DragRegions) {
        state::window(self.id, |window| {
            window.rects = aqua_shell::regions::drag_rects(regions, window.scale.value);
            window.regions = regions.clone();
        });
        state::try_with(|shell| crate::input::cursor_changed(shell, self.id));
    }

    pub fn set_cursor(self, cursor: CursorIcon) {
        state::window(self.id, |window| window.cursor = cursor);
        state::try_with(|shell| crate::input::cursor_changed(shell, self.id));
    }

    pub fn set_material(self, material: WindowMaterial) -> MaterialCapability {
        crate::capability::material_capability(material)
    }

    pub fn set_dark(self, _dark: bool) -> bool {
        false
    }

    pub fn set_topmost(self, topmost: bool) -> Result<(), Unsupported> {
        match topmost {
            true => Err(Unsupported::Topmost),
            false => Ok(()),
        }
    }

    pub fn set_position(self, _x: i32, _y: i32) -> Result<(), Unsupported> {
        Err(Unsupported::Position)
    }

    pub fn send_to_back(self) -> Result<(), Unsupported> {
        Err(Unsupported::Lowering)
    }

    pub fn set_raw_mouse(self, enabled: bool) -> Result<(), Unsupported> {
        match enabled {
            true => Err(Unsupported::RawMouse),
            false => Ok(()),
        }
    }

    pub fn set_logical_size(self, width: f64, height: f64) -> Result<(), Unsupported> {
        let constrained = state::window(self.id, |window| window.states.constrained());
        match constrained {
            Some(true) => Err(Unsupported::ConstrainedSize),
            None => Ok(()),
            Some(false) => {
                state::with(|shell| {
                    if let Some(window) = shell.windows.get_mut(&self.id) {
                        window.logical = rounded((width, height));
                        window.restore = window.logical;
                        if !window.spec.resizable
                            && let Some(toplevel) = window.role.toplevel()
                        {
                            toplevel
                                .toplevel
                                .set_min_size(window.logical.0, window.logical.1);
                            toplevel
                                .toplevel
                                .set_max_size(window.logical.0, window.logical.1);
                        }
                    }
                    toplevel::announce(shell, self.id);
                });
                Ok(())
            }
        }
    }

    pub fn minimize(self) -> Result<(), Unsupported> {
        state::try_with(|shell| minimize_of(shell, self.id)).unwrap_or(Ok(()))
    }

    pub fn restore_minimized(self) -> Result<(), Unsupported> {
        Err(Unsupported::Unminimize)
    }

    pub fn toggle_maximize(self) {
        state::try_with(|shell| toggle_maximize_of(shell, self.id));
    }

    pub fn set_fullscreen(self, fullscreen: bool) {
        state::window(self.id, |window| {
            if let Some(toplevel) = window.role.toplevel() {
                match fullscreen {
                    true => toplevel.toplevel.set_fullscreen(None),
                    false => toplevel.toplevel.unset_fullscreen(),
                }
            }
        });
    }

    pub fn is_maximized(self) -> bool {
        state::window(self.id, |window| window.states.maximized).unwrap_or(false)
    }

    pub fn is_fullscreen(self) -> bool {
        state::window(self.id, |window| window.states.fullscreen).unwrap_or(false)
    }

    pub fn is_foreground(self) -> bool {
        state::window(self.id, |window| window.states.activated).unwrap_or(false)
    }

    pub fn is_visible(self) -> bool {
        state::window(self.id, |window| matches!(window.role, Role::Mapped(_))).unwrap_or(false)
    }

    pub fn is_covered(self) -> bool {
        state::window(self.id, |window| window.covered).unwrap_or(false)
    }

    pub fn has_alpha_surface(self) -> bool {
        state::window(self.id, |window| window.spec.alpha_surface).unwrap_or(false)
    }

    pub fn has_server_decorations(self) -> bool {
        state::window(self.id, |window| window.frame == Frame::Server).unwrap_or(false)
    }

    pub fn activate(self) -> Result<(), Unsupported> {
        crate::external::activate(self.id)
    }

    pub fn activation_token(self) -> Result<String, Unsupported> {
        crate::external::activation_token(self.id)
    }

    pub fn activate_with_token(self, token: &str) -> Result<(), Unsupported> {
        crate::external::activate_with_token(self.id, token)
    }

    pub fn exported_handle(self) -> Result<String, Unsupported> {
        crate::external::exported_handle(self.id)
    }

    pub fn engaged() -> Option<Window> {
        state::try_with(|shell| shell.seat.engaged())
            .flatten()
            .map(|id| Window { id })
    }

    pub fn dialog_parent(self) -> Option<DialogParent> {
        let (display, surface) = crate::external::proxy_pointers(self.id)?;
        Some(DialogParent {
            _lease: self.surface_lease()?,
            display,
            surface,
        })
    }

    pub fn show(self) -> Result<(), ShellError> {
        state::with(|shell| toplevel::map(shell, self.id));
        crate::looping::wait_until(CONFIGURE_LIMIT, "xdg_surface.configure", || {
            state::window(self.id, |window| window.role.configured())
        })
    }

    pub fn hide(self) {
        state::try_with(|shell| toplevel::unmap(shell, self.id));
    }

    pub fn destroy(self) {
        let remaining = state::try_with(|shell| {
            shell.seat.forget(self.id);
            shell.text_input.forget(self.id);
            shell.external.forget(self.id);
            shell.windows.remove(&self.id).map(|_| shell.windows.len())
        })
        .flatten();
        if let Some(remaining) = remaining {
            emit(self.id, ShellEvent::Destroyed);
            if remaining == 0 {
                crate::looping::quit();
            }
        }
    }

    pub fn surface_lease(self) -> Option<SurfaceLease> {
        state::window(self.id, |window| SurfaceLease::new(window.core.clone()))
    }

    pub fn present_bgra(self, width: u32, height: u32, pixels: &[u8]) -> bool {
        state::try_with(|shell| crate::shm::present(shell, self.id, width, height, pixels))
            .unwrap_or(false)
    }

    pub fn chrome(self) -> Option<Chrome> {
        state::window(self.id, |window| window.spec.chrome)
    }
}

pub struct DialogParent {
    _lease: SurfaceLease,
    display: usize,
    surface: usize,
}

impl DialogParent {
    pub fn display_pointer(&self) -> usize {
        self.display
    }

    pub fn surface_pointer(&self) -> usize {
        self.surface
    }
}

delegate_noop!(Shell: WpViewport);
delegate_noop!(Shell: WpCursorShapeDeviceV1);
