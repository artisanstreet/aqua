pub use crate::services::{
    Clipboard, ClipboardBackend, ClipboardError, ClipboardImage, MemoryClipboard,
};

pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    (
        Box::new(MemoryClipboard::new()),
        ClipboardBackend::MemoryFallback,
    )
}
