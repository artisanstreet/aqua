use crate::event::{Chrome, ShellEvent, WindowKind};
use crate::input::{self, PointerButtonSource};
use crate::looping::turn;
use crate::regions::{self, Geometry, ht};
use crate::state::{self, Queued};
use aqua_core::{CursorIcon, InputEvent, KeyEvent, Modifiers, Point, PointerEvent, PointerKind};
use aqua_platform::chrome::RectI;
use aqua_protocol::CaptionControl;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{ClientToScreen, PtInRect, ScreenToClient, ValidateRect};
use windows::Win32::UI::Controls::WM_MOUSELEAVE;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, GetDpiForWindow, GetSystemMetricsForDpi,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, GetKeyboardState, ReleaseCapture, SetCapture, TME_LEAVE,
    TME_NONCLIENT, TRACKMOUSEEVENT, ToUnicodeEx, TrackMouseEvent,
};
use windows::Win32::UI::Input::{GetRawInputData, HRAWINPUT, RAWINPUT, RAWINPUTHEADER, RID_INPUT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

pub(crate) static CLASS_NAME: [u16; 12] = {
    let text = *b"AquaShell32";
    let mut out = [0u16; 12];
    let mut i = 0;
    while i < 11 {
        out[i] = text[i] as u16;
        i += 1;
    }
    out
};

pub(crate) const WM_WAKE: u32 = WM_APP + 1;
const MODAL_TIMER: usize = 1;
const MA_NOACTIVATE_RESULT: isize = 3;

pub(crate) fn hwnd_from(raw: isize) -> HWND {
    HWND(raw as *mut _)
}

fn scale_of(raw: isize) -> f64 {
    state::window(raw, |w| w.scale).unwrap_or(1.0)
}

fn logical(raw: isize, point: (i32, i32)) -> Point {
    let scale = scale_of(raw);
    Point::new(
        (f64::from(point.0) / scale) as f32,
        (f64::from(point.1) / scale) as f32,
    )
}

fn emit_input(raw: isize, event: InputEvent) {
    state::emit(raw, ShellEvent::Input(event));
}

fn held(vk: i32) -> bool {
    let state = unsafe { GetKeyState(vk) };
    state < 0
}

fn current_modifiers() -> Modifiers {
    input::modifiers(held(0x10), held(0x11), held(0x12), held(0x5B) || held(0x5C))
}

fn sync_modifiers(raw: isize) -> Modifiers {
    let now = current_modifiers();
    let changed =
        state::window(raw, |w| std::mem::replace(&mut w.modifiers, now) != now).unwrap_or(false);
    if changed {
        emit_input(raw, InputEvent::ModifiersChanged(now));
    }
    now
}

fn pointer(raw: isize, point: (i32, i32)) -> PointerEvent {
    PointerEvent {
        contact: 0,
        kind: PointerKind::Mouse,
        position: logical(raw, point),
        buttons: state::window(raw, |w| w.buttons_down).unwrap_or(0),
    }
}

fn track(hwnd: HWND, raw: isize, non_client: bool) {
    let needs = state::window(raw, |w| {
        let flag = if non_client {
            &mut w.tracking_nc
        } else {
            &mut w.tracking_client
        };
        !std::mem::replace(flag, true)
    })
    .unwrap_or(false);
    if needs {
        let mut request = TRACKMOUSEEVENT {
            cbSize: std::mem::size_of::<TRACKMOUSEEVENT>() as u32,
            dwFlags: if non_client {
                TME_LEAVE | TME_NONCLIENT
            } else {
                TME_LEAVE
            },
            hwndTrack: hwnd,
            dwHoverTime: 0,
        };
        let _ = unsafe { TrackMouseEvent(&mut request) };
    }
}

fn set_hover(raw: isize, hover: Option<CaptionControl>) {
    let changed =
        state::window(raw, |w| std::mem::replace(&mut w.hover, hover) != hover).unwrap_or(false);
    if changed {
        state::emit(raw, ShellEvent::CaptionHover(hover));
    }
}

fn pointer_button(
    hwnd: HWND,
    raw: isize,
    lparam: LPARAM,
    source: PointerButtonSource,
    pressed: bool,
) {
    sync_modifiers(raw);
    let (button, bit) = input::button(source);
    let (was, now) = state::window(raw, |w| {
        let was = w.buttons_down;
        if pressed {
            w.buttons_down |= bit
        } else {
            w.buttons_down &= !bit
        }
        (was, w.buttons_down)
    })
    .unwrap_or((0, 0));
    unsafe {
        if was == 0 && now != 0 {
            SetCapture(hwnd);
        } else if was != 0 && now == 0 {
            let _ = ReleaseCapture();
        }
    }
    let event = pointer(raw, input::point_from_lparam(lparam.0));
    emit_input(
        raw,
        if pressed {
            InputEvent::PointerDown(event, button)
        } else {
            InputEvent::PointerUp(event, button)
        },
    );
}

fn character_for(vk: u16, lparam: LPARAM, modifiers: Modifiers) -> Option<char> {
    let mut keys = [0u8; 256];
    unsafe { GetKeyboardState(&mut keys) }.ok()?;
    if !modifiers.contains(Modifiers::ALT) {
        keys[0x11] = 0;
        keys[0xA2] = 0;
        keys[0xA3] = 0;
    }
    let scan = ((lparam.0 >> 16) & 0xFF) as u32;
    let mut out = [0u16; 8];
    let count = unsafe {
        ToUnicodeEx(
            u32::from(vk),
            scan,
            &keys,
            &mut out,
            0x4,
            Some(GetKeyboardLayout(0)),
        )
    };
    (count > 0)
        .then(|| char::decode_utf16(out[..count as usize].iter().copied()).next())
        .flatten()
        .and_then(Result::ok)
}

fn key(raw: isize, wparam: WPARAM, lparam: LPARAM, pressed: bool) {
    let vk = wparam.0 as u16;
    let modifiers = sync_modifiers(raw);
    if input::is_modifier_key(vk) {
        return;
    }
    let character = match input::named_key(vk) {
        Some(_) => None,
        None => character_for(vk, lparam, modifiers),
    };
    let event = KeyEvent {
        key: input::resolve_key(vk, character),
        modifiers,
        repeat: pressed && input::is_repeat(lparam.0),
    };
    emit_input(
        raw,
        if pressed {
            InputEvent::KeyDown(event)
        } else {
            InputEvent::KeyUp(event)
        },
    );
}

fn frame_border(hwnd: HWND) -> i32 {
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    unsafe {
        GetSystemMetricsForDpi(SM_CXSIZEFRAME, dpi) + GetSystemMetricsForDpi(SM_CXPADDEDBORDER, dpi)
    }
}

fn hit_test(hwnd: HWND, raw: isize, lparam: LPARAM) -> Option<i32> {
    let (chrome, resizable, minimizable) =
        state::window(raw, |w| (w.chrome, w.resizable, w.minimizable))?;
    (chrome == Chrome::Custom).then_some(())?;
    let mut window = RECT::default();
    let _ = unsafe { GetWindowRect(hwnd, &mut window) };
    let mut origin = POINT::default();
    let _ = unsafe { ClientToScreen(hwnd, &mut origin) };
    let geometry = Geometry {
        point: input::point_from_lparam(lparam.0),
        window: RectI {
            left: window.left,
            top: window.top,
            right: window.right,
            bottom: window.bottom,
        },
        client_origin: (origin.x, origin.y),
        border: frame_border(hwnd).max(0) as u32,
        maximized: unsafe { IsZoomed(hwnd).as_bool() },
        resizable,
        minimizable,
    };
    state::window(raw, |w| regions::hit_test(&geometry, &w.rects))
}

fn caption_for(hit: i32) -> Option<CaptionControl> {
    match hit {
        ht::MINBUTTON => Some(CaptionControl::Minimize),
        ht::MAXBUTTON => Some(CaptionControl::Maximize),
        ht::CLOSE => Some(CaptionControl::Close),
        _ => None,
    }
}

fn cursor_for(icon: CursorIcon) -> Option<PCWSTR> {
    match icon {
        CursorIcon::Default => Some(IDC_ARROW),
        CursorIcon::Text => Some(IDC_IBEAM),
        CursorIcon::Pointer | CursorIcon::Grab => Some(IDC_HAND),
        CursorIcon::Crosshair => Some(IDC_CROSS),
        CursorIcon::Move | CursorIcon::Grabbing => Some(IDC_SIZEALL),
        CursorIcon::NotAllowed => Some(IDC_NO),
        CursorIcon::EwResize | CursorIcon::ColResize => Some(IDC_SIZEWE),
        CursorIcon::NsResize | CursorIcon::RowResize => Some(IDC_SIZENS),
        CursorIcon::NeswResize => Some(IDC_SIZENESW),
        CursorIcon::NwseResize => Some(IDC_SIZENWSE),
        CursorIcon::Wait => Some(IDC_WAIT),
        CursorIcon::Progress => Some(IDC_APPSTARTING),
        CursorIcon::Help => Some(IDC_HELP),
        CursorIcon::Hidden => None,
    }
}

fn apply_cursor(icon: CursorIcon) {
    unsafe {
        match cursor_for(icon) {
            Some(name) => {
                if let Ok(cursor) = LoadCursorW(None, name) {
                    SetCursor(Some(cursor));
                }
            }
            None => {
                SetCursor(None);
            }
        }
    }
}

pub(crate) fn refresh_cursor(hwnd: HWND) {
    let raw = hwnd.0 as isize;
    let mut point = POINT::default();
    let mut client = RECT::default();
    let inside = unsafe {
        GetCursorPos(&mut point).is_ok()
            && ScreenToClient(hwnd, &mut point).as_bool()
            && GetClientRect(hwnd, &mut client).is_ok()
            && PtInRect(&client, point).as_bool()
            && WindowFromPoint(POINT {
                x: point.x,
                y: point.y,
            }) != HWND::default()
    };
    if inside {
        if let Some(icon) = state::window(raw, |w| w.cursor) {
            apply_cursor(icon);
        }
    }
}

fn client_point(hwnd: HWND, lparam: LPARAM) -> (i32, i32) {
    let (x, y) = input::point_from_lparam(lparam.0);
    let mut point = POINT { x, y };
    let _ = unsafe { ScreenToClient(hwnd, &mut point) };
    (point.x, point.y)
}

fn raw_mouse(raw: isize, lparam: LPARAM) {
    let mut size = 0u32;
    let header = std::mem::size_of::<RAWINPUTHEADER>() as u32;
    let handle = HRAWINPUT(lparam.0 as *mut _);
    unsafe {
        GetRawInputData(handle, RID_INPUT, None, &mut size, header);
        let mut buffer = vec![0u8; size as usize];
        let read = GetRawInputData(
            handle,
            RID_INPUT,
            Some(buffer.as_mut_ptr().cast()),
            &mut size,
            header,
        );
        if read == size && size as usize >= std::mem::size_of::<RAWINPUT>().min(size as usize) {
            let input = &*(buffer.as_ptr() as *const RAWINPUT);
            if input.header.dwType == 0 {
                let mouse = input.data.mouse;
                state::emit(
                    raw,
                    ShellEvent::RawMouse {
                        dx: mouse.lLastX,
                        dy: mouse.lLastY,
                    },
                );
            }
        }
    }
}

fn min_track(hwnd: HWND, raw: isize) -> Option<(i32, i32)> {
    let (min, scale, chrome, kind) =
        state::window(raw, |w| (w.min_logical, w.scale, w.chrome, w.kind))?;
    let (width, height) = min?;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: (width * scale).round() as i32,
        bottom: (height * scale).round() as i32,
    };
    if kind == WindowKind::Normal && chrome == Chrome::Native {
        unsafe {
            let style = WINDOW_STYLE(GetWindowLongW(hwnd, GWL_STYLE) as u32);
            let ex = WINDOW_EX_STYLE(GetWindowLongW(hwnd, GWL_EXSTYLE) as u32);
            let _ = AdjustWindowRectExForDpi(&mut rect, style, false, ex, GetDpiForWindow(hwnd));
        }
    }
    Some((rect.right - rect.left, rect.bottom - rect.top))
}

