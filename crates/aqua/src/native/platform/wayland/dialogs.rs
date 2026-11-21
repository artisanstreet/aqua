use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use aqua_protocol::{DialogKind, DialogSpec};
use aqua_shell::picker::{DialogRequest, FileFilter};
use aqua_shell::ShellError;
use aqua_shell_wayland::{Waker, Window};

pub type DialogResult = Result<Vec<PathBuf>, ShellError>;

pub struct DialogJob {
    receiver: Receiver<DialogResult>,
}

impl DialogJob {
    pub fn poll(&self) -> Option<DialogResult> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => {
                Some(Err(ShellError::Dialog("dialog thread vanished".into())))
            }
        }
    }
}

fn request_of(spec: &DialogSpec) -> DialogRequest {
    let request = match &spec.kind {
        DialogKind::OpenFile { multiple: false } => DialogRequest::open_file(),
        DialogKind::OpenFile { multiple: true } => DialogRequest::open_files(),
        DialogKind::SaveFile { suggested_name } => {
            DialogRequest::save_file().file_name(suggested_name.clone())
        }
        DialogKind::PickFolder => DialogRequest::pick_folder(),
    };
    spec.filters
        .iter()
        .fold(request.title(spec.title.clone()), |request, filter| {
            let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
            request.filter(FileFilter::new(filter.label.clone(), &extensions))
        })
}

pub fn open(
    spec: &DialogSpec,
    _owner: Option<Window>,
    waker: Waker,
) -> Result<DialogJob, ShellError> {
    let request = request_of(spec);
    let (sender, receiver) = channel();
    std::thread::Builder::new()
        .name("aqua-dialog".into())
        .spawn(move || {
            let result = request
                .execute_multi()
                .map_err(|error| ShellError::Dialog(error.to_string()));
            let _ = sender.send(result);
            waker.wake();
        })
        .map_err(|error| ShellError::Dialog(error.to_string()))?;
    Ok(DialogJob { receiver })
}
