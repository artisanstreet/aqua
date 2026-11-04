use aqua_protocol::{ClipboardImage, RequestId, ShellEvent};
use aqua_shell_win32::services::{Clipboard, ClipboardError};

fn tolerated<T>(what: &str, result: Result<Option<T>, ClipboardError>) -> Option<T> {
    match result {
        Ok(value) => value,
        Err(ClipboardError::Unsupported(_)) => None,
        Err(error) => {
            eprintln!("aqua: clipboard {what} read failed: {error}");
            None
        }
    }
}

pub fn clipboard_event(clipboard: &mut dyn Clipboard, request: RequestId) -> ShellEvent {
    let text = tolerated("text", clipboard.get_text());
    let image = tolerated("image", clipboard.get_image()).map(|image| ClipboardImage {
        width: image.width(),
        height: image.height(),
        rgba: std::sync::Arc::from(image.rgba()),
    });
    let paths = tolerated("files", clipboard.get_files())
        .unwrap_or_default()
        .into_iter()
        .map(|path| path.to_string_lossy().into_owned())
        .collect();
    ShellEvent::Clipboard {
        request,
        text,
        image,
        paths,
    }
}
