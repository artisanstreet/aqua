//! Windows overlay guard: `user32` window-proc chain implementing the overlay.
//!
//! `cfg(windows)`-gated. Deliberately NO `comctl32` subclass helpers:
//! `SetWindowSubclass` et al. are v6-only exports, and statically importing
//! them kills the process at loader time (`0xC0000139`) on any system that
//! resolves `comctl32.dll` without an activation manifest — and would force
//! every downstream Aqua app to embed one. The `user32` chain below imports
//! only ever-present entry points (`SetWindowLongPtrW`, `CallWindowProcW`,
//! `GetPropW`/`SetPropW`, DWM APIs).
//!
//! Message flow follows the primary MS reference "Custom Window Frame Using
//! DWM" (`windows/win32/dwm/customframe`), adapted to a chained proc:
//! - `WM_NCCALCSIZE` (`wParam == TRUE`) -> full client, return 0. Initial
//!   frame applied with `SetWindowPos(..., SWP_FRAMECHANGED)` at install.
//! - **Every** message goes to `DwmDefWindowProc` FIRST; only when it
//!   returns FALSE does the overlay handle it (native buttons incl.
//!   hover/snap, `WM_NCMOUSELEAVE` stay alive).
//! - Unhandled messages chain to the PREVIOUS proc via `CallWindowProcW`
//!   (normally winit's) — never `DefWindowProcW` directly, except as a
//!   last-resort fallback when no previous proc is known.
//! - `WM_NCHITTEST` unhandled by DWM -> shared pure [`hit_test_chrome`](crate::chrome::hit_test_chrome).
//! - `WM_ACTIVATE` -> `DwmExtendFrameIntoClientArea` top margin.
//! - Maximized `WM_NCCALCSIZE` clamps to the monitor work area (all edges).
//! - `WM_DPICHANGED` refreshes band, glass margin, DPI cache.
//!
//! Ownership model (no UAF, no leaks in the normal paths, thread-bound):
//! - State is `Rc<ChromeShared>`. The guard holds one clone; the window
//!   property `AquaChromeState.v1` (`SetPropW`) holds the registration
//!   clone as a raw pointer. The callback NEVER frees: it only borrows via
//!   `GetPropW`, and reclaims (`Rc::from_raw`) exactly once when IT removes
//!   the property — on `WM_NCDESTROY`, or when teardown removes it while no
//!   other subclass can still call us.
//! - Later subclasses (AccessKit, winit internals) chaining ABOVE us keep
//!   working: our proc address stays valid code forever, and with the prop
//!   present our state stays alive. Guard `Drop` soft-disables first, then
//!   restores the previous proc ONLY if ours is currently on top — never
//!   unchaining someone else. If another subclass is above, the prop and
//!   registration stay until `WM_NCDESTROY` reclaims them; the guard's own
//!   `Rc` simply drops.
//! - `WM_NCDESTROY`: mark destroyed, remove prop + reclaim registration
//!   ref, then chain to prev. After that the `HWND` value may be recycled —
//!   no OS calls use it anymore (diagnostics skip HWND queries once
//!   destroyed).
//! - Duplicate installs are rejected (`AlreadyInstalled`) when the property
//!   exists — property check, not proc-address comparison, so a different
//!   proc topping the chain can't fool it. A stale prop on a recycled HWND
//!   (only possible if a misbehaving outer chain swallowed `WM_NCDESTROY`)
//!   fails closed, never unsound.
//! - No borrow is held across OS calls (snapshot `Cell`s, clone drag rects
//!   out of a short borrow first). The proc body runs under `catch_unwind`.
//! - Guard is `!Send + !Sync`: everything runs on the owning GUI thread.
//!
//! Install is all-or-nothing: any step failing rolls back (previous proc
//! restored, prop removed + reclaimed, glass reset, frame re-run) and
//! returns `Err` — a guard is only ever returned fully active.

use std::cell::{Cell, RefCell};
use std::marker::PhantomData;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};

