//! Custom window chrome: native caption buttons over glass (Windows overlay).
//!
//! Configuration and behavior (see `docs/WINDOW_CHROME.md`):
//! - [`crate::window::Titlebar`]: `Native` (default), `Overlay`, `Hidden`.
//! - [`crate::window::WindowOptions`] carries `titlebar`,
//!   `titlebar_height`, `minimizable`, `maximizable`, `closable` (defaults:
//!   Native, 40 logical px, all buttons on).
//! - `host::window_attributes(&WindowOptions)` maps the seam to winit
//!   (overlay keeps `decorations(true)` — `WS_CAPTION` is what keeps DWM
//!   drawing the buttons; `Hidden` maps to borderless).
//! - [`WindowsChrome::install`] attaches the
//!   overlay (real HWND subclass on Windows in `chrome_winit`; portable
//!   stub here returns explicit `Unsupported` for `Overlay` on other
//!   targets so the runtime falls back to `Native`).
//! - Pure CPU hit-testing ([`hit_test_chrome`]) is shared by the live
//!   subclass proc and headless regression tests. No UI dependency.

#[cfg(not(windows))]
use crate::window::{Titlebar, WindowOptions};

/// Real overlay guard on Windows, re-exported here so
/// `chrome::WindowsChrome` resolves to the live implementation under
/// `cfg(windows)` and to the portable stub elsewhere. Runtime code paths
/// use this one path on every target.
#[cfg(windows)]
pub use crate::chrome_winit::WindowsChrome;

/// Chrome failures. All are honest runtime results, never panics.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeError {
    /// `Overlay` on a platform without the native path (caller falls back
    /// to `Native`).
    Unsupported(&'static str),
    NullHandle,
    SubclassFailed,
    InvalidOptions(&'static str),
    /// A live overlay registration already exists on this HWND. The old
    /// guard must be dropped first: replacing it would let the old guard's
    /// `Drop` remove the NEW registration.
    AlreadyInstalled,
    /// Install/teardown called off the window's owning (GUI) thread. The
    /// `Rc`/`Cell` state and all callbacks live on that thread only.
    WrongThread,
    Detached,
}

impl std::fmt::Display for ChromeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChromeError::Unsupported(r) => write!(f, "window overlay unsupported: {r}"),
            ChromeError::NullHandle => write!(f, "null window handle"),
            ChromeError::SubclassFailed => write!(f, "window procedure installation failed"),
            ChromeError::InvalidOptions(r) => write!(f, "invalid chrome options: {r}"),
            ChromeError::AlreadyInstalled => {
                write!(f, "overlay already installed on this window")
            }
            ChromeError::WrongThread => {
                write!(f, "chrome must be installed on the window's GUI thread")
            }
            ChromeError::Detached => write!(f, "chrome guard detached"),
        }
    }
}

/// Integer rect in physical pixels. [`DragRects`] are client-local (origin
/// at the client area's top-left; stable across moves); the DWM button strip in
/// [`HitInput`] is screen space.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectI {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }
}

/// Canonical portable drag regions in LOGICAL px for the runtime's
/// `Application::chrome_regions` hook: the app publishes these after
/// layout/scene with the current scale, and the guard converts them.
/// Tuples are `(left, top, right, bottom)`, client-local. No reactive graph
/// involvement — plain data, applied imperatively via
/// [`WindowsChrome::set_drag_regions`] (stub and live guard share the
/// `set_drag_rects` tuple signature underneath).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragRegions {
    /// Extra `HTCAPTION` regions beyond the drag band.
    pub draggable: Vec<(f64, f64, f64, f64)>,
    /// `HTCLIENT` holes inside caption regions.
    pub no_drag: Vec<(f64, f64, f64, f64)>,
}

