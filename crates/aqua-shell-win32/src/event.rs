use aqua_core::{InputEvent, Point};
use aqua_protocol::{CaptionControl, WindowId, WindowMaterial};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq)]
pub enum FileDrag {
    Enter {
        paths: Vec<PathBuf>,
        position: Point,
    },
    Over {
        position: Point,
    },
    Leave,
    Drop {
        paths: Vec<PathBuf>,
        position: Point,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub enum ShellEvent {
    Resized { width: u32, height: u32 },
    Moved { x: i32, y: i32 },
    ScaleChanged { scale: f64 },
    CloseRequested,
    Focused(bool),
    Minimized(bool),
    Covered(bool),
    Maximized(bool),
    Input(InputEvent),
    FileDrag(FileDrag),
    PointerEntered,
    PointerLeft,
    CaptionHover(Option<CaptionControl>),
    RawMouse { dx: i32, dy: i32 },
    PreferencesChanged,
    Redraw,
    Destroyed,
}

pub trait Handler: 'static {
    fn window_event(&mut self, id: WindowId, event: ShellEvent);
    fn waitable(&mut self, _index: usize) {}
    fn woken(&mut self) {}
    fn tick(&mut self, now: aqua_protocol::Instant) -> Option<aqua_protocol::Instant>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Chrome {
    Native,
    Custom,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    Normal,
    Panel,
}

#[derive(Debug, Clone)]
pub struct WindowOptions {
    pub title: String,
    pub logical_size: (f64, f64),
    pub min_logical_size: Option<(f64, f64)>,
    pub position: Option<(i32, i32)>,
    pub kind: WindowKind,
    pub chrome: Chrome,
    pub material: WindowMaterial,
    pub dark: bool,
    pub topmost: bool,
    pub owner: Option<WindowId>,
    pub resizable: bool,
    pub minimizable: bool,
    pub maximizable: bool,
    pub visible: bool,
    pub alpha_surface: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: String::from("Aqua"),
            logical_size: (960.0, 640.0),
            min_logical_size: None,
            position: None,
            kind: WindowKind::Normal,
            chrome: Chrome::Native,
            material: WindowMaterial::Solid,
            dark: false,
            topmost: false,
            owner: None,
            resizable: true,
            minimizable: true,
            maximizable: true,
            visible: true,
            alpha_surface: false,
        }
    }
}

impl WindowOptions {
    pub fn panel(logical_size: (f64, f64)) -> Self {
        Self {
            kind: WindowKind::Panel,
            logical_size,
            topmost: true,
            resizable: false,
            minimizable: false,
            maximizable: false,
            ..Self::default()
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShellError {
    Win32(&'static str, i32),
    ClipboardBusy,
    ClipboardUnavailable,
    LoopAlreadyRunning,
    Dialog(String),
}

impl std::fmt::Display for ShellError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ShellError::Win32(call, code) => write!(f, "{call} failed with {code:#x}"),
            ShellError::ClipboardBusy => write!(f, "clipboard is held by another process"),
            ShellError::ClipboardUnavailable => write!(f, "clipboard has no unicode text"),
            ShellError::LoopAlreadyRunning => write!(f, "message loop already running"),
            ShellError::Dialog(reason) => write!(f, "dialog failed: {reason}"),
        }
    }
}

impl std::error::Error for ShellError {}

#[cfg(windows)]
pub(crate) fn win32(call: &'static str, error: windows::core::Error) -> ShellError {
    ShellError::Win32(call, error.code().0)
}