use raw_window_handle::{HasWindowHandle, RawWindowHandle};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{HANDLE, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmDefWindowProc, DwmExtendFrameIntoClientArea, DwmGetWindowAttribute,
    DWMWA_CAPTION_BUTTON_BOUNDS,
};
use windows::Win32::Graphics::Gdi::{
    ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::HiDpi::{GetDpiForWindow, GetSystemMetricsForDpi};
use windows::Win32::UI::WindowsAndMessaging::{
    CallWindowProcW, DefWindowProcW, GetPropW, GetSystemMenu, GetSystemMetrics, GetWindowLongPtrW,
    GetWindowRect, GetWindowThreadProcessId, IsZoomed, RemovePropW, SetPropW, SetWindowLongPtrW,
    SetWindowPos, TrackPopupMenu, GWLP_WNDPROC, MINMAXINFO, NCCALCSIZE_PARAMS, SM_CXFRAME,
    SM_CXPADDEDBORDER, SWP_FRAMECHANGED, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SYSTEM_METRICS_INDEX, TPM_LEFTBUTTON, TPM_RIGHTBUTTON, TRACK_POPUP_MENU_FLAGS, WM_ACTIVATE,
    WM_DPICHANGED, WM_GETMINMAXINFO, WM_NCCALCSIZE, WM_NCDESTROY, WM_NCHITTEST, WNDPROC,
};

use crate::chrome::{
    hit_test_chrome, ChromeError, ChromeStats, DragRects, DragRegions, HitInput, RectI,
};
use crate::window::{Titlebar, WindowOptions};

/// Window property holding the registration `Rc` (raw). Unique name so a
/// duplicate install is detected even when another proc tops the chain.
/// "AquaChromeState\0" as UTF-16.
static PROP_NAME: &[u16] = &[
    65, 113, 117, 97, 67, 104, 114, 111, 109, 101, 83, 116, 97, 116, 101, 0,
];

fn prop_name() -> PCWSTR {
    PCWSTR(PROP_NAME.as_ptr())
}

/// `Cell`-based counters: no borrow to hold across re-entrant OS calls.
#[derive(Debug, Default)]
struct StatCells {
    nc_calc_size: Cell<u64>,
    nc_hit_test: Cell<u64>,
    dwm_handled: Cell<u64>,
    fallback_handled: Cell<u64>,
    system_menus_shown: Cell<u64>,
}

impl StatCells {
    fn snapshot(&self) -> ChromeStats {
        ChromeStats {
            nc_calc_size: self.nc_calc_size.get(),
            nc_hit_test: self.nc_hit_test.get(),
            dwm_handled: self.dwm_handled.get(),
            fallback_handled: self.fallback_handled.get(),
            system_menus_shown: self.system_menus_shown.get(),
        }
    }
}

struct ChromeShared {
    /// Previous proc captured at install; chained via `CallWindowProcW`.
    /// Zero only before install completes (never observed by the proc).
    prev_proc: Cell<isize>,
    titlebar_height: Cell<f64>,
    drag_band_px: Cell<u32>,
    drag: RefCell<DragRects>,
    resizable: bool,
    min_size: Option<(f64, f64)>,
    max_size: Option<(f64, f64)>,
    extend_top_px: Cell<i32>,
    dpi: Cell<u32>,
    stats: StatCells,
    destroyed: AtomicBool,
    /// Overlay logically off (soft-disabled + will not re-register).
    detached: Cell<bool>,
}

fn our_proc_addr() -> isize {
    chrome_wndproc as *const () as usize as isize
}

fn is_ours_on_top(hwnd: HWND) -> bool {
    unsafe { GetWindowLongPtrW(hwnd, GWLP_WNDPROC) == our_proc_addr() }
}

/// Chain to a previous proc; `0` (unknown) falls back to `DefWindowProcW`.
/// Never panics, never unwinds.
fn chain_to(prev: isize, hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    unsafe {
        if prev == 0 {
            DefWindowProcW(hwnd, msg, wparam, lparam)
        } else {
            let proc: WNDPROC = std::mem::transmute(prev);
            CallWindowProcW(proc, hwnd, msg, wparam, lparam)
        }
    }
}

/// Reclaim the registration `Rc` stored in the window property. Must be
/// called exactly once per install, and only when no callback can still
/// run through this registration (our proc already unchained or the window
/// dying via `WM_NCDESTROY`). Returns true when a registration existed.
fn reclaim_registration(hwnd: HWND) -> bool {
    unsafe {
        match RemovePropW(hwnd, prop_name()) {
            Ok(old) if !old.0.is_null() => {
                // SOLE reclaim site pattern: the raw pointer came from
                // `Rc::into_raw` at install; exactly one `from_raw` per install.
                drop(Rc::from_raw(old.0 as *const ChromeShared));
                true
            }
            _ => false,
        }
    }
}

/// Live overlay guard. Thread-bound (`!Send + !Sync` via `Rc` + explicit
/// marker); drop detaches the overlay AND restores the OS frame when safe.
/// Must be held until `WM_NCDESTROY` (i.e. for the window's whole life).
pub struct WindowsChrome {
    hwnd: HWND,
    /// Guard-owned reference. The registration reference lives in the
    /// window property until reclaimed by teardown/`WM_NCDESTROY`.
    /// `None` for detached (`Native`/`Hidden`) guards.
    state: Option<Rc<ChromeShared>>,
    _thread: PhantomData<*mut ()>,
}

impl WindowsChrome {
    /// Install the overlay on a live `winit` window per `opts`.
    ///
    /// Requirements: the window must already exist (created, not yet
    /// destroyed) on the calling GUI thread. For `Titlebar::Native` this
    /// validates options and returns a detached guard (no chain).
    /// `Titlebar::Hidden` (or `decorations(false)`) needs no chain either.
    pub fn install(
        window: &winit::window::Window,
        opts: &WindowOptions,
    ) -> Result<Self, ChromeError> {
        opts.validated()
            .map_err(|_| ChromeError::InvalidOptions("WindowOptions failed validation"))?;
        let handle = window
            .window_handle()
            .map_err(|_| ChromeError::NullHandle)?;
        let hwnd = match handle.as_raw() {
            RawWindowHandle::Win32(h) => HWND(h.hwnd.get() as *mut _),
            _ => return Err(ChromeError::NullHandle),
        };
        if hwnd.0.is_null() {
            return Err(ChromeError::NullHandle);
        }
        // GUI-thread enforcement: Rc/Cell state is created here and used by
        // callbacks on the window's owning thread. Refuse cross-thread use
        // rather than shipping thread-unsafe sharing.
        if !owns_window_thread(hwnd) {
            return Err(ChromeError::WrongThread);
        }
        match opts.effective_titlebar() {
            Titlebar::Native | Titlebar::Hidden => Ok(Self {
                hwnd,
                state: None,
                _thread: PhantomData,
            }),
            Titlebar::Overlay => Self::subclass(hwnd, window, opts),
        }
    }

    fn subclass(
        hwnd: HWND,
        window: &winit::window::Window,
        opts: &WindowOptions,
    ) -> Result<Self, ChromeError> {
        // Duplicate installs rejected by PROPERTY (not proc comparison): a
        // different proc topping the chain can't hide a live registration,
        // and a stale prop on a recycled HWND fails closed, never unsound.
        unsafe {
            if !GetPropW(hwnd, prop_name()).0.is_null() {
                return Err(ChromeError::AlreadyInstalled);
            }
        }
        let scale = window.scale_factor().max(1.0);
        let drag_band_px = (opts.titlebar_height.max(0.0) * scale).round().max(0.0) as u32;
        let state = Rc::new(ChromeShared {
            prev_proc: Cell::new(0),
            titlebar_height: Cell::new(opts.titlebar_height.max(0.0)),
            drag_band_px: Cell::new(drag_band_px),
            drag: RefCell::new(DragRects::default()),
            resizable: opts.resizable,
            min_size: opts.min_size.map(|size| (size.width, size.height)),
            max_size: opts.max_size.map(|size| (size.width, size.height)),
            extend_top_px: Cell::new(drag_band_px as i32),
            dpi: Cell::new(unsafe { GetDpiForWindow(hwnd) }),
            stats: StatCells::default(),
            destroyed: AtomicBool::new(false),
            detached: Cell::new(false),
        });
        unsafe {
            // Publish state before changing the procedure: a failed
            // property insertion must leave winit's procedure intact.
            let prev = GetWindowLongPtrW(hwnd, GWLP_WNDPROC);
            if prev == 0 {
                return Err(ChromeError::SubclassFailed);
            }
            state.prev_proc.set(prev);
            // Registration reference: raw pointer into the window property.
            let reg: *const ChromeShared = Rc::into_raw(Rc::clone(&state));
            if SetPropW(hwnd, prop_name(), Some(HANDLE(reg as *mut _))).is_err() {
                drop(Rc::from_raw(reg));
                return Err(ChromeError::SubclassFailed);
            }
            let prev = SetWindowLongPtrW(hwnd, GWLP_WNDPROC, our_proc_addr());
            if prev == 0 {
                reclaim_registration(hwnd);
                return Err(ChromeError::SubclassFailed);
            }
            state.prev_proc.set(prev);
            // All-or-nothing: frame apply + glass must succeed, else roll
            // back to a real OS frame and report — never half-installed.
            let framed = SetWindowPos(
                hwnd,
                None,
                0,
                0,
                (opts.inner_size.width * scale).round().max(1.0) as i32,
                (opts.inner_size.height * scale).round().max(1.0) as i32,
                // The overlay has no non-client inset when restored.
                // winit's ordinary inner-size request adds WS_CAPTION
                // margins, so apply the requested client size directly.
                SWP_NOMOVE | SWP_NOZORDER | SWP_FRAMECHANGED,
            )
            .is_ok();
            let glass = extend_top_margin(hwnd, drag_band_px as i32);
            if !framed || !glass {
                state.detached.set(true);
                rollback_install(hwnd, prev);
                return Err(ChromeError::SubclassFailed);
            }
        }
        Ok(Self {
            hwnd,
            state: Some(state),
            _thread: PhantomData,
        })
    }

    fn attached(&self) -> Result<&ChromeShared, ChromeError> {
        self.state.as_deref().ok_or(ChromeError::Detached)
    }

    /// Live registration check: owned state, window not destroyed, teardown
    /// hasn't detached us. Mutating calls reject detached guards instead of
    /// silently writing nowhere.
    fn live(&self) -> Result<&ChromeShared, ChromeError> {
        let st = self.attached()?;
        if st.destroyed.load(Ordering::SeqCst) || st.detached.get() {
            return Err(ChromeError::Detached);
        }
        Ok(st)
    }

    /// Update draggable / non-draggable regions (logical px, client-local).
    /// Converted with `scale` (the window's current scale factor).
    pub fn set_drag_rects(
        &self,
        draggable: &[(f64, f64, f64, f64)],
        no_drag: &[(f64, f64, f64, f64)],
        scale: f64,
    ) -> Result<(), ChromeError> {
        let st = self.live()?;
        let s = if scale.is_finite() && scale > 0.0 {
            scale
        } else {
            1.0
        };
        let conv = |r: &(f64, f64, f64, f64)| RectI {
            left: (r.0 * s).round() as i32,
            top: (r.1 * s).round() as i32,
            right: (r.2 * s).round() as i32,
            bottom: (r.3 * s).round() as i32,
        };
        // Short borrow, released before return: no OS calls under it.
        *st.drag.borrow_mut() = DragRects {
            draggable: draggable.iter().map(conv).collect(),
            no_drag: no_drag.iter().map(conv).collect(),
        };
        Ok(())
    }

    /// Canonical [`DragRegions`] (logical px) entry point for the runtime
    /// hook: converts with `scale` and applies via [`Self::set_drag_rects`].
    /// Same signature on the stub guard.
    pub fn set_drag_regions(&self, regions: &DragRegions, scale: f64) -> Result<(), ChromeError> {
        self.set_drag_rects(&regions.draggable, &regions.no_drag, scale)
    }

    /// Show the native window system menu at a screen point. Selection
    /// dispatches `WM_SYSCOMMAND` to the window itself.
    pub fn show_system_menu(&self, x_screen: i32, y_screen: i32) -> Result<(), ChromeError> {
        self.live()?;
        unsafe {
            let menu = GetSystemMenu(self.hwnd, false);
            if menu.0.is_null() {
                return Err(ChromeError::Detached);
            }
            let _ = TrackPopupMenu(
                menu,
                TRACK_POPUP_MENU_FLAGS(TPM_LEFTBUTTON.0 | TPM_RIGHTBUTTON.0),
                x_screen,
                y_screen,
                None,
                self.hwnd,
                None,
            );
        }
        if let Ok(st) = self.attached() {
            st.stats
                .system_menus_shown
                .set(st.stats.system_menus_shown.get() + 1);
        }
        Ok(())
    }

    /// Live diagnostics proving installation: chain state, DPI, button
    /// bounds, maximized state, message counters. Makes NO HWND calls once
    /// destroyed (the value may be recycled).
    pub fn diagnostics(&self) -> ChromeDiagnostic {
        let destroyed = self
            .state
            .as_ref()
            .map(|st| st.destroyed.load(Ordering::SeqCst))
            .unwrap_or(true);
        if destroyed || self.hwnd.0.is_null() {
            return ChromeDiagnostic {
                subclass_active: false,
                hwnd: self.hwnd.0 as usize as u64,
                dpi: 0,
                button_bounds: None,
                maximized: false,
                stats: self
                    .attached()
                    .map(|st| st.stats.snapshot())
                    .unwrap_or_default(),
            };
        }
        unsafe {
            let dpi = GetDpiForWindow(self.hwnd);
            let buttons = caption_button_bounds(self.hwnd);
            let maximized = IsZoomed(self.hwnd).as_bool();
            let stats = self
                .attached()
                .map(|st| st.stats.snapshot())
                .unwrap_or_default();
            let live = self
                .state
                .as_ref()
                .map(|st| !st.detached.get())
                .unwrap_or(false);
            ChromeDiagnostic {
                subclass_active: live,
                hwnd: self.hwnd.0 as usize as u64,
                dpi,
                button_bounds: buttons,
                maximized,
                stats,
            }
        }
    }

    /// One-line install proof for logs (`eprintln!` at attach time).
    pub fn diagnostic_string(&self) -> String {
        let d = self.diagnostics();
        format!(
            "aqua-chrome hwnd=0x{:x} subclass={} dpi={} buttons={:?} maximized={} nccalc={} nchittest={} dwm={} fallback={}",
            d.hwnd,
            d.subclass_active,
            d.dpi,
            d.button_bounds,
            d.maximized,
            d.stats.nc_calc_size,
            d.stats.nc_hit_test,
            d.stats.dwm_handled,
            d.stats.fallback_handled,
        )
    }

    /// Restore the OS frame without dropping the guard: soft-disable, detach
    /// the chain ONLY if our proc is on top (never unchaining someone
    /// else's subclass), reset glass margins, force `WM_NCCALCSIZE`.
    /// Best-effort; teardown never panics. Afterwards the guard reports
    /// detached but keeps owning state (already-dispatched callbacks still
    /// borrow valid memory; a still-chained proc above us keeps working via
    /// the registration reference until `WM_NCDESTROY`).
    ///
    /// No-op once `WM_NCDESTROY` ran: the window is gone and its `HWND`
    /// value may already be recycled — no OS calls are made then.
    pub fn restore_frame(&mut self) {
        let st = match self.state.as_ref() {
            Some(st) => st,
            None => return,
        };
        if st.destroyed.load(Ordering::SeqCst) {
            return;
        }
        st.detached.set(true);
        unsafe {
            if is_ours_on_top(self.hwnd) {
                let prev = st.prev_proc.get();
                // Checked restore: reclaim the property ref ONLY when the
                // chain actually detached. On failure the ref stays alive
                // (leak, never UAF) for WM_NCDESTROY to reclaim.
                if SetWindowLongPtrW(self.hwnd, GWLP_WNDPROC, prev) != 0 {
                    // Prop removal is safe here: nobody can be above us (we
                    // just verified we're on top on this thread).
                    reclaim_registration(self.hwnd);
                }
            }
            extend_top_margin(self.hwnd, 0);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
    }
}

impl Drop for WindowsChrome {
    fn drop(&mut self) {
        // Guard-owned Rc drops at the end of this function in every path.
        // Registration Rc: reclaimed here ONLY when no callback can still
        // run through it (our proc already off the chain, or the window
        // dying). Otherwise it stays until WM_NCDESTROY reclaims it —
        // a bounded leak is impossible: NCDESTROY always runs for a
        // destroyed window whose chain forwards it.
        let Some(st) = self.state.take() else {
            return;
        };
        if st.destroyed.load(Ordering::SeqCst) {
            // Window gone (HWND possibly recycled): no OS calls at all.
            drop(st);
            return;
        }
        st.detached.set(true);
        unsafe {
            if is_ours_on_top(self.hwnd) {
                let prev = st.prev_proc.get();
                // Checked restore (see restore_frame): reclaim only on
                // success, else keep the ref for WM_NCDESTROY.
                if SetWindowLongPtrW(self.hwnd, GWLP_WNDPROC, prev) != 0 {
                    reclaim_registration(self.hwnd);
                }
            }
            // Someone above us keeps chaining to our proc address: valid
            // code + live registration ref, soft-disabled. Their teardown
            // or WM_NCDESTROY finishes the job.
            extend_top_margin(self.hwnd, 0);
            let _ = SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                0,
                0,
                SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
            );
        }
        drop(st);
    }
}

/// Install rollback: previous proc restored, prop removed + registration
/// reclaimed, glass reset, frame re-run — then `Err`. Runs only on the
/// installing thread immediately after install, so no other subclass can
/// exist above us yet and reclaiming is safe. The restore itself is
/// checked: on failure the property ref stays for `WM_NCDESTROY`.
fn rollback_install(hwnd: HWND, prev: isize) {
    unsafe {
        if SetWindowLongPtrW(hwnd, GWLP_WNDPROC, prev) != 0 {
            reclaim_registration(hwnd);
        }
        extend_top_margin(hwnd, 0);
        let _ = SetWindowPos(
            hwnd,
            None,
            0,
            0,
            0,
            0,
            SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_FRAMECHANGED,
        );
    }
}

/// Diagnostic snapshot for logs and the external PowerShell harness.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChromeDiagnostic {
    pub subclass_active: bool,
    pub hwnd: u64,
    pub dpi: u32,
    pub button_bounds: Option<RectI>,
    pub maximized: bool,
    pub stats: ChromeStats,
}

