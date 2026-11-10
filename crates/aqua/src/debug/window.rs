use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_core::Size;
use aqua_graph::Extent;
use aqua_protocol::{RenderCommand, RenderInput, WindowId};
use aqua_runtime::RenderSide;
use aqua_text::{SharedFonts, SharedRuns};
use windows::core::w;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    GetDpiForWindow, SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect, LoadCursorW,
    PeekMessageW, PostQuitMessage, RegisterClassW, TranslateMessage, CS_HREDRAW, CS_VREDRAW,
    CW_USEDEFAULT, IDC_ARROW, MSG, PM_REMOVE, WM_CLOSE, WM_DESTROY, WM_ERASEBKGND, WM_QUIT,
    WNDCLASSW, WS_EX_APPWINDOW, WS_OVERLAPPEDWINDOW, WS_VISIBLE,
};

use super::panel::{Panel, SURFACE};
use crate::render::host::RenderStats;
use crate::render::link::RenderContext;

const WIDTH: i32 = 480;
const HEIGHT: i32 = 960;
const TICK: Duration = Duration::from_millis(33);

static OPENED: AtomicBool = AtomicBool::new(false);

pub fn open(fonts: SharedFonts, runs: SharedRuns) {
    if OPENED.swap(true, Ordering::AcqRel) {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("AquaDebug".into())
        .spawn(move || run(fonts, runs));
    if let Err(error) = spawned {
        eprintln!("aqua: debug window thread failed: {error}");
    }
}

unsafe extern "system" fn procedure(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    match message {
        WM_CLOSE => {
            let _ = unsafe { DestroyWindow(hwnd) };
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1),
        _ => unsafe { DefWindowProcW(hwnd, message, wparam, lparam) },
    }
}

fn create() -> Option<HWND> {
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(procedure),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).ok()?,
            lpszClassName: w!("AquaDebugWindow"),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            WS_EX_APPWINDOW,
            w!("AquaDebugWindow"),
            w!("Aqua Debug"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            WIDTH,
            HEIGHT,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

fn pump() -> bool {
    let mut message = MSG::default();
    while unsafe { PeekMessageW(&mut message, None, 0, 0, PM_REMOVE) }.as_bool() {
        if message.message == WM_QUIT {
            return false;
        }
        unsafe {
            let _ = TranslateMessage(&message);
            DispatchMessageW(&message);
        }
    }
    true
}

fn viewport(hwnd: HWND) -> (Extent, Size, f32) {
    let mut client = RECT::default();
    let _ = unsafe { GetClientRect(hwnd, &mut client) };
    let scale = match unsafe { GetDpiForWindow(hwnd) } {
        0 => 1.0,
        dpi => dpi as f32 / 96.0,
    };
    let extent = Extent {
        width: u32::try_from(client.right - client.left)
            .unwrap_or(0)
            .max(1),
        height: u32::try_from(client.bottom - client.top)
            .unwrap_or(0)
            .max(1),
    };
    let size = Size {
        w: extent.width as f32 / scale,
        h: extent.height as f32 / scale,
    };
    (extent, size, scale)
}

fn run(fonts: SharedFonts, runs: SharedRuns) {
    let Some(hwnd) = create() else {
        eprintln!("aqua: debug window could not be created");
        return;
    };
    let raw = hwnd.0 as isize;
    let Some(id) = WindowId::new(raw as u64) else {
        return;
    };
    let (extent, _, _) = viewport(hwnd);
    let (_externals, receiver) = channel();
    let init = crate::native::gpu::factory(id, raw, extent, fonts.clone(), runs.clone(), SURFACE)(
        RenderContext {
            externals: receiver,
            stats: Arc::new(RenderStats::default()),
            composition: Arc::new(|_| {}),
        },
    );
    let mut target = match init() {
        Ok(target) => target,
        Err(error) => {
            eprintln!("aqua: debug window renderer failed: {error}");
            let _ = unsafe { DestroyWindow(hwnd) };
            return;
        }
    };
    let idle = Arc::new(RenderStats::default());
    let mut side = RenderSide::new();
    let mut panel = Panel::new(fonts, runs);
    let mut frame = 0;
    while pump() {
        let started = Instant::now();
        let (_, size, scale) = viewport(hwnd);
        side.step(RenderInput {
            commands: vec![RenderCommand::Viewport { size, scale }],
            feedback: Vec::new(),
            present_at: aqua_protocol::Instant::from_nanos(0),
        });
        let watched = super::watched();
        let stats = watched.as_ref().map_or(&idle, |watched| &watched.stats);
        let refresh = watched
            .as_ref()
            .and_then(|watched| crate::native::gpu::refresh_of(watched.hwnd));
        let observed = watched
            .as_ref()
            .map(|watched| viewport(HWND(watched.hwnd as *mut _)).1)
            .unwrap_or_default();
        side.set_overlay(panel.items(size, stats, refresh, observed));
        frame += 1;
        target.wait_frame();
        target.present(&mut side, frame);
        std::thread::sleep(TICK.saturating_sub(started.elapsed()));
    }
}
