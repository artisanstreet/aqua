//! Native file dialogs via `rfd` (Win32 on Windows, portal-based on Linux).
//!
//! The request builder is pure and headless-testable; `execute()` opens the
//! real OS dialog and therefore needs a display + event loop. Headless use
//! returns [`DialogError::Unavailable`], never a panic and never a fake path.

use std::path::PathBuf;

/// What the dialog is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DialogKind {
    OpenFile,
    OpenFiles,
    SaveFile,
    PickFolder,
}

/// One file-type filter, e.g. `("PNG images", ["png"])`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    pub extensions: Vec<String>,
}

impl FileFilter {
    pub fn new(name: impl Into<String>, extensions: &[&str]) -> Self {
        Self {
            name: name.into(),
            extensions: extensions.iter().map(|s| s.to_string()).collect(),
        }
    }
}

/// Pure dialog description. Build it headless, `execute()` it on the GUI thread.
#[derive(Debug, Clone, Default)]
pub struct DialogRequest {
    pub kind: Option<DialogKind>,
    pub title: Option<String>,
    pub start_dir: Option<PathBuf>,
    pub file_name: Option<String>,
    pub filters: Vec<FileFilter>,
}

impl DialogRequest {
    pub fn open_file() -> Self {
        Self {
            kind: Some(DialogKind::OpenFile),
            ..Self::default()
        }
    }

    pub fn open_files() -> Self {
        Self {
            kind: Some(DialogKind::OpenFiles),
            ..Self::default()
        }
    }

    pub fn save_file() -> Self {
        Self {
            kind: Some(DialogKind::SaveFile),
            ..Self::default()
        }
    }

    pub fn pick_folder() -> Self {
        Self {
            kind: Some(DialogKind::PickFolder),
            ..Self::default()
        }
    }

    pub fn title(mut self, title: impl Into<String>) -> Self {
        self.title = Some(title.into());
        self
    }

    pub fn start_dir(mut self, dir: PathBuf) -> Self {
        self.start_dir = Some(dir);
        self
    }

    pub fn file_name(mut self, name: impl Into<String>) -> Self {
        self.file_name = Some(name.into());
        self
    }

    pub fn filter(mut self, filter: FileFilter) -> Self {
        self.filters.push(filter);
        self
    }

    fn validated(&self) -> Result<DialogKind, DialogError> {
        self.kind.ok_or(DialogError::NoKind)
    }

    fn apply_to(&self, mut dialog: rfd::FileDialog) -> rfd::FileDialog {
        if let Some(t) = &self.title {
            dialog = dialog.set_title(t);
        }
        if let Some(d) = &self.start_dir {
            dialog = dialog.set_directory(d);
        }
        if let Some(n) = &self.file_name {
            dialog = dialog.set_file_name(n);
        }
        for f in &self.filters {
            let exts: Vec<&str> = f.extensions.iter().map(String::as_str).collect();
            dialog = dialog.add_filter(&f.name, &exts);
        }
        dialog
    }

    /// Open the OS dialog (blocking, GUI thread only). `Ok(None)` = user
    /// cancelled — a normal outcome, not an error.
    pub fn execute(&self) -> Result<Option<PathBuf>, DialogError> {
        let kind = self.validated()?;
        if !dialogs_available() {
            return Err(DialogError::Unavailable(
                "no display / portal for native dialogs in this session",
            ));
        }
        let dialog = self.apply_to(rfd::FileDialog::new());
        // rfd returns None on cancel AND on backend failure; the capability
        // pre-check above keeps the failure case honest in practice.
        Ok(match kind {
            DialogKind::OpenFile => dialog.pick_file(),
            DialogKind::SaveFile => dialog.save_file(),
            DialogKind::PickFolder => dialog.pick_folder(),
            DialogKind::OpenFiles => dialog.pick_files().and_then(|mut v| v.pop()),
        })
    }

    /// Multi-select variant: `Ok(vec![])` = user cancelled.
    pub fn execute_multi(&self) -> Result<Vec<PathBuf>, DialogError> {
        let kind = self.validated()?;
        if !dialogs_available() {
            return Err(DialogError::Unavailable(
                "no display / portal for native dialogs in this session",
            ));
        }
        let dialog = self.apply_to(rfd::FileDialog::new());
        Ok(match kind {
            DialogKind::OpenFiles => dialog.pick_files().unwrap_or_default(),
            DialogKind::OpenFile => dialog.pick_file().into_iter().collect(),
            DialogKind::SaveFile => dialog.save_file().into_iter().collect(),
            DialogKind::PickFolder => dialog.pick_folder().into_iter().collect(),
        })
    }
}

/// Dialog failure. Cancellation is NOT an error (`Ok(None)` / empty vec).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DialogError {
    NoKind,
    Unavailable(&'static str),
}

impl std::fmt::Display for DialogError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DialogError::NoKind => write!(f, "dialog request has no kind"),
            DialogError::Unavailable(r) => write!(f, "dialogs unavailable: {r}"),
        }
    }
}

/// Whether this session can show a native dialog: needs a display server.
/// Headless CI / `DISPLAY`-less Linux returns false so callers disable menu
/// items instead of opening a dialog that can never appear.
pub fn dialogs_available() -> bool {
    #[cfg(windows)]
    {
        true
    }
    #[cfg(target_os = "macos")]
    {
        true
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        std::env::var_os("DISPLAY").is_some() || std::env::var_os("WAYLAND_DISPLAY").is_some()
    }
    #[cfg(not(any(windows, unix)))]
    {
        false
    }
}

/// Capability report for diagnostics / menu enablement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DialogCapability {
    pub supported: bool,
    pub reason: &'static str,
}

pub fn dialogs_capability() -> DialogCapability {
    if dialogs_available() {
        DialogCapability {
            supported: true,
            reason: "native OS dialog backend reachable",
        }
    } else {
        DialogCapability {
            supported: false,
            reason: "headless session: no display/portal; disable dialog UI",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builder_is_pure_headless() {
        let req = DialogRequest::open_file()
            .title("Open")
            .file_name("untitled.png")
            .filter(FileFilter::new("Images", &["png", "jpg"]));
        assert_eq!(req.kind, Some(DialogKind::OpenFile));
        assert_eq!(req.filters.len(), 1);
        assert!(req.validated().is_ok());
        assert!(DialogRequest::default().validated().is_err());
    }

    #[test]
    fn headless_execute_is_honest() {
        if dialogs_available() {
            return; // display present: cannot test the headless path here.
        }
        let err = DialogRequest::open_file().execute().unwrap_err();
        assert!(matches!(err, DialogError::Unavailable(_)));
        assert!(!dialogs_capability().supported);
    }
}