/// DWM caption-button bounds in SCREEN space. The OS reports them
/// window-relative (`DWMWA_CAPTION_BUTTON_BOUNDS`); callers must add the
/// window origin — done here so [`HitInput`] always sees screen coords.
fn caption_button_bounds(hwnd: HWND) -> Option<RectI> {
    if hwnd.0.is_null() {
        return None;
    }
    let mut rect = RECT::default();
    let ok = unsafe {
        DwmGetWindowAttribute(
            hwnd,
            DWMWA_CAPTION_BUTTON_BOUNDS,
            &mut rect as *mut RECT as *mut _,
            std::mem::size_of::<RECT>() as u32,
        )
        .is_ok()
    };
    if !ok || (rect.right <= rect.left) {
        return None;
    }
    let mut win = RECT::default();
    unsafe {
        let _ = GetWindowRect(hwnd, &mut win);
    }
    Some(RectI {
        left: rect.left + win.left,
        top: rect.top + win.top,
        right: rect.right + win.left,
        bottom: rect.bottom + win.top,
    })
}

fn frame_padding_px(_hwnd: HWND, dpi: u32) -> i32 {
    // DPI-aware frame metrics; plain fallback when DPI is unavailable.
    let metric = |index: SYSTEM_METRICS_INDEX| -> i32 {
        if dpi == 0 {
            unsafe { GetSystemMetrics(index) }
        } else {
            unsafe { GetSystemMetricsForDpi(index, dpi) }
        }
    };
    metric(SM_CXFRAME) + metric(SM_CXPADDEDBORDER)
}

