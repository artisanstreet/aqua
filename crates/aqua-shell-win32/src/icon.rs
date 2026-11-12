use std::path::Path;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileIcon {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

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
                let bgra = std::slice::from_raw_parts(bits.cast::<u8>(), (edge * edge * 4) as usize);
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

pub fn straight_rgba(premultiplied_bgra: &[u8]) -> Vec<u8> {
    let without_alpha = premultiplied_bgra
        .chunks_exact(4)
        .all(|pixel| pixel[3] == 0);
    premultiplied_bgra
        .chunks_exact(4)
        .flat_map(|pixel| {
            let alpha = if without_alpha { 255 } else { pixel[3] };
            let straight = |channel: u8| match alpha {
                0 => 0,
                alpha => (u32::from(channel) * 255 / u32::from(alpha)).min(255) as u8,
            };
            [
                straight(pixel[2]),
                straight(pixel[1]),
                straight(pixel[0]),
                alpha,
            ]
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::straight_rgba;

    #[test]
    fn icon_pixels_become_straight_rgba_and_alpha_less_icons_turn_opaque() {
        assert_eq!(
            straight_rgba(&[0, 64, 128, 128, 0, 0, 0, 0]),
            [255, 127, 0, 128, 0, 0, 0, 0]
        );
        assert_eq!(
            straight_rgba(&[10, 20, 30, 0, 1, 2, 3, 0]),
            [30, 20, 10, 255, 3, 2, 1, 255]
        );
    }
}
