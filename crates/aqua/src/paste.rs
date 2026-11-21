use aqua_protocol::{ClipboardImage, RequestId, ShellEvent};
use aqua_shell::services::{Clipboard, ClipboardError};

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

pub(crate) enum PrimarySelection {
    Unavailable,
    #[cfg(target_os = "linux")]
    System(Box<dyn Clipboard>),
}

impl PrimarySelection {
    #[cfg(target_os = "linux")]
    pub(crate) fn platform() -> PrimarySelection {
        match crate::native::platform::shell::services::primary_selection() {
            Ok(primary) => PrimarySelection::System(Box::new(primary)),
            Err(error) => {
                eprintln!("aqua: primary selection unavailable: {error}");
                PrimarySelection::Unavailable
            }
        }
    }

    #[cfg(not(target_os = "linux"))]
    pub(crate) fn platform() -> PrimarySelection {
        PrimarySelection::Unavailable
    }

    pub(crate) fn write(&mut self, text: String) {
        match self {
            PrimarySelection::Unavailable => drop(text),
            #[cfg(target_os = "linux")]
            PrimarySelection::System(primary) => {
                if let Err(error) = primary.set_text(text) {
                    eprintln!("aqua: primary selection write failed: {error}");
                }
            }
        }
    }

    pub(crate) fn read(&mut self, request: RequestId) -> ShellEvent {
        let text = match self {
            PrimarySelection::Unavailable => None,
            #[cfg(target_os = "linux")]
            PrimarySelection::System(primary) => tolerated("primary selection", primary.get_text()),
        };
        ShellEvent::Clipboard {
            request,
            text,
            image: None,
            paths: Vec::new(),
        }
    }
}