/// Monitor work area (taskbar side respected, all four edges incl. top).
/// `None` when the monitor cannot be queried — caller keeps default handling.
fn monitor_work_area(hwnd: HWND) -> Option<RectI> {
    unsafe {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        if monitor.0.is_null() {
            return None;
        }
        let mut info = MONITORINFO::default();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info).as_bool() {
            let w = info.rcWork;
            Some(RectI {
                left: w.left,
                top: w.top,
                right: w.right,
                bottom: w.bottom,
            })
        } else {
            None
        }
    }
}

fn extend_top_margin(hwnd: HWND, top_px: i32) -> bool {
    let margins = MARGINS {
        cxLeftWidth: 0,
        cxRightWidth: 0,
        cyBottomHeight: 0,
        cyTopHeight: top_px.max(0),
    };
    unsafe { DwmExtendFrameIntoClientArea(hwnd, &margins).is_ok() }
}

/// Borrow the registration state for one message, holding a REAL owned
/// reference for the whole dispatch: bumps the strong count from the raw
/// property pointer and wraps it back. The returned `Rc` keeps state alive
/// even if `WM_NCDESTROY` reclaims the property ref, or the guard drops,
/// mid-dispatch. GUI thread only (all owner threads are the window's).
/// Returns `None` when no registration exists (pre-prop window, post-teardown
/// stray call): the caller chains to `DefWindowProcW` with no state access.
fn registration_state(hwnd: HWND) -> Option<Rc<ChromeShared>> {
    unsafe {
        let raw = GetPropW(hwnd, prop_name());
        if raw.0.is_null() {
            return None;
        }
        // SAFETY: non-null raw came from `Rc::into_raw` at install; the
        // property ref it represents is alive until `reclaim_registration`
        // runs, which can only run on this same thread — and even then,
        // this increment keeps OUR reference valid regardless.
        Rc::increment_strong_count(raw.0 as *const ChromeShared);
        Some(Rc::from_raw(raw.0 as *const ChromeShared))
    }
}

