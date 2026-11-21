use crate::chrome::{self, Hit};
use crate::looping::emit;
use crate::state::Shell;
use aqua_core::{
    ClickSettings, CursorIcon, InputEvent, Point, PointerEvent, PointerKind, ScrollDelta,
};
use aqua_protocol::{CaptionControl, WindowId};
use aqua_shell::input::{button, scroll, PointerButtonSource};
use aqua_shell::ShellEvent;
use wayland_client::protocol::wl_pointer::{self, Axis, AxisSource, ButtonState, WlPointer};
use wayland_client::{Connection, Dispatch, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::cursor_shape::v1::client::wp_cursor_shape_device_v1::WpCursorShapeDeviceV1;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const BTN_MIDDLE: u32 = 0x112;
const BTN_SIDE: u32 = 0x113;
const BTN_EXTRA: u32 = 0x114;

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Focus {
    pub window: WindowId,
    pub serial: u32,
    pub at: (f64, f64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChromePress {
    None,
    Control(CaptionControl),
    Handed,
}

#[derive(Debug, Clone, Copy, PartialEq)]
struct CaptionClick {
    window: WindowId,
    time: u32,
    at: (f64, f64),
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct Axes {
    source: Option<AxisSource>,
    value: [f64; 2],
    value120: [i32; 2],
    discrete: [i32; 2],
}

pub(crate) struct Pointer {
    pub proxy: WlPointer,
    pub shape: Option<WpCursorShapeDeviceV1>,
    pub themed: Option<crate::cursor::Themed>,
    pub focus: Option<Focus>,
    pub applied: Option<CursorIcon>,
    buttons: u8,
    press: ChromePress,
    hover: Option<CaptionControl>,
    last_caption: Option<CaptionClick>,
    axes: Axes,
}

impl Pointer {
    pub(crate) fn new(proxy: WlPointer, shape: Option<WpCursorShapeDeviceV1>) -> Self {
        Self {
            proxy,
            shape,
            themed: None,
            focus: None,
            applied: None,
            buttons: 0,
            press: ChromePress::None,
            hover: None,
            last_caption: None,
            axes: Axes::default(),
        }
    }

    pub(crate) fn forget(&mut self, window: WindowId) {
        if self.focus.is_some_and(|focus| focus.window == window) {
            self.focus = None;
            self.buttons = 0;
            self.press = ChromePress::None;
            self.hover = None;
        }
    }

    fn event(&self, focus: Focus) -> PointerEvent {
        PointerEvent {
            contact: 0,
            kind: PointerKind::Mouse,
            position: Point::new(focus.at.0 as f32, focus.at.1 as f32),
            buttons: self.buttons,
        }
    }
}

impl Drop for Pointer {
    fn drop(&mut self) {
        if let Some(shape) = self.shape.take() {
            shape.destroy();
        }
        if self.proxy.version() >= 3 {
            self.proxy.release();
        }
    }
}

fn source(code: u32) -> PointerButtonSource {
    match code {
        BTN_LEFT => PointerButtonSource::Left,
        BTN_RIGHT => PointerButtonSource::Right,
        BTN_MIDDLE => PointerButtonSource::Middle,
        BTN_SIDE => PointerButtonSource::X(1),
        BTN_EXTRA => PointerButtonSource::X(2),
        other => PointerButtonSource::X(
            u16::try_from(other.saturating_sub(BTN_SIDE) + 1).unwrap_or(u16::MAX),
        ),
    }
}

fn hit_at(shell: &Shell, focus: Focus) -> Hit {
    shell
        .windows
        .get(&focus.window)
        .map_or(Hit::Client, |window| chrome::hit(window, focus.at))
}

fn refresh_hover(shell: &mut Shell) {
    let Some(focus) = shell
        .seat
        .pointer
        .as_ref()
        .and_then(|pointer| pointer.focus)
    else {
        return;
    };
    let hit = hit_at(shell, focus);
    let client = shell
        .windows
        .get(&focus.window)
        .map_or(CursorIcon::Default, |window| window.cursor);
    let wanted = hit.cursor(client);
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let hover = hit.hover();
    if std::mem::replace(&mut pointer.hover, hover) != hover {
        emit(focus.window, ShellEvent::CaptionHover(hover));
    }
    if pointer.applied != Some(wanted) {
        crate::cursor::apply(shell, wanted);
    }
}

pub(crate) fn cursor_changed(shell: &mut Shell, window: WindowId) {
    if shell
        .seat
        .pointer
        .as_ref()
        .and_then(|pointer| pointer.focus)
        .is_some_and(|focus| focus.window == window)
    {
        refresh_hover(shell);
    }
}

fn enter(shell: &mut Shell, window: WindowId, serial: u32, at: (f64, f64)) {
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let focus = Focus { window, serial, at };
    pointer.focus = Some(focus);
    pointer.applied = None;
    let event = pointer.event(focus);
    emit(window, ShellEvent::PointerEntered);
    refresh_hover(shell);
    emit(window, ShellEvent::Input(InputEvent::PointerMove(event)));
}

fn leave(shell: &mut Shell) {
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let Some(focus) = pointer.focus.take() else {
        return;
    };
    if std::mem::take(&mut pointer.buttons) != 0 {
        emit(
            focus.window,
            ShellEvent::Input(InputEvent::PointerCancel { contact: 0 }),
        );
    }
    pointer.press = ChromePress::None;
    pointer.applied = None;
    if pointer.hover.take().is_some() {
        emit(focus.window, ShellEvent::CaptionHover(None));
    }
    emit(focus.window, ShellEvent::PointerLeft);
}

fn motion(shell: &mut Shell, at: (f64, f64)) {
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let Some(focus) = pointer.focus.as_mut() else {
        return;
    };
    focus.at = at;
    let focus = *focus;
    let event = pointer.event(focus);
    refresh_hover(shell);
    emit(
        focus.window,
        ShellEvent::Input(InputEvent::PointerMove(event)),
    );
}

fn double_click(
    previous: Option<CaptionClick>,
    current: CaptionClick,
    settings: ClickSettings,
) -> bool {
    previous.is_some_and(|previous| {
        let interval = u128::from(current.time.wrapping_sub(previous.time));
        let distance = settings.max_distance();
        previous.window == current.window
            && interval <= settings.interval().as_millis()
            && (current.at.0 - previous.at.0).abs() <= f64::from(distance.x)
            && (current.at.1 - previous.at.1).abs() <= f64::from(distance.y)
    })
}

enum Gesture {
    Arm(CaptionControl),
    Resize(wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge),
    Move,
    ToggleMaximize,
    WindowMenu,
    Nothing,
}

fn gesture(
    pointer: &mut Pointer,
    focus: Focus,
    hit: Hit,
    code: u32,
    time: u32,
    maximizable: bool,
) -> Gesture {
    match (hit, code) {
        (Hit::Control(control), BTN_LEFT) => Gesture::Arm(control),
        (Hit::Edge(edge), BTN_LEFT) => Gesture::Resize(edge),
        (Hit::Caption, BTN_LEFT) => {
            let click = CaptionClick {
                window: focus.window,
                time,
                at: focus.at,
            };
            let settings = crate::window::default_click_settings();
            match (
                double_click(pointer.last_caption, click, settings),
                maximizable,
            ) {
                (true, true) => {
                    pointer.last_caption = None;
                    Gesture::ToggleMaximize
                }
                (true, false) => {
                    pointer.last_caption = None;
                    Gesture::Nothing
                }
                (false, _) => {
                    pointer.last_caption = Some(click);
                    Gesture::Move
                }
            }
        }
        (Hit::Caption, BTN_RIGHT) => Gesture::WindowMenu,
        _ => Gesture::Nothing,
    }
}

fn chrome_press(shell: &mut Shell, focus: Focus, hit: Hit, code: u32, serial: u32, time: u32) {
    let seat = shell.globals.seat.as_ref().map(|(_, seat)| seat.clone());
    let Some(window) = shell.windows.get(&focus.window) else {
        return;
    };
    let toplevel = window
        .role
        .toplevel()
        .map(|toplevel| toplevel.toplevel.clone());
    let maximizable = window.spec.maximizable && window.spec.resizable;
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let gesture = gesture(pointer, focus, hit, code, time, maximizable);
    pointer.press = match gesture {
        Gesture::Arm(control) => ChromePress::Control(control),
        Gesture::Nothing => ChromePress::None,
        _ => ChromePress::Handed,
    };
    let (Some(seat), Some(toplevel)) = (seat, toplevel) else {
        return;
    };
    match gesture {
        Gesture::Resize(edge) => toplevel.resize(&seat, serial, edge),
        Gesture::Move => toplevel._move(&seat, serial),
        Gesture::WindowMenu => {
            toplevel.show_window_menu(&seat, serial, focus.at.0 as i32, focus.at.1 as i32)
        }
        Gesture::ToggleMaximize => crate::window::toggle_maximize_of(shell, focus.window),
        Gesture::Arm(_) | Gesture::Nothing => {}
    }
}

fn chrome_release(shell: &mut Shell, focus: Focus, code: u32) {
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let pressed = std::mem::replace(&mut pointer.press, ChromePress::None);
    let ChromePress::Control(control) = pressed else {
        return;
    };
    if code != BTN_LEFT || hit_at(shell, focus) != Hit::Control(control) {
        return;
    }
    match control {
        CaptionControl::Close => emit(focus.window, ShellEvent::CloseRequested),
        CaptionControl::Maximize => crate::window::toggle_maximize_of(shell, focus.window),
        CaptionControl::Minimize => {
            let _ = crate::window::minimize_of(shell, focus.window);
        }
    }
}

fn forward(
    pointer: &mut Pointer,
    focus: Focus,
    which: aqua_core::PointerButton,
    bit: u8,
    down: bool,
) {
    let event = match down {
        true => {
            pointer.buttons |= bit;
            InputEvent::PointerDown(pointer.event(focus), which)
        }
        false => {
            pointer.buttons &= !bit;
            InputEvent::PointerUp(pointer.event(focus), which)
        }
    };
    emit(focus.window, ShellEvent::Input(event));
}

fn pressed(shell: &mut Shell, serial: u32, time: u32, code: u32, down: bool) {
    let Some(focus) = shell
        .seat
        .pointer
        .as_ref()
        .and_then(|pointer| pointer.focus)
    else {
        return;
    };
    if down {
        shell.seat.serial = Some(serial);
    }
    let (which, bit) = button(source(code));
    let hit = hit_at(shell, focus);
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let held = pointer.buttons & bit != 0;
    let chrome_active = pointer.press != ChromePress::None;
    let client = hit == Hit::Client || pointer.buttons != 0;
    match (down, held, chrome_active, client) {
        (true, _, true, _) => {}
        (true, _, false, true) => forward(pointer, focus, which, bit, true),
        (true, _, false, false) => chrome_press(shell, focus, hit, code, serial, time),
        (false, true, _, _) => forward(pointer, focus, which, bit, false),
        (false, false, _, _) => chrome_release(shell, focus, code),
    }
}

fn axis_index(axis: Axis) -> usize {
    match axis {
        Axis::HorizontalScroll => 0,
        _ => 1,
    }
}

fn clamp_i16(value: i32) -> i16 {
    i16::try_from(value.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

fn scroll_delta(axes: Axes) -> Option<ScrollDelta> {
    let notches = |index: usize| match (axes.value120[index], axes.discrete[index]) {
        (0, 0) => None,
        (0, discrete) => Some(discrete.saturating_mul(120)),
        (value120, _) => Some(value120),
    };
    let wheel = axes
        .source
        .is_none_or(|source| source == AxisSource::Wheel || source == AxisSource::WheelTilt);
    let delta = match (wheel, notches(0), notches(1)) {
        (true, horizontal, vertical) if horizontal.is_some() || vertical.is_some() => ScrollDelta {
            x: horizontal.map_or(0.0, |value| scroll(false, clamp_i16(value)).x),
            y: vertical.map_or(0.0, |value| scroll(true, clamp_i16(-value)).y),
        },
        _ => ScrollDelta {
            x: axes.value[0] as f32,
            y: axes.value[1] as f32,
        },
    };
    (delta.x != 0.0 || delta.y != 0.0).then_some(delta)
}

fn frame(shell: &mut Shell) {
    let Some(pointer) = shell.seat.pointer.as_mut() else {
        return;
    };
    let axes = std::mem::take(&mut pointer.axes);
    let (Some(focus), Some(delta)) = (pointer.focus, scroll_delta(axes)) else {
        return;
    };
    emit(
        focus.window,
        ShellEvent::Input(InputEvent::Scroll {
            at: Point::new(focus.at.0 as f32, focus.at.1 as f32),
            delta,
        }),
    );
}

fn axes(shell: &mut Shell) -> Option<&mut Axes> {
    shell.seat.pointer.as_mut().map(|pointer| &mut pointer.axes)
}

impl Dispatch<WlPointer, ()> for Shell {
    fn event(
        state: &mut Self,
        proxy: &WlPointer,
        event: wl_pointer::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let framed = proxy.version() >= 5;
        match event {
            wl_pointer::Event::Enter {
                serial,
                surface,
                surface_x,
                surface_y,
            } => {
                if let Some(id) = surface.data::<WindowId>().copied() {
                    enter(state, id, serial, (surface_x, surface_y));
                }
            }
            wl_pointer::Event::Leave { .. } => leave(state),
            wl_pointer::Event::Motion {
                surface_x,
                surface_y,
                ..
            } => motion(state, (surface_x, surface_y)),
            wl_pointer::Event::Button {
                serial,
                time,
                button,
                state: WEnum::Value(button_state),
            } => pressed(
                state,
                serial,
                time,
                button,
                button_state == ButtonState::Pressed,
            ),
            wl_pointer::Event::Axis {
                axis: WEnum::Value(axis),
                value,
                ..
            } => {
                if let Some(axes) = axes(state) {
                    axes.value[axis_index(axis)] += value;
                }
            }
            wl_pointer::Event::AxisSource {
                axis_source: WEnum::Value(source),
            } => {
                if let Some(axes) = axes(state) {
                    axes.source = Some(source);
                }
            }
            wl_pointer::Event::AxisDiscrete {
                axis: WEnum::Value(axis),
                discrete,
            } => {
                if let Some(axes) = axes(state) {
                    axes.discrete[axis_index(axis)] += discrete;
                }
            }
            wl_pointer::Event::AxisValue120 {
                axis: WEnum::Value(axis),
                value120,
            } => {
                if let Some(axes) = axes(state) {
                    axes.value120[axis_index(axis)] += value120;
                }
            }
            wl_pointer::Event::Frame => frame(state),
            _ => {}
        }
        if !framed {
            frame(state);
        }
    }
}