/// Draggable / non-draggable rectangles in CLIENT-LOCAL physical pixels
/// (origin at the client area's top-left, i.e. layout coordinates scaled).
/// The app publishes layout rects; the guard converts logical→physical with
/// the current scale. Client space (not outer-window space) because a
/// maximized client origin sits inside the outer frame by the frame padding
/// — the hit tester offsets the cursor by the live client origin
/// (`ClientToScreen(0,0)`) every hit, so regions track maximize/restore/DPI.
#[derive(Debug, Clone, Default)]
pub struct DragRects {
    /// Extra `HTCAPTION` regions beyond the drag band (e.g. toolbar gaps).
    pub draggable: Vec<RectI>,
    /// `HTCLIENT` holes inside caption regions (buttons, text fields).
    pub no_drag: Vec<RectI>,
}

/// Input to the pure hit tester. All coordinates physical pixels.
#[derive(Debug, Clone)]
pub struct HitInput<'a> {
    /// Cursor in screen space (from `WM_NCHITTEST` lParam).
    pub point: (i32, i32),
    /// Window rect in screen space (`GetWindowRect`).
    pub window: RectI,
    /// Client origin in screen space (`ClientToScreen(0,0)`). Drag rects
    /// are client-local; the tester subtracts this (not the window origin),
    /// so regions stay correct when maximize padding shifts the client area.
    pub client_origin: (i32, i32),
    /// Resize border thickness in px (DPI-aware frame metrics; 0 disables).
    pub border: u32,
    /// DWM caption-button strip in screen space
    /// (`DWMWA_CAPTION_BUTTON_BOUNDS`), if the OS reports one. Reserved:
    /// points inside resolve to `HTCLIENT`, never to guessed button cells
    /// (only `DwmDefWindowProc` may authoritatively return 8/9/20).
    pub buttons: Option<RectI>,
    /// Drag-band height in px (converted `titlebar_height`).
    pub drag_band: u32,
    pub drag: &'a DragRects,
    /// `IsZoomed` state: no resize frame, keeps caption behavior.
    pub maximized: bool,
    pub resizable: bool,
}

/// `HT*` codes returned by [`hit_test_chrome`] (same values as Win32).
pub mod ht {
    pub const NOWHERE: i32 = 0;
    pub const CLIENT: i32 = 1;
    pub const CAPTION: i32 = 2;
    pub const MINBUTTON: i32 = 8;
    pub const MAXBUTTON: i32 = 9;
    pub const LEFT: i32 = 10;
    pub const RIGHT: i32 = 11;
    pub const TOP: i32 = 12;
    pub const TOPLEFT: i32 = 13;
    pub const TOPRIGHT: i32 = 14;
    pub const BOTTOM: i32 = 15;
    pub const BOTTOMLEFT: i32 = 16;
    pub const BOTTOMRIGHT: i32 = 17;
    pub const CLOSE: i32 = 20;
}

