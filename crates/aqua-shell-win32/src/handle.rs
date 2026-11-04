use raw_window_handle::{
    DisplayHandle, HandleError, HasDisplayHandle, HasWindowHandle, RawWindowHandle,
    Win32WindowHandle, WindowHandle,
};
use std::num::NonZeroIsize;

#[derive(Debug, Clone, Copy)]
pub struct NativeHandle {
    pub(crate) hwnd: isize,
    pub(crate) hinstance: isize,
}

impl HasWindowHandle for NativeHandle {
    fn window_handle(&self) -> Result<WindowHandle<'_>, HandleError> {
        let hwnd = NonZeroIsize::new(self.hwnd).ok_or(HandleError::Unavailable)?;
        let mut handle = Win32WindowHandle::new(hwnd);
        handle.hinstance = NonZeroIsize::new(self.hinstance);
        Ok(unsafe { WindowHandle::borrow_raw(RawWindowHandle::Win32(handle)) })
    }
}

impl HasDisplayHandle for NativeHandle {
    fn display_handle(&self) -> Result<DisplayHandle<'_>, HandleError> {
        Ok(DisplayHandle::windows())
    }
}
