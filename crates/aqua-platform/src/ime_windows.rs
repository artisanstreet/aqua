use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::ops::Range;
use std::rc::Rc;

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Input::Ime::{
    ImmAssociateContextEx, ImmGetCompositionStringW, ImmGetContext, ImmNotifyIME,
    ImmReleaseContext, ImmSetCandidateWindow, ImmSetCompositionWindow, CANDIDATEFORM,
    CFS_CANDIDATEPOS, CFS_POINT, COMPOSITIONFORM, CPS_COMPLETE, GCS_COMPATTR, GCS_COMPSTR,
    GCS_CURSORPOS, GCS_RESULTSTR, HIMC, IACE_DEFAULT, IME_COMPOSITION_STRING, NI_COMPOSITIONSTR,
};
use windows::Win32::UI::Input::KeyboardAndMouse::GetFocus;
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetPropW, GetWindowLongPtrW, GetWindowThreadProcessId,
    RemovePropW, SetPropW, SetWindowLongPtrW, GWLP_WNDPROC, WM_IME_COMPOSITION,
    WM_IME_ENDCOMPOSITION, WM_IME_SETCONTEXT, WM_IME_STARTCOMPOSITION, WM_NCDESTROY, WNDPROC,
};

use crate::ime::{handle_composition, CompositionSource, CompositionTarget};
use crate::services::{gpui_ime_position, ImeCursorArea};

pub trait NativeImeClient {
    fn selected_bounds(&self) -> Option<ImeCursorArea>;
    fn replace(&self, text: &str, selected_utf16: Option<Range<usize>>);
}

struct State {
    previous: Cell<isize>,
    enabled: Cell<bool>,
    destroyed: Cell<bool>,
    client: RefCell<Option<Rc<dyn NativeImeClient>>>,
    scale: Cell<f64>,
}

pub struct NativeImeWindow {
    hwnd: HWND,
    state: Rc<State>,
    thread: PhantomData<*mut ()>,
}

fn property() -> PCWSTR {
    w!("AquaNativeImeState")
}

fn proc_address() -> isize {
    window_proc as *const () as usize as isize
}

fn state(hwnd: HWND) -> Option<Rc<State>> {
    let raw = unsafe { GetPropW(hwnd, property()) }.0 as *const State;
    if raw.is_null() {
        return None;
    }
    unsafe {
        Rc::increment_strong_count(raw);
        Some(Rc::from_raw(raw))
    }
}

fn reclaim(hwnd: HWND) {
    if let Ok(value) = unsafe { RemovePropW(hwnd, property()) } {
        if !value.0.is_null() {
            unsafe {
                drop(Rc::from_raw(value.0 as *const State));
            }
        }
    }
}

fn chain(previous: isize, hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if previous == 0 {
            DefWindowProcW(hwnd, message, wparam, lparam)
        } else {
            CallWindowProcW(
                std::mem::transmute::<isize, WNDPROC>(previous),
                hwnd,
                message,
                wparam,
                lparam,
            )
        }
    }
}

impl NativeImeWindow {
    pub fn install(window: &winit::window::Window) -> Option<Self> {
        let RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw() else {
            return None;
        };
        let hwnd = HWND(handle.hwnd.get() as *mut _);
        if hwnd.0.is_null()
            || unsafe { GetWindowThreadProcessId(hwnd, None) != GetCurrentThreadId() }
        {
            return None;
        }
        if !unsafe { GetPropW(hwnd, property()) }.0.is_null() {
            return None;
        }
        let previous = unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) };
        if previous == 0 {
            return None;
        }
        let state = Rc::new(State {
            previous: Cell::new(previous),
            enabled: Cell::new(false),
            destroyed: Cell::new(false),
            client: RefCell::new(None),
            scale: Cell::new(window.scale_factor()),
        });
        let registration = Rc::into_raw(state.clone());
        if unsafe { SetPropW(hwnd, property(), Some(HANDLE(registration as *mut _))) }.is_err() {
            unsafe {
                drop(Rc::from_raw(registration));
            }
            return None;
        }
        let previous = unsafe { SetWindowLongPtrW(hwnd, GWLP_WNDPROC, proc_address()) };
        if previous == 0 {
            reclaim(hwnd);
            return None;
        }
        state.previous.set(previous);
        Some(Self {
            hwnd,
            state,
            thread: PhantomData,
        })
    }

    pub fn set_client(&self, client: Option<Rc<dyn NativeImeClient>>, scale: f64) {
        if self.state.destroyed.get() {
            return;
        }
        self.state.scale.set(scale);
        let enabled = client.is_some();
        if self.state.enabled.replace(enabled) == enabled {
            *self.state.client.borrow_mut() = client;
            return;
        }
        unsafe {
            if enabled {
                *self.state.client.borrow_mut() = client;
                let _ = ImmAssociateContextEx(self.hwnd, HIMC::default(), IACE_DEFAULT);
            } else {
                if GetFocus() == self.hwnd {
                    if let Some(context) = Context::get(self.hwnd) {
                        let _ = ImmNotifyIME(context.himc, NI_COMPOSITIONSTR, CPS_COMPLETE, 0);
                    }
                }
                let _ = ImmAssociateContextEx(self.hwnd, HIMC::default(), 0);
                *self.state.client.borrow_mut() = None;
            }
        }
    }
}

