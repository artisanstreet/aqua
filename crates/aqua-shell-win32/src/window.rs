use crate::event::{Chrome, ShellError, ShellEvent, WindowKind, WindowOptions, win32};
use crate::handle::NativeHandle;
use crate::looping::Waker;
use crate::material::{self, MaterialOutcome};
use crate::state::{self, WindowState};
use crate::wndproc::{self, CLASS_NAME, hwnd_from};
use aqua_core::{CursorIcon, Modifiers};
use aqua_protocol::chrome::DragRects;
use aqua_protocol::{DragRegions, WindowId, WindowMaterial};
use std::sync::Once;
use windows::Win32::Foundation::{HINSTANCE, HWND, RECT};
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, GetDC, GetMonitorInfoW,
    MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromWindow, ReleaseDC, SetDIBitsToDevice,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    AdjustWindowRectExForDpi, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetDpiForSystem,
    GetDpiForWindow, GetSystemMetricsForDpi, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::HiDpi::{GetDpiForMonitor, MDT_EFFECTIVE_DPI};
use windows::Win32::UI::Input::{RAWINPUTDEVICE, RIDEV_REMOVE, RegisterRawInputDevices};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::PCWSTR;

static INIT: Once = Once::new();

pub(crate) fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

fn init() {
    INIT.call_once(|| unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let module = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc::wndproc),
            hInstance: HINSTANCE(module.0),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: PCWSTR(CLASS_NAME.as_ptr()),
            ..Default::default()
        };
        RegisterClassExW(&class);
    });
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub(crate) hwnd: isize,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Monitor {
    pub bounds: (i32, i32, i32, i32),
    pub work_area: (i32, i32, i32, i32),
    pub scale: f64,
}

fn style_for(options: &WindowOptions) -> WINDOW_STYLE {
    let mut style = match options.kind {
        WindowKind::Normal => WS_OVERLAPPEDWINDOW,
        WindowKind::Panel => WS_POPUP,
    };
    if options.kind == WindowKind::Normal {
        if !options.resizable {
            style &= !(WS_THICKFRAME | WS_MAXIMIZEBOX);
        }
        if !options.maximizable {
            style &= !WS_MAXIMIZEBOX;
        }
        if !options.minimizable {
            style &= !WS_MINIMIZEBOX;
        }
    }
    style
}

fn ex_style_for(options: &WindowOptions) -> WINDOW_EX_STYLE {
    let panel = match options.kind {
        WindowKind::Panel => WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
        WindowKind::Normal => WINDOW_EX_STYLE(0),
    };
    let topmost = if options.topmost {
        WS_EX_TOPMOST
    } else {
        WINDOW_EX_STYLE(0)
    };
    let alpha = if options.alpha_surface {
        WS_EX_NOREDIRECTIONBITMAP
    } else {
        WINDOW_EX_STYLE(0)
    };
    panel | topmost | alpha
}

fn outer_size(
    options: &WindowOptions,
    style: WINDOW_STYLE,
    ex: WINDOW_EX_STYLE,
    dpi: u32,
) -> Result<(i32, i32), ShellError> {
    let scale = f64::from(dpi) / 96.0;
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: (options.logical_size.0 * scale).round() as i32,
        bottom: (options.logical_size.1 * scale).round() as i32,
    };
    if options.kind == WindowKind::Normal && options.chrome == Chrome::Native {
        unsafe { AdjustWindowRectExForDpi(&mut rect, style, false, ex, dpi) }
            .map_err(|e| win32("AdjustWindowRectExForDpi", e))?;
    }
    Ok((rect.right - rect.left, rect.bottom - rect.top))
}

