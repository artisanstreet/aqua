pub use crate::clipboard::{PasteboardName, SystemClipboard, PASTEBOARD_VARIABLE};
pub use aqua_shell::services::{
    Clipboard, ClipboardBackend, ClipboardError, ClipboardImage, MemoryClipboard,
};

pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    (
        Box::new(SystemClipboard::open(&PasteboardName::from_environment())),
        ClipboardBackend::System,
    )
}
