#![allow(dead_code, unsafe_code)]

use aqua_graph::Extent;
use windows::core::w;
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DestroyWindow, DispatchMessageW, GetClientRect, PeekMessageW, SetWindowPos,
    ShowWindow, TranslateMessage, HWND_BOTTOM, MSG, PM_REMOVE, SWP_NOACTIVATE, SWP_NOMOVE,
    SWP_NOSIZE, SW_SHOWNOACTIVATE, WS_EX_NOACTIVATE, WS_POPUP,
};

pub struct Window(pub HWND);

impl Window {
    pub fn open(extent: Extent) -> Self {
        let hwnd = unsafe {
            CreateWindowExW(
                WS_EX_NOACTIVATE,
                w!("STATIC"),
                w!("aqua-window-budget"),
                WS_POPUP,
                0,
                0,
                extent.width as i32,
                extent.height as i32,
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
                Some(HWND_BOTTOM),
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        let window = Self(hwnd);
        window.pump();
        window
    }

    pub fn client(&self) -> Extent {
        let mut rect = RECT::default();
        unsafe { GetClientRect(self.0, &mut rect).expect("client rect") };
        Extent {
            width: (rect.right - rect.left) as u32,
            height: (rect.bottom - rect.top) as u32,
        }
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

impl Drop for Window {
    fn drop(&mut self) {
        unsafe {
            let _ = DestroyWindow(self.0);
        }
    }
}
