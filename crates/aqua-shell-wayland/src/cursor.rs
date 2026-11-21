use crate::state::Shell;
use aqua_core::CursorIcon;
use wayland_client::protocol::wl_surface::WlSurface;
use wayland_client::Proxy;
use wayland_cursor::CursorTheme;
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::Shape;

pub(crate) fn shape(icon: CursorIcon) -> Option<Shape> {
    match icon {
        CursorIcon::Default => Some(Shape::Default),
        CursorIcon::Text => Some(Shape::Text),
        CursorIcon::Pointer => Some(Shape::Pointer),
        CursorIcon::Crosshair => Some(Shape::Crosshair),
        CursorIcon::Move => Some(Shape::Move),
        CursorIcon::NotAllowed => Some(Shape::NotAllowed),
        CursorIcon::EwResize => Some(Shape::EwResize),
        CursorIcon::NsResize => Some(Shape::NsResize),
        CursorIcon::ColResize => Some(Shape::ColResize),
        CursorIcon::RowResize => Some(Shape::RowResize),
        CursorIcon::Grab => Some(Shape::Grab),
        CursorIcon::Grabbing => Some(Shape::Grabbing),
        CursorIcon::NeswResize => Some(Shape::NeswResize),
        CursorIcon::NwseResize => Some(Shape::NwseResize),
        CursorIcon::Wait => Some(Shape::Wait),
        CursorIcon::Progress => Some(Shape::Progress),
        CursorIcon::Help => Some(Shape::Help),
        CursorIcon::Hidden => None,
    }
}

fn theme_names(icon: CursorIcon) -> &'static [&'static str] {
    match icon {
        CursorIcon::Default | CursorIcon::Hidden => &["default", "left_ptr"],
        CursorIcon::Text => &["text", "xterm"],
        CursorIcon::Pointer => &["pointer", "hand2"],
        CursorIcon::Crosshair => &["crosshair", "cross"],
        CursorIcon::Move => &["move", "fleur"],
        CursorIcon::NotAllowed => &["not-allowed", "crossed_circle"],
        CursorIcon::EwResize => &["ew-resize", "sb_h_double_arrow"],
        CursorIcon::NsResize => &["ns-resize", "sb_v_double_arrow"],
        CursorIcon::ColResize => &["col-resize", "sb_h_double_arrow"],
        CursorIcon::RowResize => &["row-resize", "sb_v_double_arrow"],
        CursorIcon::Grab => &["grab", "openhand", "hand1"],
        CursorIcon::Grabbing => &["grabbing", "closedhand", "fleur"],
        CursorIcon::NeswResize => &["nesw-resize", "fd_double_arrow"],
        CursorIcon::NwseResize => &["nwse-resize", "bd_double_arrow"],
        CursorIcon::Wait => &["wait", "watch"],
        CursorIcon::Progress => &["progress", "left_ptr_watch"],
        CursorIcon::Help => &["help", "question_arrow"],
    }
}

pub(crate) struct Themed {
    theme: CursorTheme,
    surface: WlSurface,
    scale: i32,
}

fn theme_size() -> u32 {
    std::env::var("XCURSOR_SIZE")
        .ok()
        .and_then(|size| size.parse().ok())
        .filter(|size| *size > 0)
        .unwrap_or(24)
}

fn load_theme(shell: &Shell, scale: i32) -> Option<Themed> {
    let shm = shell.globals.shm.clone()?;
    let compositor = shell.globals.compositor.as_ref()?;
    let size = theme_size() * u32::try_from(scale).unwrap_or(1);
    let theme = match std::env::var("XCURSOR_THEME") {
        Ok(name) => CursorTheme::load_from_name(&shell.conn, shm, &name, size),
        Err(_) => CursorTheme::load(&shell.conn, shm, size),
    }
    .ok()?;
    Some(Themed {
        theme,
        surface: compositor.create_surface(&shell.qh, ()),
        scale,
    })
}

pub(crate) fn apply(shell: &mut Shell, icon: CursorIcon) {
    let scale = shell
        .seat
        .pointer
        .as_ref()
        .and_then(|pointer| pointer.focus)
        .and_then(|focus| shell.windows.get(&focus.window))
        .map_or(1, |window| window.scale.value.ceil().max(1.0) as i32);
    let reload = shell.seat.pointer.as_ref().is_some_and(|pointer| {
        pointer.shape.is_none()
            && pointer
                .themed
                .as_ref()
                .is_none_or(|themed| themed.scale != scale)
    });
    let loaded = match reload && icon != CursorIcon::Hidden {
        true => load_theme(shell, scale),
        false => None,
    };
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let Some(focus) = pointer.focus else {
        return;
    };
    if loaded.is_some() {
        pointer.themed = loaded;
    }
    pointer.applied = Some(icon);
    match (shape(icon), &pointer.shape, pointer.themed.as_mut()) {
        (None, _, _) => pointer.proxy.set_cursor(focus.serial, None, 0, 0),
        (Some(shape), Some(device), _) => device.set_shape(focus.serial, shape),
        (Some(_), None, Some(themed)) => {
            let Some(cursor) = theme_names(icon)
                .iter()
                .find_map(|name| themed.theme.get_cursor(name).map(|_| *name))
                .and_then(|name| themed.theme.get_cursor(name))
            else {
                return;
            };
            let image = &cursor[0];
            let (x, y) = image.hotspot();
            themed.surface.attach(Some(image), 0, 0);
            if themed.surface.version() >= 3 {
                themed.surface.set_buffer_scale(themed.scale);
            }
            let (width, height) = image.dimensions();
            themed
                .surface
                .damage_buffer(0, 0, width as i32, height as i32);
            themed.surface.commit();
            pointer.proxy.set_cursor(
                focus.serial,
                Some(&themed.surface),
                x as i32 / themed.scale,
                y as i32 / themed.scale,
            );
        }
        (Some(_), None, None) => {}
    }
}

impl Drop for Themed {
    fn drop(&mut self) {
        self.surface.destroy();
    }
}