pub(crate) unsafe extern "system" fn wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let raw = hwnd.0 as isize;
    if !state::with(|s| s.windows.contains_key(&raw)) {
        return unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) };
    }
    let handled = match msg {
        WM_ERASEBKGND => Some(LRESULT(1)),
        WM_PAINT => {
            unsafe {
                let _ = ValidateRect(Some(hwnd), None);
            }
            state::window(raw, |w| w.redraw_pending = true);
            Some(LRESULT(0))
        }
        WM_WAKE => {
            state::with(|s| s.queue.push_back(Queued::Wake));
            Some(LRESULT(0))
        }
        WM_MOUSEACTIVATE => state::window(raw, |w| w.kind == WindowKind::Panel)
            .unwrap_or(false)
            .then_some(LRESULT(MA_NOACTIVATE_RESULT)),
        WM_GETMINMAXINFO => min_track(hwnd, raw).map(|(width, height)| {
            let info = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
            info.ptMinTrackSize = POINT {
                x: width,
                y: height,
            };
            LRESULT(0)
        }),
        WM_NCCALCSIZE if wparam.0 != 0 => {
            let custom = state::window(raw, |w| {
                w.chrome == Chrome::Custom && w.kind == WindowKind::Normal
            })
            .unwrap_or(false);
            custom.then(|| {
                if unsafe { IsZoomed(hwnd).as_bool() } {
                    let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
                    let border = frame_border(hwnd);
                    params.rgrc[0].left += border;
                    params.rgrc[0].right -= border;
                    params.rgrc[0].top += border;
                    params.rgrc[0].bottom -= border;
                }
                LRESULT(0)
            })
        }
        WM_NCHITTEST => hit_test(hwnd, raw, lparam).map(|code| LRESULT(code as isize)),
        WM_NCMOUSEMOVE => {
            let custom = state::window(raw, |w| w.chrome == Chrome::Custom).unwrap_or(false);
            if custom {
                track(hwnd, raw, true);
                set_hover(raw, caption_for(wparam.0 as i32));
                let point = client_point(hwnd, lparam);
                emit_input(raw, InputEvent::PointerMove(pointer(raw, point)));
            }
            None
        }
        WM_NCMOUSELEAVE => {
            state::window(raw, |w| w.tracking_nc = false);
            set_hover(raw, None);
            None
        }
        WM_NCLBUTTONDOWN if caption_for(wparam.0 as i32).is_some() => Some(LRESULT(0)),
        WM_NCLBUTTONUP if caption_for(wparam.0 as i32).is_some() => {
            let command = match wparam.0 as i32 {
                ht::MINBUTTON => SC_MINIMIZE,
                ht::MAXBUTTON if unsafe { IsZoomed(hwnd).as_bool() } => SC_RESTORE,
                ht::MAXBUTTON => SC_MAXIMIZE,
                _ => SC_CLOSE,
            };
            let _ = unsafe {
                PostMessageW(
                    Some(hwnd),
                    WM_SYSCOMMAND,
                    WPARAM(command as usize),
                    LPARAM(0),
                )
            };
            Some(LRESULT(0))
        }
        WM_SETCURSOR if (lparam.0 & 0xFFFF) as i32 == ht::CLIENT => {
            let icon = state::window(raw, |w| w.cursor).unwrap_or(CursorIcon::Default);
            apply_cursor(icon);
            Some(LRESULT(1))
        }
        WM_SIZE => {
            let width = (lparam.0 & 0xFFFF) as u32;
            let height = ((lparam.0 >> 16) & 0xFFFF) as u32;
            let minimized = wparam.0 as u32 == SIZE_MINIMIZED;
            let was = state::window(raw, |w| std::mem::replace(&mut w.minimized, minimized))
                .unwrap_or(false);
            if minimized != was {
                state::emit(raw, ShellEvent::Minimized(minimized));
            }
            if !minimized {
                state::emit(
                    raw,
                    ShellEvent::Maximized(wparam.0 as u32 == SIZE_MAXIMIZED),
                );
                state::emit(raw, ShellEvent::Resized { width, height });
                if state::with(|s| s.in_modal) {
                    turn();
                }
            }
            Some(LRESULT(0))
        }
        WM_MOVE => {
            let mut rect = RECT::default();
            let _ = unsafe { GetWindowRect(hwnd, &mut rect) };
            state::emit(
                raw,
                ShellEvent::Moved {
                    x: rect.left,
                    y: rect.top,
                },
            );
            None
        }
        WM_DPICHANGED => {
            let dpi = (wparam.0 & 0xFFFF) as u32;
            let scale = f64::from(dpi) / 96.0;
            state::window(raw, |w| {
                w.scale = scale;
                w.rects = regions::drag_rects(&w.regions, scale);
            });
            let suggested = unsafe { &*(lparam.0 as *const RECT) };
            unsafe {
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    suggested.left,
                    suggested.top,
                    suggested.right - suggested.left,
                    suggested.bottom - suggested.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            state::emit(raw, ShellEvent::ScaleChanged { scale });
            Some(LRESULT(0))
        }
        WM_SETTINGCHANGE | WM_THEMECHANGED | WM_DWMCOMPOSITIONCHANGED | WM_SYSCOLORCHANGE => {
            state::emit(raw, ShellEvent::PreferencesChanged);
            None
        }
        WM_ENTERSIZEMOVE => {
            state::with(|s| s.in_modal = true);
            unsafe { SetTimer(Some(hwnd), MODAL_TIMER, 10, None) };
            None
        }
        WM_EXITSIZEMOVE => {
            state::with(|s| s.in_modal = false);
            let _ = unsafe { KillTimer(Some(hwnd), MODAL_TIMER) };
            None
        }
        WM_TIMER if wparam.0 == MODAL_TIMER => {
            turn();
            Some(LRESULT(0))
        }
        WM_SETFOCUS => {
            state::emit(raw, ShellEvent::Focused(true));
            sync_modifiers(raw);
            None
        }
        WM_KILLFOCUS => {
            state::emit(raw, ShellEvent::Focused(false));
            let changed = state::window(raw, |w| {
                std::mem::replace(&mut w.modifiers, Modifiers::NONE) != Modifiers::NONE
            })
            .unwrap_or(false);
            if changed {
                emit_input(raw, InputEvent::ModifiersChanged(Modifiers::NONE));
            }
            None
        }
        WM_CAPTURECHANGED => {
            let lost = state::window(raw, |w| w.buttons_down != 0 && lparam.0 != hwnd.0 as isize)
                .unwrap_or(false);
            if lost {
                state::window(raw, |w| w.buttons_down = 0);
                emit_input(raw, InputEvent::PointerCancel { contact: 0 });
            }
            None
        }
        WM_CLOSE => {
            state::emit(raw, ShellEvent::CloseRequested);
            Some(LRESULT(0))
        }
        WM_DESTROY => {
            state::emit(raw, ShellEvent::Destroyed);
            let empty = state::with(|s| {
                s.windows.remove(&raw);
                s.windows.is_empty()
            });
            if empty {
                unsafe { PostQuitMessage(0) };
            }
            Some(LRESULT(0))
        }
        WM_MOUSEMOVE => {
            let entered = state::window(raw, |w| !w.tracking_client).unwrap_or(false);
            track(hwnd, raw, false);
            set_hover(raw, None);
            if entered {
                state::emit(raw, ShellEvent::PointerEntered);
            }
            sync_modifiers(raw);
            let event = pointer(raw, input::point_from_lparam(lparam.0));
            emit_input(raw, InputEvent::PointerMove(event));
            Some(LRESULT(0))
        }
        WM_MOUSELEAVE => {
            state::window(raw, |w| w.tracking_client = false);
            state::emit(raw, ShellEvent::PointerLeft);
            Some(LRESULT(0))
        }
        WM_LBUTTONDOWN | WM_LBUTTONUP => {
            pointer_button(
                hwnd,
                raw,
                lparam,
                PointerButtonSource::Left,
                msg == WM_LBUTTONDOWN,
            );
            Some(LRESULT(0))
        }
        WM_RBUTTONDOWN | WM_RBUTTONUP => {
            pointer_button(
                hwnd,
                raw,
                lparam,
                PointerButtonSource::Right,
                msg == WM_RBUTTONDOWN,
            );
            Some(LRESULT(0))
        }
        WM_MBUTTONDOWN | WM_MBUTTONUP => {
            pointer_button(
                hwnd,
                raw,
                lparam,
                PointerButtonSource::Middle,
                msg == WM_MBUTTONDOWN,
            );
            Some(LRESULT(0))
        }
        WM_XBUTTONDOWN | WM_XBUTTONUP => {
            let which = PointerButtonSource::X(((wparam.0 >> 16) & 0xFFFF) as u16);
            pointer_button(hwnd, raw, lparam, which, msg == WM_XBUTTONDOWN);
            Some(LRESULT(1))
        }
        WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
            sync_modifiers(raw);
            let delta = input::scroll(msg == WM_MOUSEWHEEL, input::wheel_delta(wparam.0));
            let at = logical(raw, client_point(hwnd, lparam));
            emit_input(raw, InputEvent::Scroll { at, delta });
            Some(LRESULT(0))
        }
        WM_KEYDOWN | WM_KEYUP => {
            key(raw, wparam, lparam, msg == WM_KEYDOWN);
            Some(LRESULT(0))
        }
        WM_SYSKEYDOWN | WM_SYSKEYUP => {
            key(raw, wparam, lparam, msg == WM_SYSKEYDOWN);
            None
        }
        WM_SYSCHAR if wparam.0 != 0x20 => Some(LRESULT(0)),
        WM_CHAR => {
            let modifiers = state::window(raw, |w| w.modifiers).unwrap_or(Modifiers::NONE);
            let text = state::window(raw, |w| w.text.push(wparam.0 as u16)).flatten();
            if let Some(text) = text.filter(|_| !input::is_shortcut(modifiers)) {
                emit_input(raw, InputEvent::TextInput { text });
            }
            Some(LRESULT(0))
        }
        WM_INPUT => {
            if state::window(raw, |w| w.raw_mouse).unwrap_or(false) {
                raw_mouse(raw, lparam);
            }
            None
        }
        _ => None,
    };
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}
