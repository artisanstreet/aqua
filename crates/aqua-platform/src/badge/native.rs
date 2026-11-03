use super::{BadgeError, TaskbarBadge};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{HSTRING, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER};
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{ITaskbarList3, TaskbarList};
use windows::Win32::UI::WindowsAndMessaging::*;

struct Dc(HDC);

impl Drop for Dc {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteDC(self.0);
        }
    }
}

struct Object(HGDIOBJ);

impl Drop for Object {
    fn drop(&mut self) {
        unsafe {
            let _ = DeleteObject(self.0);
        }
    }
}

struct Selected(HDC, HGDIOBJ);

impl Selected {
    fn new(dc: HDC, object: HGDIOBJ) -> Self {
        Self(dc, unsafe { SelectObject(dc, object) })
    }
}

impl Drop for Selected {
    fn drop(&mut self) {
        unsafe {
            SelectObject(self.0, self.1);
        }
    }
}

fn native_error(error: impl std::fmt::Display) -> BadgeError {
    BadgeError::Native(error.to_string())
}

pub fn install_taskbar_wake<T: 'static>(
    builder: &mut winit::event_loop::EventLoopBuilder<T>,
    mut wake: impl FnMut() + 'static,
) -> Result<(), BadgeError> {
    use winit::platform::windows::EventLoopBuilderExtWindows;
    let message = unsafe { RegisterWindowMessageW(windows::core::w!("TaskbarButtonCreated")) };
    if message == 0 {
        return Err(native_error(windows::core::Error::from_thread()));
    }
    builder.with_msg_hook(move |event| {
        if !event.is_null() && unsafe { (*event.cast::<MSG>()).message } == message {
            wake();
        }
        false
    });
    Ok(())
}

pub fn set_badge(
    window: &winit::window::Window,
    badge: Option<TaskbarBadge>,
) -> Result<(), BadgeError> {
    let _apartment = crate::apartment::Apartment::new().map_err(native_error)?;
    let handle = window.window_handle().map_err(native_error)?;
    let RawWindowHandle::Win32(handle) = handle.as_raw() else {
        return Err(BadgeError::Unsupported);
    };
    let hwnd = HWND(handle.hwnd.get() as *mut _);
    let taskbar: ITaskbarList3 =
        unsafe { CoCreateInstance(&TaskbarList, None, CLSCTX_INPROC_SERVER) }
            .map_err(native_error)?;
    unsafe { taskbar.HrInit() }.map_err(native_error)?;
    let Some(badge) = badge.filter(|badge| badge.count > 0) else {
        return unsafe { taskbar.SetOverlayIcon(hwnd, HICON::default(), PCWSTR::null()) }
            .map_err(native_error);
    };
    let dpi = unsafe { GetDpiForWindow(hwnd) }.max(96);
    let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, dpi) }.clamp(16, 256);
    let icon = raster_icon(badge, size).map_err(native_error)?;
    let result = unsafe {
        taskbar.SetOverlayIcon(
            hwnd,
            icon,
            &HSTRING::from(format!("{} unread", badge.count)),
        )
    };
    unsafe {
        let _ = DestroyIcon(icon);
    }
    result.map_err(native_error)
}

fn raster_icon(badge: TaskbarBadge, size: i32) -> windows::core::Result<HICON> {
    unsafe {
        let dc = Dc(CreateCompatibleDC(None));
        if dc.0 .0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size,
                biHeight: -size,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(dc.0), &info, DIB_RGB_COLORS, &mut bits, None, 0)?;
        let bitmap_guard = Object(bitmap.into());
        let bitmap_selection = Selected::new(dc.0, bitmap_guard.0);
        let length = size as usize * size as usize;
        std::ptr::write_bytes(bits.cast::<u8>(), 0, length * 4);
        let mut font = LOGFONTW {
            lfHeight: -(size * 11 / 16),
            lfWeight: FW_BOLD.0 as i32,
            lfCharSet: DEFAULT_CHARSET,
            lfQuality: ANTIALIASED_QUALITY,
            ..Default::default()
        };
        for (output, input) in font.lfFaceName.iter_mut().zip("Segoe UI".encode_utf16()) {
            *output = input;
        }
        let font = CreateFontIndirectW(&font);
        if font.0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let font_guard = Object(font.into());
        let font_selection = Selected::new(dc.0, font_guard.0);
        SetBkMode(dc.0, TRANSPARENT);
        SetTextColor(dc.0, COLORREF(0x00ffffff));
        let mut text: Vec<_> = badge.label().encode_utf16().collect();
        let mut bounds = RECT {
            left: 0,
            top: 0,
            right: size,
            bottom: size,
        };
        DrawTextW(
            dc.0,
            &mut text,
            &mut bounds,
            DT_CENTER | DT_VCENTER | DT_SINGLELINE | DT_NOPREFIX,
        );
        let _ = GdiFlush();
        let coverage: Vec<_> = std::slice::from_raw_parts(bits.cast::<[u8; 4]>(), length)
            .iter()
            .map(|pixel| pixel[1])
            .collect();
        let pixels = badge
            .compose(size as u32, &coverage)
            .ok_or_else(windows::core::Error::from_thread)?;
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast::<u8>(), pixels.len());
        drop(font_selection);
        drop(bitmap_selection);
        let stride = (size as usize).div_ceil(16) * 2;
        let mask_bits = vec![0u8; stride * size as usize];
        let mask = CreateBitmap(size, size, 1, 1, Some(mask_bits.as_ptr().cast()));
        if mask.0.is_null() {
            return Err(windows::core::Error::from_thread());
        }
        let _mask_guard = Object(mask.into());
        CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            hbmMask: mask,
            hbmColor: bitmap,
            ..Default::default()
        })
    }
}
