use crate::decoration::{self, Negotiation};
use crate::lease::SurfaceGeometry;
use crate::looping::emit;
use crate::state::{Announced, Shell};
use aqua_protocol::WindowId;
use aqua_shell::{ShellEvent, WindowKind};
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_client::{Connection, Dispatch, QueueHandle, WEnum};
use wayland_protocols::xdg::decoration::zv1::client::zxdg_toplevel_decoration_v1::{
    self, ZxdgToplevelDecorationV1,
};
use wayland_protocols::xdg::shell::client::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::client::xdg_toplevel::{self, XdgToplevel};

fn words(raw: &[u8]) -> impl Iterator<Item = u32> + '_ {
    raw.as_chunks::<4>()
        .0
        .iter()
        .map(|word| u32::from_ne_bytes(*word))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct States {
    pub maximized: bool,
    pub fullscreen: bool,
    pub activated: bool,
    pub tiled: bool,
    pub suspended: bool,
}

impl States {
    pub(crate) fn constrained(self) -> bool {
        self.maximized || self.fullscreen || self.tiled
    }

    fn parse(raw: &[u8]) -> Self {
        words(raw)
            .filter_map(|value| xdg_toplevel::State::try_from(value).ok())
            .fold(States::default(), |states, state| match state {
                xdg_toplevel::State::Maximized => States {
                    maximized: true,
                    ..states
                },
                xdg_toplevel::State::Fullscreen => States {
                    fullscreen: true,
                    ..states
                },
                xdg_toplevel::State::Activated => States {
                    activated: true,
                    ..states
                },
                xdg_toplevel::State::TiledLeft
                | xdg_toplevel::State::TiledRight
                | xdg_toplevel::State::TiledTop
                | xdg_toplevel::State::TiledBottom => States {
                    tiled: true,
                    ..states
                },
                xdg_toplevel::State::Suspended => States {
                    suspended: true,
                    ..states
                },
                _ => states,
            })
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(crate) struct Capabilities {
    pub window_menu: bool,
    pub maximize: bool,
    pub fullscreen: bool,
    pub minimize: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Handshake {
    AwaitingFirstConfigure,
    Configured,
}

#[derive(Debug, Default)]
pub(crate) struct PendingConfigure {
    size: Option<(i32, i32)>,
    states: States,
    decoration: Option<zxdg_toplevel_decoration_v1::Mode>,
}

pub(crate) struct Toplevel {
    pub xdg_surface: XdgSurface,
    pub toplevel: XdgToplevel,
    pub decoration: Option<ZxdgToplevelDecorationV1>,
    pub handshake: Handshake,
    pub capabilities: Option<Capabilities>,
    pending: PendingConfigure,
}

impl Drop for Toplevel {
    fn drop(&mut self) {
        if let Some(decoration) = self.decoration.take() {
            decoration.destroy();
        }
        self.toplevel.destroy();
        self.xdg_surface.destroy();
    }
}

pub(crate) enum Role {
    Unmapped,
    Mapped(Box<Toplevel>),
}

impl Role {
    pub(crate) fn toplevel(&self) -> Option<&Toplevel> {
        match self {
            Role::Mapped(toplevel) => Some(toplevel),
            Role::Unmapped => None,
        }
    }

    pub(crate) fn configured(&self) -> bool {
        self.toplevel()
            .is_some_and(|toplevel| toplevel.handshake == Handshake::Configured)
    }
}

pub(crate) fn map(shell: &mut Shell, id: WindowId) {
    let qh = shell.qh.clone();
    let Some(wm_base) = shell.globals.wm_base.clone() else {
        return;
    };
    let decoration_manager = shell.globals.decoration.clone();
    let owner = shell
        .windows
        .get(&id)
        .and_then(|window| window.spec.owner)
        .and_then(|owner| shell.windows.get(&owner))
        .and_then(|owner| owner.role.toplevel())
        .map(|owner| owner.toplevel.clone());
    let Some(window) = shell.windows.get_mut(&id) else {
        return;
    };
    if matches!(window.role, Role::Mapped(_)) {
        return;
    }
    let surface = &window.core.surface;
    surface.attach(None, 0, 0);
    surface.commit();
    let xdg_surface = wm_base.get_xdg_surface(surface, &qh, id);
    let toplevel = xdg_surface.get_toplevel(&qh, id);
    toplevel.set_title(window.spec.title.clone());
    toplevel.set_app_id(window.spec.app_id.clone());
    match (window.spec.resizable, window.spec.min_logical) {
        (false, _) => {
            toplevel.set_min_size(window.logical.0, window.logical.1);
            toplevel.set_max_size(window.logical.0, window.logical.1);
        }
        (true, Some((width, height))) => toplevel.set_min_size(width, height),
        (true, None) => {}
    }
    if window.spec.kind == WindowKind::Panel
        && let Some(owner) = &owner
    {
        toplevel.set_parent(Some(owner));
    }
    let decoration = decoration_manager.map(|manager| {
        let decoration = manager.get_toplevel_decoration(&toplevel, &qh, id);
        decoration.set_mode(decoration::requested(&window.spec));
        decoration
    });
    if decoration.is_none() {
        window.frame = decoration::negotiated(&window.spec, Negotiation::Unavailable);
    }
    surface.commit();
    window.role = Role::Mapped(Box::new(Toplevel {
        xdg_surface,
        toplevel,
        decoration,
        handshake: Handshake::AwaitingFirstConfigure,
        capabilities: None,
        pending: PendingConfigure::default(),
    }));
}

pub(crate) fn unmap(shell: &mut Shell, id: WindowId) {
    let Some(window) = shell.windows.get_mut(&id) else {
        return;
    };
    if let Role::Mapped(_) = std::mem::replace(&mut window.role, Role::Unmapped) {
        window.core.surface.attach(None, 0, 0);
        window.core.surface.commit();
        let was_active = std::mem::take(&mut window.states).activated;
        window.outputs.clear();
        window.probe = crate::occlusion::Probe::Idle;
        window.starved = false;
        if was_active {
            emit(id, ShellEvent::Focused(false));
        }
        crate::occlusion::settle(window);
    }
}

pub(crate) fn announce(shell: &mut Shell, id: WindowId) {
    let refresh = shell
        .windows
        .get(&id)
        .and_then(|window| crate::output::primary_refresh(shell, &window.outputs));
    let Some(window) = shell.windows.get_mut(&id) else {
        return;
    };
    let current = Announced {
        scale: window.scale.value,
        physical: window.physical(),
    };
    let previous = window.announced.replace(current);
    if previous.is_some_and(|previous| previous.scale != current.scale) {
        emit(
            id,
            ShellEvent::ScaleChanged {
                scale: current.scale,
            },
        );
    }
    if previous.is_none_or(|previous| previous.physical != current.physical) {
        emit(
            id,
            ShellEvent::Resized {
                width: current.physical.0,
                height: current.physical.1,
            },
        );
    }
    window.core.set_geometry(SurfaceGeometry {
        logical: (
            window.logical.0.max(1) as u32,
            window.logical.1.max(1) as u32,
        ),
        physical: current.physical,
        scale: current.scale,
        scaling: crate::scale::scaling(window),
        refresh,
    });
    window.redraw_pending = true;
}

fn update_scale(shell: &mut Shell, id: WindowId) {
    let Some(scale) = shell
        .windows
        .get(&id)
        .map(|window| crate::scale::resolve(shell, window))
    else {
        return;
    };
    let Some(window) = shell.windows.get_mut(&id) else {
        return;
    };
    if window.scale.value != scale {
        window.scale.value = scale;
        window.rects = aqua_shell::regions::drag_rects(&window.regions, scale);
    }
}

pub(crate) fn rescale(shell: &mut Shell, id: WindowId) {
    update_scale(shell, id);
    if shell
        .windows
        .get(&id)
        .is_some_and(|window| window.announced.is_some())
    {
        announce(shell, id);
    }
}

pub(crate) fn outputs_changed(shell: &mut Shell, id: WindowId) {
    rescale(shell, id);
    emit(id, ShellEvent::DisplayChanged);
}

fn configured(shell: &mut Shell, id: WindowId, serial: u32) {
    let Some(window) = shell.windows.get_mut(&id) else {
        return;
    };
    let Role::Mapped(toplevel) = &mut window.role else {
        return;
    };
    toplevel.xdg_surface.ack_configure(serial);
    toplevel.handshake = Handshake::Configured;
    let pending = std::mem::take(&mut toplevel.pending);
    if let Some(mode) = pending.decoration {
        window.frame = decoration::negotiated(&window.spec, Negotiation::Granted(mode));
    }
    let before = std::mem::replace(&mut window.states, pending.states);
    let after = window.states;
    let minimum = window.spec.min_logical.unwrap_or((1, 1));
    window.logical = match pending.size {
        Some((width, height)) => (width.max(minimum.0), height.max(minimum.1)),
        None if before.constrained() && !after.constrained() => window.restore,
        None => window.logical,
    };
    if !after.constrained() {
        window.restore = window.logical;
    }
    if before.activated != after.activated {
        emit(id, ShellEvent::Focused(after.activated));
    }
    if before.maximized != after.maximized {
        emit(id, ShellEvent::Maximized(after.maximized));
    }
    crate::occlusion::settle(window);
    update_scale(shell, id);
    announce(shell, id);
}

impl Dispatch<WlSurface, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &WlSurface,
        event: wl_surface::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_surface::Event::Enter { output } => crate::output::entered(state, *id, &output),
            wl_surface::Event::Leave { output } => crate::output::left_proxy(state, *id, &output),
            wl_surface::Event::PreferredBufferScale { factor } => {
                if let Some(window) = state.windows.get_mut(id) {
                    window.scale.preferred_buffer = Some(factor);
                }
                rescale(state, *id);
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgSurface, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &XdgSurface,
        event: xdg_surface::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let xdg_surface::Event::Configure { serial } = event {
            configured(state, *id, serial);
        }
    }
}

fn pending_of(shell: &mut Shell, id: WindowId) -> Option<&mut Toplevel> {
    match shell.windows.get_mut(&id).map(|window| &mut window.role) {
        Some(Role::Mapped(toplevel)) => Some(toplevel),
        _ => None,
    }
}

impl Dispatch<XdgToplevel, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &XdgToplevel,
        event: xdg_toplevel::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            xdg_toplevel::Event::Configure {
                width,
                height,
                states,
            } => {
                if let Some(toplevel) = pending_of(state, *id) {
                    toplevel.pending.size = (width > 0 && height > 0).then_some((width, height));
                    toplevel.pending.states = States::parse(&states);
                }
            }
            xdg_toplevel::Event::Close => emit(*id, ShellEvent::CloseRequested),
            xdg_toplevel::Event::WmCapabilities { capabilities } => {
                if let Some(toplevel) = pending_of(state, *id) {
                    toplevel.capabilities = Some(parse_capabilities(&capabilities));
                }
            }
            _ => {}
        }
    }
}

fn parse_capabilities(raw: &[u8]) -> Capabilities {
    words(raw)
        .filter_map(|value| xdg_toplevel::WmCapabilities::try_from(value).ok())
        .fold(
            Capabilities::default(),
            |found, capability| match capability {
                xdg_toplevel::WmCapabilities::WindowMenu => Capabilities {
                    window_menu: true,
                    ..found
                },
                xdg_toplevel::WmCapabilities::Maximize => Capabilities {
                    maximize: true,
                    ..found
                },
                xdg_toplevel::WmCapabilities::Fullscreen => Capabilities {
                    fullscreen: true,
                    ..found
                },
                xdg_toplevel::WmCapabilities::Minimize => Capabilities {
                    minimize: true,
                    ..found
                },
                _ => found,
            },
        )
}

impl Dispatch<ZxdgToplevelDecorationV1, WindowId> for Shell {
    fn event(
        state: &mut Self,
        _: &ZxdgToplevelDecorationV1,
        event: zxdg_toplevel_decoration_v1::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let zxdg_toplevel_decoration_v1::Event::Configure {
            mode: WEnum::Value(mode),
        } = event
            && let Some(toplevel) = pending_of(state, *id)
        {
            toplevel.pending.decoration = Some(mode);
        }
    }
}