impl Drop for NativeImeWindow {
    fn drop(&mut self) {
        if self.state.destroyed.get() {
            return;
        }
        self.set_client(None, self.state.scale.get());
        if unsafe { GetWindowLongPtrW(self.hwnd, GWLP_WNDPROC) } == proc_address()
            && unsafe { SetWindowLongPtrW(self.hwnd, GWLP_WNDPROC, self.state.previous.get()) } != 0
        {
            reclaim(self.hwnd);
        }
    }
}

struct Context {
    hwnd: HWND,
    himc: HIMC,
}

impl Context {
    fn get(hwnd: HWND) -> Option<Self> {
        let himc = unsafe { ImmGetContext(hwnd) };
        (!himc.0.is_null()).then_some(Self { hwnd, himc })
    }

    fn bytes(&self, kind: IME_COMPOSITION_STRING) -> Option<Vec<u8>> {
        let len = unsafe { ImmGetCompositionStringW(self.himc, kind, None, 0) };
        if len < 0 {
            return None;
        }
        let mut data = vec![0; len as usize];
        if len > 0 {
            unsafe {
                ImmGetCompositionStringW(
                    self.himc,
                    kind,
                    Some(data.as_mut_ptr().cast()),
                    len as u32,
                );
            }
        }
        Some(data)
    }

    fn utf16(&self, kind: IME_COMPOSITION_STRING) -> Option<Vec<u16>> {
        Some(
            self.bytes(kind)?
                .as_chunks::<2>()
                .0
                .iter()
                .map(|bytes| u16::from_ne_bytes([bytes[0], bytes[1]]))
                .collect(),
        )
    }
}

impl Drop for Context {
    fn drop(&mut self) {
        unsafe {
            let _ = ImmReleaseContext(self.hwnd, self.himc);
        }
    }
}

impl CompositionSource for Context {
    fn result(&mut self) -> Option<Vec<u16>> {
        self.utf16(GCS_RESULTSTR)
    }
    fn composition(&mut self) -> Option<Vec<u16>> {
        self.utf16(GCS_COMPSTR)
    }
    fn cursor(&mut self) -> usize {
        unsafe { ImmGetCompositionStringW(self.himc, GCS_CURSORPOS, None, 0) as usize }
    }
    fn attributes(&mut self) -> Option<Vec<u8>> {
        let len = unsafe { ImmGetCompositionStringW(self.himc, GCS_COMPATTR, None, 0) };
        if len <= 0 {
            return None;
        }
        let mut attributes = vec![0; len as usize];
        let read = unsafe {
            ImmGetCompositionStringW(
                self.himc,
                GCS_COMPATTR,
                Some(attributes.as_mut_ptr().cast()),
                len as u32,
            )
        };
        (read > 0).then_some(attributes)
    }
}

struct Target(Rc<dyn NativeImeClient>);
impl CompositionTarget for Target {
    fn replace(&mut self, text: &str, selected: Option<Range<usize>>) {
        self.0.replace(text, selected);
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        dispatch(hwnd, message, wparam, lparam)
    }))
    .unwrap_or_else(|_| {
        chain(
            state(hwnd).map_or(0, |state| state.previous.get()),
            hwnd,
            message,
            wparam,
            lparam,
        )
    })
}

fn dispatch(hwnd: HWND, message: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let Some(state) = state(hwnd) else {
        return chain(0, hwnd, message, wparam, lparam);
    };
    let previous = state.previous.get();
    if message == WM_NCDESTROY {
        state.destroyed.set(true);
        reclaim(hwnd);
        return chain(previous, hwnd, message, wparam, lparam);
    }
    let client = state.client.borrow().clone();
    if let Some(client) = client {
        match message {
            WM_IME_STARTCOMPOSITION => {
                if let Some((x, y)) = client
                    .selected_bounds()
                    .and_then(|area| gpui_ime_position(area, state.scale.get()))
                {
                    if let Some(context) = Context::get(hwnd) {
                        let point = POINT { x, y };
                        let composition = COMPOSITIONFORM {
                            dwStyle: CFS_POINT,
                            ptCurrentPos: point,
                            ..Default::default()
                        };
                        let candidate = CANDIDATEFORM {
                            dwStyle: CFS_CANDIDATEPOS,
                            ptCurrentPos: point,
                            ..Default::default()
                        };
                        unsafe {
                            let _ = ImmSetCompositionWindow(context.himc, &composition);
                            let _ = ImmSetCandidateWindow(context.himc, &candidate);
                        }
                    }
                }
                return LRESULT(0);
            }
            WM_IME_COMPOSITION => {
                if let Some(mut context) = Context::get(hwnd) {
                    if let Some(result) =
                        handle_composition(lparam.0 as u32, &mut context, &mut Target(client))
                    {
                        return LRESULT(result);
                    }
                }
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) };
            }
            WM_IME_ENDCOMPOSITION | WM_IME_SETCONTEXT => {
                return unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
            }
            _ => {}
        }
    }
    chain(previous, hwnd, message, wparam, lparam)
}
