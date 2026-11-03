use crate::geometry::{LogicalPosition, LogicalSize};
use std::collections::HashMap;
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClipboardError {
    Unavailable(&'static str),
    Os(String),
    InvalidImage,
    Unsupported(&'static str),
}

impl std::fmt::Display for ClipboardError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClipboardError::Unavailable(r) => write!(f, "clipboard unavailable: {r}"),
            ClipboardError::Os(m) => write!(f, "clipboard OS error: {m}"),
            ClipboardError::InvalidImage => {
                write!(f, "clipboard image dimensions do not match RGBA bytes")
            }
            ClipboardError::Unsupported(format) => {
                write!(f, "clipboard format unsupported: {format}")
            }
        }
    }
}

impl std::error::Error for ClipboardError {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardImage {
    width: usize,
    height: usize,
    rgba: Vec<u8>,
}

impl ClipboardImage {
    pub fn new(width: usize, height: usize, rgba: Vec<u8>) -> Result<Self, ClipboardError> {
        let expected = width
            .checked_mul(height)
            .and_then(|pixels| pixels.checked_mul(4));
        match expected {
            Some(bytes) if width > 0 && height > 0 && bytes == rgba.len() => Ok(Self {
                width,
                height,
                rgba,
            }),
            _ => Err(ClipboardError::InvalidImage),
        }
    }

    pub fn width(&self) -> usize {
        self.width
    }
    pub fn height(&self) -> usize {
        self.height
    }
    pub fn rgba(&self) -> &[u8] {
        &self.rgba
    }
}

pub trait Clipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError>;
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError>;
    fn clear(&mut self) -> Result<(), ClipboardError>;
    fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
        Err(ClipboardError::Unsupported("image"))
    }
    fn set_image(&mut self, _image: ClipboardImage) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported("image"))
    }
    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        Err(ClipboardError::Unsupported("files"))
    }
    fn set_files(&mut self, _files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        Err(ClipboardError::Unsupported("files"))
    }
}

#[derive(Debug)]
enum ClipboardContent {
    Text(String),
    Image(ClipboardImage),
    Files(Vec<PathBuf>),
}

#[derive(Debug, Default)]
pub struct MemoryClipboard {
    content: Option<ClipboardContent>,
}

impl MemoryClipboard {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Clipboard for MemoryClipboard {
    fn get_text(&mut self) -> Result<Option<String>, ClipboardError> {
        Ok(match &self.content {
            Some(ClipboardContent::Text(text)) => Some(text.clone()),
            _ => None,
        })
    }
    fn set_text(&mut self, text: String) -> Result<(), ClipboardError> {
        self.content = Some(ClipboardContent::Text(text));
        Ok(())
    }
    fn clear(&mut self) -> Result<(), ClipboardError> {
        self.content = None;
        Ok(())
    }
    fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
        Ok(match &self.content {
            Some(ClipboardContent::Image(image)) => Some(image.clone()),
            _ => None,
        })
    }
    fn set_image(&mut self, image: ClipboardImage) -> Result<(), ClipboardError> {
        self.content = Some(ClipboardContent::Image(image));
        Ok(())
    }
    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        Ok(match &self.content {
            Some(ClipboardContent::Files(files)) => Some(files.clone()),
            _ => None,
        })
    }
    fn set_files(&mut self, files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        self.content = match files.is_empty() {
            true => None,
            false => Some(ClipboardContent::Files(files)),
        };
        Ok(())
    }
}

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
    fn get_image(&mut self) -> Result<Option<ClipboardImage>, ClipboardError> {
        match self.inner.get_image() {
            Ok(image) => {
                ClipboardImage::new(image.width, image.height, image.bytes.into_owned()).map(Some)
            }
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(ClipboardError::Os(error.to_string())),
        }
    }
    fn set_image(&mut self, image: ClipboardImage) -> Result<(), ClipboardError> {
        self.inner
            .set_image(arboard::ImageData {
                width: image.width,
                height: image.height,
                bytes: std::borrow::Cow::Owned(image.rgba),
            })
            .map_err(|error| ClipboardError::Os(error.to_string()))
    }
    fn get_files(&mut self) -> Result<Option<Vec<PathBuf>>, ClipboardError> {
        match self.inner.get().file_list() {
            Ok(files) if files.is_empty() => Ok(None),
            Ok(files) => Ok(Some(files)),
            Err(arboard::Error::ContentNotAvailable) => Ok(None),
            Err(error) => Err(ClipboardError::Os(error.to_string())),
        }
    }
    fn set_files(&mut self, files: Vec<PathBuf>) -> Result<(), ClipboardError> {
        match files.is_empty() {
            true => self.clear(),
            false => self
                .inner
                .set()
                .file_list(&files)
                .map_err(|error| ClipboardError::Os(error.to_string())),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBackend {
    System,
    MemoryFallback,
}

pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    match SystemClipboard::new() {
        Ok(sys) => (Box::new(sys), ClipboardBackend::System),
        Err(_) => (
            Box::new(MemoryClipboard::new()),
            ClipboardBackend::MemoryFallback,
        ),
    }
}

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

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ImeCursorArea {
    pub origin: LogicalPosition,
    pub size: LogicalSize,
}

impl ImeCursorArea {
    pub fn is_valid(self) -> bool {
        self.origin.x.is_finite()
            && self.origin.y.is_finite()
            && self.size.width.is_finite()
            && self.size.height.is_finite()
            && self.size.width >= 0.0
            && self.size.height >= 0.0
    }
}

pub trait ImeAreaWindow {
    fn set_area(&self, origin: (f64, f64), size: (f64, f64));
}

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