/// Enforce the GUI-thread contract: public install/teardown entry points
/// must run on the window's owning thread, otherwise the `Rc`/`Cell` state
/// could be created on a different thread than the callbacks use.
fn owns_window_thread(hwnd: HWND) -> bool {
    unsafe { GetWindowThreadProcessId(hwnd, None) == GetCurrentThreadId() }
}

/// The chained window procedure. NEVER unwinds: the body runs under
/// `catch_unwind`, panics degrade to chaining the previous proc.
unsafe extern "system" fn chrome_wndproc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        chrome_message(hwnd, msg, wparam, lparam)
    }));
    match result {
        Ok(outcome) => outcome,
        Err(_) => {
            // Panic across FFI is UB: degrade to the previous proc (or DWM
            // default when unknown). State untouched.
            let prev = registration_state(hwnd)
                .map(|st| st.prev_proc.get())
                .unwrap_or(0);
            chain_to(prev, hwnd, msg, wparam, lparam)
        }
    }
}

/// Message handling. Holds NO borrow across any OS call: scalars are
/// snapshotted out of `Cell`s and drag rects cloned out of a short borrow
/// before the first syscall below.
fn chrome_message(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // (0) DWM FIRST for every message (MS recipe). Handled DWM hits
    // (caption buttons incl. hover/snap, NCMOUSELEAVE) return here untouched.
    let mut dwm_result = LRESULT(0);
    let dwm_handled =
        unsafe { DwmDefWindowProc(hwnd, msg, wparam, lparam, &mut dwm_result).as_bool() };
    let state = registration_state(hwnd);
    // Previous proc snapshot (for chaining); unknown (0) only when no
    // registration exists — then DWM-default is the only sane chain.
    let prev = state.as_ref().map(|st| st.prev_proc.get()).unwrap_or(0);
    let chain = |m: u32, w: WPARAM, l: LPARAM| chain_to(prev, hwnd, m, w, l);

    if msg == WM_NCDESTROY {
        // Mark only here; reclaim the registration ref NOW (window dying —
        // this is the final reclaim site) and chain so comctl/winit tear
        // down. Guard-owned Rc (if the guard still lives) keeps state valid
        // for any stray post-destroy dispatch; diagnostics skip HWND use.
        if let Some(st) = state {
            st.destroyed.store(true, Ordering::SeqCst);
            reclaim_registration(hwnd);
        }
        return chain(msg, wparam, lparam);
    }

    let Some(st) = state else {
        // No registration (e.g. teardown raced a stray dispatch, or a
        // foreign chain calls our address directly): DWM default, no state.
        if dwm_handled {
            return dwm_result;
        }
        return chain(msg, wparam, lparam);
    };

    if dwm_handled {
        if msg == WM_NCHITTEST {
            let n = st.stats.dwm_handled.get();
            st.stats.dwm_handled.set(n + 1);
        }
        return dwm_result;
    }

    // Soft-disabled (guard dropped/restored while chained below another
    // subclass): pure forward, no handling, no stats.
    if st.detached.get() {
        return chain(msg, wparam, lparam);
    }

    match msg {
        WM_NCCALCSIZE => {
            if wparam.0 != 0 {
                // Full client; maximized windows clamp to the monitor work
                // area (all edges incl. top; taskbar side respected).
                // Residual gap: auto-hide taskbars report a full-monitor
                // work area, so a few px may sit under an auto-hidden bar.
                let params = unsafe { &mut *(lparam.0 as *mut NCCALCSIZE_PARAMS) };
                let maximized = unsafe { IsZoomed(hwnd).as_bool() };
                if maximized {
                    if let Some(work) = monitor_work_area(hwnd) {
                        let r = &mut params.rgrc[0];
                        r.left = r.left.max(work.left);
                        r.top = r.top.max(work.top);
                        r.right = r.right.min(work.right);
                        r.bottom = r.bottom.min(work.bottom);
                    } else {
                        // Monitor query failed: DPI-aware frame padding
                        // fallback so content never bleeds under chrome.
                        let pad = frame_padding_px(hwnd, st.dpi.get());
                        let r = &mut params.rgrc[0];
                        r.left += pad;
                        r.top += pad;
                        r.right -= pad;
                        r.bottom -= pad;
                    }
                }
                let n = st.stats.nc_calc_size.get();
                st.stats.nc_calc_size.set(n + 1);
                return LRESULT(0);
            }
            chain(msg, wparam, lparam)
        }
        WM_GETMINMAXINFO => {
            // Let winit fill the usual monitor/default constraints, then
            // replace authored limits: its WS_CAPTION adjustment would
            // add an inset that this full-client frame does not have.
            let result = chain(msg, wparam, lparam);
            let scale = f64::from(unsafe { GetDpiForWindow(hwnd) }.max(96)) / 96.0;
            if lparam.0 != 0 {
                let limits = unsafe { &mut *(lparam.0 as *mut MINMAXINFO) };
                let point = |(width, height): (f64, f64)| POINT {
                    x: (width * scale).round().max(1.0) as i32,
                    y: (height * scale).round().max(1.0) as i32,
                };
                if let Some(size) = st.min_size {
                    limits.ptMinTrackSize = point(size);
                }
                if let Some(size) = st.max_size {
                    limits.ptMaxTrackSize = point(size);
                }
            }
            result
        }
        WM_NCHITTEST => {
            let x = ((lparam.0 as u32) & 0xFFFF) as u16 as i16 as i32;
            let y = (((lparam.0 as u32) >> 16) & 0xFFFF) as u16 as i16 as i32;
            // Scalar snapshot first (Cells only — no borrow held).
            let drag_band = st.drag_band_px.get();
            let maximized_now = unsafe { IsZoomed(hwnd).as_bool() };
            let resizable_now = st.resizable;
            // OS-query phase: window rect, CLIENT origin, button strip, DPI.
            // No state borrow is held across any of these calls. The client
            // origin (not the window origin) anchors drag rects: maximize
            // padding shifts the client area inside the outer frame.
            let mut rect = RECT::default();
            unsafe {
                let _ = GetWindowRect(hwnd, &mut rect);
            }
            let mut origin = POINT { x: 0, y: 0 };
            unsafe {
                let _ = ClientToScreen(hwnd, &mut origin);
            }
            // DWM declined: fall back to the shared pure hit tester with
            // live button bounds (attr 5, converted to screen space) so
            // disabled/absent-DWM cases still behave.
            let buttons = caption_button_bounds(hwnd);
            let dpi_now = unsafe { GetDpiForWindow(hwnd) };
            let border = frame_padding_px(hwnd, dpi_now).max(0) as u32;
            // Pure phase: short RefCell borrow ONLY around hit_test_chrome
            // (no OS calls inside), released before stats/forwarding.
            let code = {
                let drag = st.drag.borrow();
                hit_test_chrome(&HitInput {
                    point: (x, y),
                    window: RectI {
                        left: rect.left,
                        top: rect.top,
                        right: rect.right,
                        bottom: rect.bottom,
                    },
                    client_origin: (origin.x, origin.y),
                    border,
                    buttons,
                    drag_band,
                    drag: &drag,
                    maximized: maximized_now,
                    resizable: resizable_now,
                })
            };
            let n = st.stats.nc_hit_test.get();
            st.stats.nc_hit_test.set(n + 1);
            let f = st.stats.fallback_handled.get();
            st.stats.fallback_handled.set(f + 1);
            LRESULT(code as isize)
        }
        WM_DPICHANGED => {
            // Refresh band + glass margin for the new DPI. wParam hiword =
            // Y-axis DPI. Chain afterwards (winit repositions from lParam).
            let new_dpi = (((wparam.0 as u32) >> 16) & 0xFFFF) as u32;
            if new_dpi != 0 {
                st.dpi.set(new_dpi);
                let band = (st.titlebar_height.get() * f64::from(new_dpi) / 96.0)
                    .round()
                    .max(0.0) as u32;
                st.drag_band_px.set(band);
                st.extend_top_px.set(band as i32);
            }
            extend_top_margin(hwnd, st.extend_top_px.get());
            chain(msg, wparam, lparam)
        }
        WM_ACTIVATE => {
            // Re-extend the top glass margin on activation changes
            // (MS recipe extends the frame here; keeps buttons composited).
            extend_top_margin(hwnd, st.extend_top_px.get());
            chain(msg, wparam, lparam)
        }
        _ => chain(msg, wparam, lparam),
    }
}