/// Pure CPU hit test shared by the live subclass proc and regression tests.
///
/// Order (matches the MS `HitTestNCA` grid, extended for the overlay):
/// 1. Resize borders/corners first (skipped when maximized or unresizable).
/// 2. Reported DWM button strip -> `HTCLIENT` (reserved, non-draggable).
///    Only `DwmDefWindowProc` authoritatively returns 8/9/20; when it
///    declines, the strip must NOT be subdivided into guessed cells
///    (disabled buttons, DPI/theme widths, and RTL make equal thirds
///    unreliable) and must NOT become `HTCAPTION` either — dragging from a
///    disabled button cell would be wrong. Reserved CLIENT keeps input
///    with the app without dispatching guesses.
/// 3. Drag band + explicit `draggable` rects -> `HTCAPTION`, except
///    `no_drag` holes -> `HTCLIENT`.
/// 4. Everything else -> `HTCLIENT` (never `HTNOWHERE`: swallowing input
///    breaks the app; unknown regions stay client).
///
/// Native behavior (hover, snap layouts, double-click maximize, dragging)
/// comes from `HT*` codes processed by `DefWindowProc`/`DwmDefWindowProc` —
/// the live proc calls DWM first, and this fallback only covers
/// DWM-declined regions. Async AT clicks likewise arrive as action
/// requests, never mouse input.
pub fn hit_test_chrome(input: &HitInput<'_>) -> i32 {
    let (x, y) = input.point;
    let w = input.window;
    // Outer-window-local coordinates (borders, drag band).
    let lx = x - w.left;
    let ly = y - w.top;
    // Client-local coordinates (drag rects). NEVER derived from the window
    // origin: maximized padding offsets the client area inside the frame.
    let cx = x - input.client_origin.0;
    let cy = y - input.client_origin.1;
    let ww = w.width();
    let wh = (w.bottom - w.top).max(0);
    let b = input.border as i32;

    // (1) Resize frame. Skipped when maximized or unresizable (maximized
    // top edge still drags via the caption rule below).
    if !input.maximized && input.resizable && b > 0 {
        let left = lx < b;
        let right = lx >= ww - b;
        let top = ly < b;
        let bottom = ly >= wh - b;
        if top && left {
            return ht::TOPLEFT;
        }
        if top && right {
            return ht::TOPRIGHT;
        }
        if bottom && left {
            return ht::BOTTOMLEFT;
        }
        if bottom && right {
            return ht::BOTTOMRIGHT;
        }
        if left {
            return ht::LEFT;
        }
        if right {
            return ht::RIGHT;
        }
        if bottom {
            return ht::BOTTOM;
        }
        // Top edge: the overlay's drag band owns the top strip
        // (drag/double-click/snap). Pure-top resize applies only when the
        // drag band is 0.
        if top && input.drag_band == 0 {
            return ht::TOP;
        }
    }

    // (2) Reported DWM strip is reserved CLIENT. Never subdivided, never
    // caption: only DWM authoritatively returns 8/9/20.
    if let Some(strip) = input.buttons {
        if strip.contains(x, y) {
            return ht::CLIENT;
        }
    }

    // Client-local point for rect tests (rects are client-local).
    let in_rect = |r: &RectI| -> bool { r.contains(cx, cy) };

    // (3) No-drag holes win over everything caption-like.
    for r in &input.drag.no_drag {
        if in_rect(r) {
            return ht::CLIENT;
        }
    }
    // Drag band across the top (window-local y < band height).
    if input.drag_band > 0 && ly >= 0 && ly < input.drag_band as i32 && lx >= 0 && lx < ww {
        return ht::CAPTION;
    }
    for r in &input.drag.draggable {
        if in_rect(r) {
            return ht::CAPTION;
        }
    }

    // (4) Client. Never NOWHERE.
    ht::CLIENT
}

/// Counters proving the overlay is installed and handling messages.
/// Readable from the guard (`diagnostics()`); the PowerShell harness can
/// additionally probe native `WM_NCHITTEST` results externally.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChromeStats {
    pub nc_calc_size: u64,
    pub nc_hit_test: u64,
    /// Hits resolved by `DwmDefWindowProc` (native buttons/hover).
    pub dwm_handled: u64,
    /// Hits resolved by [`hit_test_chrome`] fallback.
    pub fallback_handled: u64,
    pub system_menus_shown: u64,
}

/// Portable stub guard (non-Windows): installs nothing. `Native`/`Hidden`
/// resolve without OS work; `Overlay` reports `Unsupported` so the runtime
/// falls back to `Native`. Same name and `install` signature as the Windows
/// guard in `chrome_winit` — `chrome::WindowsChrome` resolves to the REAL
/// guard under `cfg(windows)` (see the re-export in `lib.rs`), so this stub
/// never shadows it there.
#[cfg(not(windows))]
#[derive(Debug)]
pub struct WindowsChrome {
    mode: Titlebar,
    stats: ChromeStats,
}

