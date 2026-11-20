use std::path::Path;

pub use aqua_shell::icon::{straight_rgba, FileIcon};

#[cfg(not(windows))]
pub fn file_icon(_path: &Path, _edge: u32) -> Option<FileIcon> {
    None
}

#[cfg(windows)]
pub fn file_icon(path: &Path, edge: u32) -> Option<FileIcon> {
    use windows::core::HSTRING;
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    if edge == 0 || edge > 256 {
        return None;
    }
    let path = HSTRING::from(path.as_os_str());
    let mut info = SHFILEINFOW::default();
    let found = unsafe {
        SHGetFileInfoW(
            &path,
            FILE_FLAGS_AND_ATTRIBUTES(0),
            Some(&mut info),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        )
    };
    if found == 0 || info.hIcon.is_invalid() {
        return None;
    }
    let rgba = unsafe { rendered(info.hIcon, edge) };
    unsafe {
        let _ = DestroyIcon(info.hIcon);
    }
    Some(FileIcon {
        width: edge,
        height: edge,
        rgba: rgba?,
    })
}

#[cfg(windows)]
unsafe fn rendered(
    icon: windows::Win32::UI::WindowsAndMessaging::HICON,
    edge: u32,
) -> Option<Vec<u8>> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, CreateDIBSection, DeleteDC, DeleteObject, GdiFlush, GetDC, ReleaseDC,
        SelectObject, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DrawIconEx, DI_NORMAL};

    unsafe {
        let screen = GetDC(None::<HWND>);
        if screen.is_invalid() {
            return None;
        }
        let memory = CreateCompatibleDC(Some(screen));
        let header = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: edge as i32,
                biHeight: -(edge as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let bitmap = CreateDIBSection(Some(screen), &header, DIB_RGB_COLORS, &mut bits, None, 0);
        let pixels = match bitmap {
            Ok(bitmap) if !memory.is_invalid() && !bits.is_null() => {
                let previous = SelectObject(memory, bitmap.into());
                let drawn = DrawIconEx(
                    memory,
                    0,
                    0,
                    icon,
                    edge as i32,
                    edge as i32,
                    0,
                    None,
                    DI_NORMAL,
                )
                .is_ok();
                let _ = GdiFlush();
                let bgra =
                    std::slice::from_raw_parts(bits.cast::<u8>(), (edge * edge * 4) as usize);
                let rgba = straight_rgba(bgra);
                SelectObject(memory, previous);
                let _ = DeleteObject(bitmap.into());
                drawn.then_some(rgba)
            }
            Ok(bitmap) => {
                let _ = DeleteObject(bitmap.into());
                None
            }
            Err(_) => None,
        };
        if !memory.is_invalid() {
            let _ = DeleteDC(memory);
        }
        ReleaseDC(None::<HWND>, screen);
        pixels
    }
}
