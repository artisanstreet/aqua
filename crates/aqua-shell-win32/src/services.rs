use std::path::PathBuf;

pub use aqua_shell::services::{
    Clipboard, ClipboardBackend, ClipboardError, ClipboardImage, MemoryClipboard,
};

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
                width: image.width(),
                height: image.height(),
                bytes: std::borrow::Cow::Owned(image.into_rgba()),
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

pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    match SystemClipboard::new() {
        Ok(sys) => (Box::new(sys), ClipboardBackend::System),
        Err(_) => (
            Box::new(MemoryClipboard::new()),
            ClipboardBackend::MemoryFallback,
        ),
    }
}
