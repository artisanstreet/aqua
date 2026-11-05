use crate::event::{ShellError, win32};
use crate::window::Window;
use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber,
    IsClipboardFormatAvailable, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

struct Open;

impl Open {
    fn acquire(owner: Option<HWND>) -> Result<Open, ShellError> {
        (0..5)
            .find_map(|attempt| {
                unsafe { OpenClipboard(owner) }
                    .is_ok()
                    .then(|| Open)
                    .or_else(|| {
                        std::thread::sleep(std::time::Duration::from_millis(2 << attempt));
                        None
                    })
            })
            .ok_or(ShellError::ClipboardBusy)
    }
}

impl Drop for Open {
    fn drop(&mut self) {
        let _ = unsafe { CloseClipboard() };
    }
}

pub fn is_empty() -> bool {
    unsafe { windows::Win32::System::DataExchange::CountClipboardFormats() == 0 }
}

pub fn sequence() -> u32 {
    unsafe { GetClipboardSequenceNumber() }
}

pub fn get_text() -> Result<Option<String>, ShellError> {
    let _open = Open::acquire(None)?;
    unsafe {
        if IsClipboardFormatAvailable(u32::from(CF_UNICODETEXT.0)).is_err() {
            return Ok(None);
        }
        let data = GetClipboardData(u32::from(CF_UNICODETEXT.0))
            .map_err(|e| win32("GetClipboardData", e))?;
        let global = HGLOBAL(data.0);
        let ptr = GlobalLock(global) as *const u16;
        if ptr.is_null() {
            return Err(ShellError::ClipboardUnavailable);
        }
        let mut len = 0;
        while *ptr.add(len) != 0 {
            len += 1;
        }
        let text = String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len));
        let _ = GlobalUnlock(global);
        Ok(Some(text))
    }
}

pub fn set_text(owner: Window, text: &str) -> Result<(), ShellError> {
    let units: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = units.len() * std::mem::size_of::<u16>();
    let _open = Open::acquire(Some(crate::wndproc::hwnd_from(owner.hwnd)))?;
    unsafe {
        EmptyClipboard().map_err(|e| win32("EmptyClipboard", e))?;
        let global = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| win32("GlobalAlloc", e))?;
        let ptr = GlobalLock(global) as *mut u16;
        if ptr.is_null() {
            let _ = GlobalFree(Some(global));
            return Err(ShellError::ClipboardUnavailable);
        }
        std::ptr::copy_nonoverlapping(units.as_ptr(), ptr, units.len());
        let _ = GlobalUnlock(global);
        match SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(global.0))) {
            Ok(_) => Ok(()),
            Err(e) => {
                let _ = GlobalFree(Some(global));
                Err(win32("SetClipboardData", e))
            }
        }
    }
}
