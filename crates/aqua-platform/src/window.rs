//! Window requests (intent) vs observed state (fact) vs events.
//!
//! A request being accepted never proves the compositor applied it; the
//! facade must update [`ObservedWindowState`] from [`WindowEvent`]s.

use crate::geometry::{AquaWindowId, LogicalSize};

/// Portable window creation options. No native handles.
///
/// Chrome fields (`titlebar`, `titlebar_height`, button flags) are the
/// canonical chrome config: the facade builds this struct and
/// `host::window_attributes` + `WindowsChrome::install` consume it. There
/// is deliberately NO separate `AquaConfig` chrome field and NO parallel
/// `ChromeOptions` struct.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowOptions {
    pub title: String,
    pub inner_size: LogicalSize,
    pub min_size: Option<LogicalSize>,
    pub max_size: Option<LogicalSize>,
    pub resizable: bool,
    pub decorations: bool,
    pub transparent: bool,
    /// Native chrome appearance; Unknown follows the system theme.
    pub theme: ThemePreference,
    /// Frame style: `Native` (default), `Overlay` (native caption buttons
    /// over glass, Windows-only), `Hidden` (frameless).
    pub titlebar: Titlebar,
    /// Reserved drag-band height in logical px for `Overlay` (default 40).
    pub titlebar_height: f64,
    pub minimizable: bool,
    pub maximizable: bool,
    pub closable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Aqua".to_string(),
            inner_size: LogicalSize::new(800.0, 600.0),
            min_size: None,
            max_size: None,
            resizable: true,
            decorations: true,
            transparent: false,
            theme: ThemePreference::Unknown,
            titlebar: Titlebar::Native,
            titlebar_height: 40.0,
            minimizable: true,
            maximizable: true,
            closable: true,
        }
    }
}

impl WindowOptions {
    pub fn validated(&self) -> Result<(), WindowError> {
        if self.inner_size.is_empty() {
            return Err(WindowError::EmptySize);
        }
        if let Some(m) = self.min_size {
            if m.is_empty() {
                return Err(WindowError::EmptySize);
            }
        }
        if self.titlebar_height.is_nan() || self.titlebar_height < 0.0 {
            return Err(WindowError::InvalidChromeHeight);
        }
        Ok(())
    }

    /// `decorations(false)` wins over everything: fully frameless takes
    /// precedence and no overlay work is needed.
    pub fn effective_titlebar(&self) -> Titlebar {
        if !self.decorations {
            Titlebar::Hidden
        } else {
            self.titlebar
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    EmptySize,
    InvalidChromeHeight,
    Unsupported(&'static str),
}

/// Intent sent toward the host. The host may accept, defer, or reject.
#[derive(Debug, Clone, PartialEq)]
pub enum WindowRequest {
    Create(WindowOptions),
    SetTitle(AquaWindowId, String),
    SetInnerSize(AquaWindowId, LogicalSize),
    SetVisible(AquaWindowId, bool),
    RequestRedraw(AquaWindowId),
    SetMinimized(AquaWindowId, bool),
    SetMaximized(AquaWindowId, bool),
    Close(AquaWindowId),
}

/// Observed (compositor-confirmed) per-window state.
///
/// New fields are only added with a `Default`-based constructor path kept
/// working: prefer [`ObservedWindowState::new`] over struct literals so
/// future additive fields don't break call sites.
#[derive(Debug, Clone, PartialEq)]
pub struct ObservedWindowState {
    pub id: AquaWindowId,
    pub title: String,
    pub inner_size: LogicalSize,
    pub scale_factor: f64,
    pub focused: bool,
    pub visible: bool,
    pub occluded: bool,
}

impl ObservedWindowState {
    pub fn new(id: AquaWindowId, title: String, inner_size: LogicalSize) -> Self {
        Self {
            id,
            title,
            inner_size,
            scale_factor: 1.0,
            focused: false,
            visible: true,
            occluded: false,
        }
    }
}

/// Portable OS light/dark preference. `Unknown` means "not reported on this
/// platform yet" — never guessed from the app theme.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    Unknown,
}

/// Window frame style. Consumed from [`WindowOptions`] by
/// `host::window_attributes` and [`crate::chrome::WindowsChrome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Titlebar {
    /// OS frame as created (default). No subclass installed.
    #[default]
    Native,
    /// Native caption buttons over glass: keeps `WS_CAPTION` so DWM draws
    /// min/max/close (incl. snap layouts), removes caption text/background
    /// via `WM_NCCALCSIZE`, app draws the full client including the drag
    /// band. Windows-only; elsewhere the guard reports `Unsupported`.
    Overlay,
    /// Fully frameless (`decorations(false)` equivalent). No native buttons.
    Hidden,
}

/// Framework-level window event after host translation.
///
/// Non-exhaustive matching is REQUIRED at call sites: variants are added as
/// the adapter wires more of winit (e.g. `ThemeChanged` in pass 2).
#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    CloseRequested(AquaWindowId),
    Resized(AquaWindowId, LogicalSize),
    ScaleChanged(AquaWindowId, f64),
    Focused(AquaWindowId, bool),
    Occluded(AquaWindowId, bool),
    ThemeChanged(AquaWindowId, ThemePreference),
    RedrawRequested(AquaWindowId),
    Destroyed(AquaWindowId),
}