#[cfg(not(windows))]
impl WindowsChrome {
    pub fn install(
        _window: &winit::window::Window,
        opts: &WindowOptions,
    ) -> Result<Self, ChromeError> {
        opts.validated()
            .map_err(|_| ChromeError::InvalidOptions("WindowOptions failed validation"))?;
        match opts.effective_titlebar() {
            Titlebar::Native | Titlebar::Hidden => Ok(Self {
                mode: opts.effective_titlebar(),
                stats: ChromeStats::default(),
            }),
            Titlebar::Overlay => Err(ChromeError::Unsupported(
                "window overlay chrome is Windows-only in this milestone",
            )),
        }
    }

    pub fn mode(&self) -> Titlebar {
        self.mode
    }

    /// Tuple API shared with the live guard: accepts logical-px tuples
    /// directly. Stub stores nothing (no OS registration exists); returns
    /// `Ok` so runtime code paths stay uniform across targets.
    pub fn set_drag_rects(
        &self,
        _draggable: &[(f64, f64, f64, f64)],
        _no_drag: &[(f64, f64, f64, f64)],
        _scale: f64,
    ) -> Result<(), ChromeError> {
        Ok(())
    }

    /// Canonical [`DragRegions`] (logical px) entry point. Same signature
    /// on stub and live guard; the stub accepts and stores nothing.
    pub fn set_drag_regions(&self, _regions: &DragRegions, _scale: f64) -> Result<(), ChromeError> {
        Ok(())
    }

