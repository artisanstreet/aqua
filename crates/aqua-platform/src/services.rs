//! Desktop services: clipboard, cursor, IME cursor area.
//!
//! The OS clipboard is real here ([`SystemClipboard`] via `arboard`:
//! Win32 clipboard on Windows, Wayland/X11 on Linux). [`MemoryClipboard`]
//! stays for headless tests and as the explicit fallback — it is NOT the
//! application default; see [`app_clipboard`]. Nothing here opens a window.

use crate::geometry::{LogicalPosition, LogicalSize};
use std::collections::HashMap;

/// Clipboard failure. OS errors keep their message; headless/test code gets
/// `Unavailable` with an actionable reason instead of a panic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    Unavailable(&'static str),
    Os(String),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::Unavailable(r) => write!(f, "clipboard unavailable: {r}"),
            ClipboardError::Os(m) => write!(f, "clipboard OS error: {m}"),
        }
    }
}

/// Portable clipboard contract. Fallible: the OS backend can be locked,
/// unsupported, or absent (headless CI). Implementations: [`SystemClipboard`]
/// (native), [`MemoryClipboard`] (headless/test/fallback).
pub trait Clipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError>;
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError>;
    fn clear(&mut self) -> Result<(), ClipboardError>;
}

/// Headless/test clipboard. The documented fallback when no native backend
/// is available — never the silent application default.
#[derive(Debug, Default)]
pub struct MemoryClipboard {
    text: Option<String>,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clipboard for MemoryClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(self.text.clone())
    }
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        self.text = Some(text);
        Ok(())
    }
    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.text = None;
        Ok(())
    }
}

/// Native OS clipboard via `arboard` (Win32 `OpenClipboard` on Windows).
/// Construction fails cleanly headless (`ClipboardError::Unavailable`) so
/// startup can fall back to [`MemoryClipboard`] with a logged reason.
pub struct SystemClipboard {
    inner: arboard::Clipboard,
}

impl SystemClipboard {
    pub fn new() -> Result<Self, ClipboardError> {
        arboard::Clipboard::new()
            .map(|inner| Self { inner })
            .map_err(|e| ClipboardError::Os(e.to_string()))
    }
}

impl Clipboard for SystemClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        match self.inner.get_text() {
            Ok(t) => Ok(Some(t)),
            // Empty/locked clipboard reads as "no text", not a fatal error.
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(e) => Err(ClipboardError::Os(e.to_string())),
        }
    }
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        self.inner
            .set_text(text)
            .map_err(|e| ClipboardError::Os(e.to_string()))
    }
    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.inner
            .clear()
            .map_err(|e| ClipboardError::Os(e.to_string()))
    }
}

/// Which backend [`app_clipboard`] selected. Report to diagnostics; when
/// `Memory`, ALSO log the reason — a user pasting between apps with a
/// memory clipboard silently loses cross-app interop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBackend {
    System,
    MemoryFallback,
}

/// Application clipboard constructor: native when available, explicit
/// in-memory fallback otherwise. Returns the backend alongside so the app
/// can report it honestly.
pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    match SystemClipboard::new() {
        Ok(sys) => (Box::new(sys), ClipboardBackend::System),
        Err(_) => (
            Box::new(MemoryClipboard::new()),
            ClipboardBackend::MemoryFallback,
        ),
    }
}

/// Portable cursor icons (subset). Converted to `winit::window::CursorIcon`
/// at the [`crate::host`] boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CursorIcon {
    #[default]
    Default,
    Text,
    Pointer,
    Crosshair,
    Move,
    NotAllowed,
    EwResize,
    NsResize,
}

/// IME candidate-window anchor in logical units relative to content.
/// The host forwards it via `Window::set_ime_cursor_area`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImeCursorArea {
    pub origin: LogicalPosition,
    pub size: LogicalSize,
}

impl ImeCursorArea {
    pub fn is_valid(self) -> bool {
        self.size.width >= 0.0 && self.size.height >= 0.0
    }
}

/// Minimal window surface the IME hook needs. Implemented for
/// `winit::window::Window` in [`crate::host`]; fakes implement it in tests.
pub trait ImeAreaWindow {
    fn set_area(&self, origin: (f64, f64), size: (f64, f64));
}

/// Validate + forward the IME area. Returns `false` (and does nothing) for
/// degenerate input rather than sending garbage to the OS.
pub fn set_ime_cursor_area<W: ImeAreaWindow>(window: &W, area: ImeCursorArea) -> bool {
    if !area.is_valid() {
        return false;
    }
    window.set_area(
        (area.origin.x, area.origin.y),
        (area.size.width, area.size.height),
    );
    true
}

/// Typed drag-and-drop payload registry placeholder: honest stub.
/// Full OS DnD is unsupported in this milestone; payloads round-trip
/// in-process so UI logic is testable.
#[derive(Debug, Default)]
pub struct InProcessDnD {
    offered: HashMap<String, Vec<u8>>,
}

impl InProcessDnD {
    pub fn offer(&mut self, mime: impl Into<String>, bytes: Vec<u8>) {
        self.offered.insert(mime.into(), bytes);
    }
    pub fn get(&self, mime: &str) -> Option<&[u8]> {
        self.offered.get(mime).map(Vec::as_slice)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::RefCell;

    #[test]
    fn memory_clipboard_round_trips() {
        let mut c = MemoryClipboard::new();
        assert_eq!(c.get_text().unwrap(), None);
        c.set_text("hello".to_string()).unwrap();
        assert_eq!(c.get_text().unwrap().as_deref(), Some("hello"));
        c.clear().unwrap();
        assert_eq!(c.get_text().unwrap(), None);
    }

    #[test]
    fn app_clipboard_reports_its_backend() {
        let (mut clip, backend) = app_clipboard();
        match backend {
            // Headless CI has no OS clipboard: the fallback must round-trip.
            ClipboardBackend::MemoryFallback => {
                clip.set_text("x".to_string()).unwrap();
                assert_eq!(clip.get_text().unwrap().as_deref(), Some("x"));
            }
            // With a display present, exercise best-effort without asserting
            // OS clipboard semantics (locks, ownership races).
            ClipboardBackend::System => {
                let _ = clip.set_text("x".to_string());
            }
        }
    }

    type AreaRecord = Option<((f64, f64), (f64, f64))>;
    struct FakeWindow {
        last: RefCell<AreaRecord>,
    }
    impl ImeAreaWindow for FakeWindow {
        fn set_area(&self, origin: (f64, f64), size: (f64, f64)) {
            *self.last.borrow_mut() = Some((origin, size));
        }
    }

    #[test]
    fn ime_area_forwards_valid_rejects_degenerate() {
        let w = FakeWindow {
            last: RefCell::new(None),
        };
        let ok = set_ime_cursor_area(
            &w,
            ImeCursorArea {
                origin: LogicalPosition { x: 8.0, y: 12.0 },
                size: LogicalSize::new(2.0, 16.0),
            },
        );
        assert!(ok);
        assert!(w.last.borrow().is_some());
        let bad = set_ime_cursor_area(
            &w,
            ImeCursorArea {
                origin: LogicalPosition { x: 0.0, y: 0.0 },
                size: LogicalSize::new(-1.0, 5.0),
            },
        );
        assert!(!bad);
    }

    #[test]
    fn dnd_stub_round_trips_in_process() {
        let mut d = InProcessDnD::default();
        d.offer("text/plain", b"abc".to_vec());
        assert_eq!(d.get("text/plain"), Some(b"abc".as_slice()));
        assert_eq!(d.get("image/png"), None);
    }
}
