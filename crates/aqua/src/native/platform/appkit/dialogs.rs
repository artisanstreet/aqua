use std::path::PathBuf;

use aqua_protocol::DialogSpec;
use aqua_shell::ShellError;
use aqua_shell_appkit::{Waker, Window};

pub type DialogResult = Result<Vec<PathBuf>, ShellError>;

pub enum DialogJob {}

impl DialogJob {
    pub fn poll(&self) -> Option<DialogResult> {
        match *self {}
    }
}

pub fn open(
    _spec: &DialogSpec,
    _owner: Option<Window>,
    _waker: Waker,
) -> Result<DialogJob, ShellError> {
    Err(ShellError::Dialog(
        "file dialogs are not available on macOS yet".into(),
    ))
}