    pub fn diagnostics(&self) -> ChromeStats {
        self.stats.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::window::{Titlebar, WindowOptions};

    fn input<'a>(point: (i32, i32), drag: &'a DragRects, buttons: Option<RectI>) -> HitInput<'a> {
        HitInput {
            point,
            window: RectI {
                left: 100,
                top: 100,
                right: 900,
                bottom: 700,
            },
            // Restored window: client origin == window origin.
            client_origin: (100, 100),
            border: 8,
            buttons,
            drag_band: 40,
            drag,
            maximized: false,
            resizable: true,
        }
    }

    #[test]
    fn window_options_chrome_defaults() {
        let o = WindowOptions::default();
        assert_eq!(o.titlebar, Titlebar::Native);
        assert_eq!(o.titlebar_height, 40.0);
        assert!(o.minimizable && o.maximizable && o.closable);
        assert!(o.validated().is_ok());
        assert_eq!(o.effective_titlebar(), Titlebar::Native);
        // decorations(false) takes precedence: fully frameless.
        let over = WindowOptions {
            titlebar: Titlebar::Overlay,
            ..WindowOptions::default()
        };
        assert_eq!(over.effective_titlebar(), Titlebar::Overlay);
        assert_eq!(
            WindowOptions {
                decorations: false,
                ..over
            }
            .effective_titlebar(),
            Titlebar::Hidden
        );
    }

    #[test]
    fn stub_guard_reports_honestly() {
        // No window needed: validation happens before any handle use, and
        // the stub never touches the window. (Signature keeps &Window for
        // parity with the Windows guard.)
        assert_eq!(
            WindowOptions {
                titlebar_height: f64::NAN,
                ..WindowOptions::default()
            }
            .validated(),
            Err(crate::window::WindowError::InvalidChromeHeight)
        );
    }

    #[test]
    fn resize_borders_and_corners_win() {
        let drag = DragRects::default();
        assert_eq!(
            hit_test_chrome(&input((100, 100), &drag, None)),
            ht::TOPLEFT
        );
        assert_eq!(
            hit_test_chrome(&input((899, 699), &drag, None)),
            ht::BOTTOMRIGHT
        );
        assert_eq!(hit_test_chrome(&input((100, 400), &drag, None)), ht::LEFT);
        assert_eq!(hit_test_chrome(&input((899, 400), &drag, None)), ht::RIGHT);
        assert_eq!(hit_test_chrome(&input((400, 699), &drag, None)), ht::BOTTOM);
    }

    #[test]
    fn drag_band_is_caption_with_nodrag_holes() {
        let drag = DragRects {
            draggable: vec![],
            // Window-local hole (e.g. Todo search field).
            no_drag: vec![RectI {
                left: 0,
                top: 0,
                right: 100,
                bottom: 40,
            }],
        };
        // Drag band (below the 8px top resize sliver).
        assert_eq!(
            hit_test_chrome(&input((400, 120), &drag, None)),
            ht::CAPTION
        );
        // No-drag hole (screen 100..200 = local 0..100) stays client.
        assert_eq!(hit_test_chrome(&input((150, 120), &drag, None)), ht::CLIENT);
        // Content below the band is client, never nowhere.
        assert_eq!(hit_test_chrome(&input((400, 400), &drag, None)), ht::CLIENT);
    }

    #[test]
    fn reported_strip_is_reserved_client_never_guessed() {
        let drag = DragRects::default();
        // 135px strip at top-right of window (screen x 765..900, y 100..132).
        let strip = RectI {
            left: 765,
            top: 100,
            right: 900,
            bottom: 132,
        };
        // All three thirds resolve to reserved CLIENT: no equal-cell
        // guessing (unreliable under disabled buttons, DPI/theme widths,
        // RTL). Native 8/9/20 come only from DwmDefWindowProc live.
        for x in [780, 830, 880] {
            assert_eq!(
                hit_test_chrome(&input((x, 110), &drag, Some(strip))),
                ht::CLIENT,
                "strip must be reserved, not subdivided"
            );
        }
        // Without a reported strip, the same band point is caption.
        assert_eq!(
            hit_test_chrome(&input((400, 110), &drag, None)),
            ht::CAPTION
        );
    }

    #[test]
    fn maximized_has_no_resize_frame_but_keeps_caption() {
        let drag = DragRects::default();
        let mut i = input((100, 100), &drag, None);
        i.maximized = true;
        // Corner is caption (drag band), not TOPLEFT — maximized windows
        // must not offer resize grips.
        assert_eq!(hit_test_chrome(&i), ht::CAPTION);
        i.point = (100, 400);
        assert_eq!(hit_test_chrome(&i), ht::CLIENT);
    }

    #[test]
    fn unresizable_window_has_no_borders() {
        let drag = DragRects::default();
        let mut i = input((100, 400), &drag, None);
        i.resizable = false;
        assert_eq!(hit_test_chrome(&i), ht::CLIENT);
    }

    #[test]
    fn explicit_draggable_rects_outside_band() {
        let drag = DragRects {
            draggable: vec![RectI {
                left: 0,
                top: 400,
                right: 200,
                bottom: 440,
            }],
            no_drag: vec![],
        };
        // Screen (150, 520) = client-local (50, 420).
        assert_eq!(
            hit_test_chrome(&input((150, 520), &drag, None)),
            ht::CAPTION
        );
    }

    #[test]
    fn maximized_client_offset_tracks_padding() {
        // Maximized: 8px frame padding shifts the client origin inside the
        // outer window. A no-drag hole at client-local (0..100, 0..40) must
        // still hit even though its screen position moved.
        let drag = DragRects {
            draggable: vec![],
            no_drag: vec![RectI {
                left: 0,
                top: 0,
                right: 100,
                bottom: 40,
            }],
        };
        let mut i = input((150, 120), &drag, None);
        i.maximized = true;
        i.client_origin = (108, 108);
        // Screen (150,120) = client-local (42,12): inside the hole.
        assert_eq!(hit_test_chrome(&i), ht::CLIENT);
        // Discriminating point: screen (108,108) = client (0,0) -> hole.
        // Against the OUTER origin it would read (8,8) and still hit here,
        // so also verify a point outside the hole stays caption: screen
        // (250,120) = client (142,12) -> drag band -> caption.
        i.point = (250, 120);
        assert_eq!(hit_test_chrome(&i), ht::CAPTION);
    }
}
