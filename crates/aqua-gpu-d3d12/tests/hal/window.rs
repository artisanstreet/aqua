use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::w;

pub struct TestWindow {
    pub hwnd: HWND,
}

impl TestWindow {
    pub fn open(width: i32, height: i32) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("aqua-hal-test"),
                WS_OVERLAPPEDWINDOW,
                80,
                80,
                width,
                height,
                None,
                None,
                None,
                None,
            )
        }
        .expect("window");
        unsafe {
            let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            let _ = SetWindowPos(
                hwnd,
                Some(HWND_TOPMOST),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let window = Self { hwnd };
        window.pump();
        window
    }

    pub fn raw(&self) -> isize {
        self.hwnd.0 as isize
    }

    pub fn client(&self) -> (u32, u32) {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.hwnd, &mut rect).expect("client rect") };
        (
            (rect.right - rect.left) as u32,
            (rect.bottom - rect.top) as u32,
        )
    }

    pub fn resize(&self, width: i32, height: i32) {
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                width,
                height,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            );
        }
        self.pump();
    }

    pub fn pump(&self) {
        let mut message = MSG::default();
        while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
            unsafe {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
    }
}

impl Drop for TestWindow {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.hwnd);
        }
    }
}
