pub use crate::clipboard::{PrimarySelection, SystemClipboard};
pub use aqua_shell::services::{
    Clipboard, ClipboardBackend, ClipboardError, ClipboardImage, MemoryClipboard,
};

pub fn app_clipboard() -> (Box<dyn Clipboard>, ClipboardBackend) {
    match SystemClipboard::new() {
        Ok(system) => (Box::new(system), ClipboardBackend::System),
        Err(error) => {
            eprintln!("aqua: clipboard: {error}; using an in-process clipboard");
            (
                Box::new(MemoryClipboard::new()),
                ClipboardBackend::MemoryFallback,
            )
        }
    }
}

pub fn primary_selection() -> Result<PrimarySelection, ClipboardError> {
    PrimarySelection::new()
}
