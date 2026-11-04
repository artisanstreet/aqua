use crate::dialog_protocol::{
    HelperRequest, HelperResponse, decode_request, decode_response, encode_request, encode_response,
};
use crate::event::ShellError;
use crate::handle::NativeHandle;
use crate::looping::Waker;
use crate::window::Window;
use aqua_platform::dialogs::{DialogKind, DialogRequest};
use std::io::{Read, Write};
use std::os::windows::process::CommandExt;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc::{Receiver, TryRecvError, channel};

const HELPER_FLAG: &str = "--aqua-shell-dialog-helper";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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
                Some(Err(ShellError::Dialog("helper thread vanished".into())))
            }
        }
    }
}

fn run_helper(request: &HelperRequest) -> DialogResult {
    let exe = std::env::current_exe().map_err(|e| ShellError::Dialog(e.to_string()))?;
    let mut child = Command::new(exe)
        .arg(HELPER_FLAG)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| ShellError::Dialog(e.to_string()))?;
    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| ShellError::Dialog("no stdin".into()))?;
    stdin
        .write_all(encode_request(request).as_bytes())
        .map_err(|e| ShellError::Dialog(e.to_string()))?;
    drop(stdin);
    let mut output = String::new();
    child
        .stdout
        .take()
        .ok_or_else(|| ShellError::Dialog("no stdout".into()))?
        .read_to_string(&mut output)
        .map_err(|e| ShellError::Dialog(e.to_string()))?;
    let _ = child.wait();
    match decode_response(&output) {
        Ok(HelperResponse::Selected(paths)) => Ok(paths),
        Ok(HelperResponse::Failed(reason)) => Err(ShellError::Dialog(reason)),
        Err(error) => Err(ShellError::Dialog(format!("{error:?}"))),
    }
}

pub fn open(
    request: &DialogRequest,
    owner: Option<Window>,
    waker: Waker,
) -> Result<DialogJob, ShellError> {
    let helper = HelperRequest::new(request, owner.map(|w| w.hwnd))
        .ok_or_else(|| ShellError::Dialog("dialog request has no kind".into()))?;
    let (sender, receiver) = channel();
    std::thread::Builder::new()
        .name("aqua-dialog".into())
        .spawn(move || {
            let _ = sender.send(run_helper(&helper));
            waker.wake();
        })
        .map_err(|e| ShellError::Dialog(e.to_string()))?;
    Ok(DialogJob { receiver })
}

fn show(request: &HelperRequest) -> HelperResponse {
    let mut dialog = rfd::FileDialog::new();
    let handle = request
        .owner
        .map(|hwnd| NativeHandle { hwnd, hinstance: 0 });
    if let Some(handle) = handle.as_ref() {
        dialog = dialog.set_parent(handle);
    }
    if let Some(title) = &request.title {
        dialog = dialog.set_title(title);
    }
    if let Some(dir) = &request.start_dir {
        dialog = dialog.set_directory(dir);
    }
    if let Some(name) = &request.file_name {
        dialog = dialog.set_file_name(name);
    }
    for filter in &request.filters {
        let extensions: Vec<&str> = filter.extensions.iter().map(String::as_str).collect();
        dialog = dialog.add_filter(&filter.name, &extensions);
    }
    HelperResponse::Selected(match request.kind {
        DialogKind::OpenFile => dialog.pick_file().into_iter().collect(),
        DialogKind::OpenFiles => dialog.pick_files().unwrap_or_default(),
        DialogKind::SaveFile => dialog.save_file().into_iter().collect(),
        DialogKind::PickFolder => dialog.pick_folder().into_iter().collect(),
    })
}

pub fn run_helper_if_requested() -> bool {
    if std::env::args().nth(1).as_deref() != Some(HELPER_FLAG) {
        return false;
    }
    let mut input = String::new();
    let response = match std::io::stdin().read_to_string(&mut input) {
        Ok(_) => match decode_request(&input) {
            Ok(request) => show(&request),
            Err(error) => HelperResponse::Failed(format!("{error:?}")),
        },
        Err(error) => HelperResponse::Failed(error.to_string()),
    };
    let _ = std::io::stdout().write_all(encode_response(&response).as_bytes());
    true
}