impl Window {
    pub fn create(options: &WindowOptions) -> Result<Window, ShellError> {
        init();
        unsafe {
            let style = style_for(options);
            let ex = ex_style_for(options);
            let system_dpi = GetDpiForSystem();
            let (width, height) = outer_size(options, style, ex, system_dpi)?;
            let module = GetModuleHandleW(None).map_err(|e| win32("GetModuleHandleW", e))?;
            let title = wide(&options.title);
            let (x, y) = options.position.unwrap_or((CW_USEDEFAULT, CW_USEDEFAULT));
            let hwnd = CreateWindowExW(
                ex,
                PCWSTR(CLASS_NAME.as_ptr()),
                PCWSTR(title.as_ptr()),
                style,
                x,
                y,
                width,
                height,
                options.owner.map(|o| HWND(o.raw() as *mut _)),
                None,
                Some(HINSTANCE(module.0)),
                None,
            )
            .map_err(|e| win32("CreateWindowExW", e))?;
            let raw = hwnd.0 as isize;
            let dpi = GetDpiForWindow(hwnd).max(96);
            let scale = f64::from(dpi) / 96.0;
            let id = WindowId::new(raw as u64).ok_or(ShellError::Win32("WindowId", 0))?;
            state::with(|s| {
                s.windows.insert(
                    raw,
                    WindowState {
                        id,
                        scale,
                        chrome: options.chrome,
                        kind: options.kind,
                        min_logical: options.min_logical_size,
                        modifiers: Modifiers::NONE,
                        resizable: options.resizable,
                        minimizable: options.minimizable,
                        regions: DragRegions::default(),
                        rects: DragRects::default(),
                        cursor: CursorIcon::Default,
                        redraw_pending: true,
                        resized_since_paint: false,
                        tracking_client: false,
                        tracking_nc: false,
                        hover: None,
                        buttons_down: 0,
                        raw_mouse: false,
                        minimized: false,
                        covered: false,
                        text: Default::default(),
                        touch_buttons: Default::default(),
                        composition: None,
                    },
                )
            });
            let window = Window { hwnd: raw };
            let _ = crate::drop_target::register(hwnd);
            if dpi != system_dpi {
                let (width, height) = outer_size(options, style, ex, dpi)?;
                let _ = SetWindowPos(
                    hwnd,
                    None,
                    0,
                    0,
                    width,
                    height,
                    SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            let _ = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE | SWP_FRAMECHANGED,
            );
            let custom = options.chrome == Chrome::Custom && options.kind == WindowKind::Normal;
            if custom {
                material::extend_frame(hwnd, false, true);
            }
            material::set_dark(hwnd, options.dark);
            if options.kind == WindowKind::Panel {
                material::round_corners(hwnd);
            }
            if options.material != WindowMaterial::Solid {
                material::apply(hwnd, options.material, custom);
            }
            let (width, height) = window.inner_size();
            state::emit(raw, ShellEvent::Resized { width, height });
            if options.visible {
                window.show();
            }
            Ok(window)
        }
    }

    fn hwnd(self) -> HWND {
        hwnd_from(self.hwnd)
    }

    pub fn id(self) -> WindowId {
        WindowId::new(self.hwnd as u64).expect("hwnd is never zero")
    }

    pub fn native_handle(self) -> NativeHandle {
        let module = unsafe { GetModuleHandleW(None).unwrap_or_default() };
        NativeHandle {
            hwnd: self.hwnd,
            hinstance: module.0 as isize,
        }
    }

    pub fn raw_hwnd(self) -> isize {
        self.hwnd
    }

    pub fn waker(self) -> Waker {
        Waker::new(self.hwnd)
    }

    pub fn scale(self) -> f64 {
        state::window(self.hwnd, |w| w.scale).unwrap_or(1.0)
    }

    pub fn inner_size(self) -> (u32, u32) {
        let mut rect = RECT::default();
        let _ = unsafe { GetClientRect(self.hwnd(), &mut rect) };
        (
            (rect.right - rect.left).max(0) as u32,
            (rect.bottom - rect.top).max(0) as u32,
        )
    }

    pub fn outer_rect(self) -> (i32, i32, i32, i32) {
        let mut rect = RECT::default();
        let _ = unsafe { GetWindowRect(self.hwnd(), &mut rect) };
        (rect.left, rect.top, rect.right, rect.bottom)
    }

    pub fn monitor(self) -> Option<Monitor> {
        let monitor = unsafe { MonitorFromWindow(self.hwnd(), MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        unsafe { GetMonitorInfoW(monitor, &mut info) }
            .as_bool()
            .then(|| {
                let (mut dpi_x, mut dpi_y) = (96u32, 96u32);
                let _ =
                    unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
                let tuple = |r: RECT| (r.left, r.top, r.right, r.bottom);
                Monitor {
                    bounds: tuple(info.rcMonitor),
                    work_area: tuple(info.rcWork),
                    scale: f64::from(dpi_x.max(96)) / 96.0,
                }
            })
    }

    pub fn click_settings(self) -> aqua_core::ClickSettings {
        use windows::Win32::UI::Input::KeyboardAndMouse::GetDoubleClickTime;
        let dpi = unsafe { GetDpiForWindow(self.hwnd()) }.max(96);
        let scale = self.scale() as f32;
        let metric = |index| unsafe { GetSystemMetricsForDpi(index, dpi) } as f32 / (2.0 * scale);
        let interval = std::time::Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
        aqua_core::ClickSettings::new(
            interval,
            aqua_core::Point::new(metric(SM_CXDOUBLECLK), metric(SM_CYDOUBLECLK)),
        )
        .unwrap_or_default()
    }

    pub fn set_title(self, title: &str) {
        let title = wide(title);
        let _ = unsafe { SetWindowTextW(self.hwnd(), PCWSTR(title.as_ptr())) };
    }

    pub fn request_redraw(self) {
        state::window(self.hwnd, |w| w.redraw_pending = true);
    }

    pub fn set_drag_regions(self, regions: &DragRegions) {
        state::window(self.hwnd, |w| {
            w.rects = crate::regions::drag_rects(regions, w.scale);
            w.regions = regions.clone();
        });
    }

    pub fn set_cursor(self, cursor: CursorIcon) {
        state::window(self.hwnd, |w| w.cursor = cursor);
        crate::wndproc::refresh_cursor(self.hwnd());
    }

    pub fn set_material(self, material: WindowMaterial) -> MaterialOutcome {
        let custom = state::window(self.hwnd, |w| {
            w.chrome == Chrome::Custom && w.kind == WindowKind::Normal
        })
        .unwrap_or(false);
        material::apply(self.hwnd(), material, custom)
    }

    pub fn attach_composition_surface(
        self,
        surface: &windows::core::IUnknown,
    ) -> Result<(), ShellError> {
        let existing = state::window(self.hwnd, |w| w.composition.is_some()).unwrap_or(false);
        if !existing {
            let composition = crate::composition::Composition::create(self.hwnd())?;
            state::window(self.hwnd, |w| w.composition = Some(composition));
        }
        state::window(self.hwnd, |w| {
            w.composition
                .as_ref()
                .map(|composition| composition.set_content(surface))
        })
        .flatten()
        .unwrap_or(Err(ShellError::Win32("attach_composition_surface", 0)))
    }

    pub fn is_covered(self) -> bool {
        crate::occlusion::is_covered(self.hwnd())
    }

    pub fn has_alpha_surface(self) -> bool {
        let ex = unsafe { GetWindowLongW(self.hwnd(), GWL_EXSTYLE) } as u32;
        ex & WS_EX_NOREDIRECTIONBITMAP.0 != 0
    }

    pub fn backdrop_attribute(self) -> Option<i32> {
        material::backdrop_attribute(self.hwnd())
    }

    pub fn set_dark(self, dark: bool) -> bool {
        material::set_dark(self.hwnd(), dark)
    }

    pub fn set_topmost(self, topmost: bool) {
        let order = if topmost {
            HWND_TOPMOST
        } else {
            HWND_NOTOPMOST
        };
        let _ = unsafe {
            SetWindowPos(
                self.hwnd(),
                Some(order),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }

    pub fn set_position(self, x: i32, y: i32) {
        let _ = unsafe {
            SetWindowPos(
                self.hwnd(),
                None,
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub fn set_logical_size(self, width: f64, height: f64) {
        let scale = self.scale();
        let kind = state::window(self.hwnd, |w| (w.kind, w.chrome));
        let mut rect = RECT {
            left: 0,
            top: 0,
            right: (width * scale).round() as i32,
            bottom: (height * scale).round() as i32,
        };
        if let Some((WindowKind::Normal, Chrome::Native)) = kind {
            unsafe {
                let style = WINDOW_STYLE(GetWindowLongW(self.hwnd(), GWL_STYLE) as u32);
                let ex = WINDOW_EX_STYLE(GetWindowLongW(self.hwnd(), GWL_EXSTYLE) as u32);
                let _ = AdjustWindowRectExForDpi(
                    &mut rect,
                    style,
                    false,
                    ex,
                    GetDpiForWindow(self.hwnd()),
                );
            }
        }
        let _ = unsafe {
            SetWindowPos(
                self.hwnd(),
                None,
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
        };
    }

    pub fn minimize(self) {
        let _ = unsafe { ShowWindow(self.hwnd(), SW_MINIMIZE) };
    }

    pub fn toggle_maximize(self) {
        let command = if self.is_maximized() {
            SW_RESTORE
        } else {
            SW_MAXIMIZE
        };
        let _ = unsafe { ShowWindow(self.hwnd(), command) };
    }

    pub fn is_maximized(self) -> bool {
        unsafe { IsZoomed(self.hwnd()).as_bool() }
    }

    pub fn is_foreground(self) -> bool {
        unsafe { GetForegroundWindow() == self.hwnd() }
    }

    pub fn show(self) {
        let activate = state::window(self.hwnd, |w| w.kind == WindowKind::Normal).unwrap_or(true);
        let command = if activate { SW_SHOW } else { SW_SHOWNOACTIVATE };
        let _ = unsafe { ShowWindow(self.hwnd(), command) };
    }

    pub fn present_bgra(self, width: u32, height: u32, pixels: &[u8]) -> bool {
        let expected = (width as usize) * (height as usize) * 4;
        if width == 0 || height == 0 || pixels.len() < expected {
            return false;
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: width as i32,
                biHeight: -(height as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        unsafe {
            let dc = GetDC(Some(self.hwnd()));
            let copied = SetDIBitsToDevice(
                dc,
                0,
                0,
                width,
                height,
                0,
                0,
                0,
                height,
                pixels.as_ptr().cast(),
                &info,
                DIB_RGB_COLORS,
            );
            ReleaseDC(Some(self.hwnd()), dc);
            copied != 0
        }
    }

    pub fn send_to_back(self) {
        let _ = unsafe {
            SetWindowPos(
                self.hwnd(),
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            )
        };
    }

    pub fn show_inactive(self) {
        let _ = unsafe { ShowWindow(self.hwnd(), SW_SHOWNOACTIVATE) };
    }

    pub fn hide(self) {
        let _ = unsafe { ShowWindow(self.hwnd(), SW_HIDE) };
    }

    pub fn is_visible(self) -> bool {
        unsafe { IsWindowVisible(self.hwnd()).as_bool() }
    }

    pub fn destroy(self) {
        let _ = unsafe { DestroyWindow(self.hwnd()) };
    }

    pub fn set_raw_mouse(self, enabled: bool) {
        let device = RAWINPUTDEVICE {
            usUsagePage: 0x01,
            usUsage: 0x02,
            dwFlags: if enabled {
                Default::default()
            } else {
                RIDEV_REMOVE
            },
            hwndTarget: if enabled {
                self.hwnd()
            } else {
                HWND::default()
            },
        };
        let registered = unsafe {
            RegisterRawInputDevices(&[device], std::mem::size_of::<RAWINPUTDEVICE>() as u32)
        };
        if registered.is_ok() {
            state::window(self.hwnd, |w| w.raw_mouse = enabled);
        }
    }
}
