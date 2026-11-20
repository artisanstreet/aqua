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
    pub fn into_rgba(self) -> Vec<u8> {
        self.rgba
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardBackend {
    System,
    MemoryFallback,
}
