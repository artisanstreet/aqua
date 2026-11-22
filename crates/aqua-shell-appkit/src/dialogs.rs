use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use aqua_protocol::{DialogKind, DialogSpec};
use aqua_shell::ShellError;
use block2::RcBlock;
use objc2::rc::Retained;
use objc2::Message;
use objc2_app_kit::{NSModalResponse, NSModalResponseOK, NSOpenPanel, NSSavePanel};
use objc2_foundation::{MainThreadMarker, NSArray, NSString, NSURL};

use crate::looping::Waker;
use crate::window::Window;

pub type DialogResult = Result<Vec<PathBuf>, ShellError>;

pub struct DialogJob {
    receiver: Receiver<DialogResult>,
}

impl DialogJob {
    pub fn poll(&self) -> Option<DialogResult> {
        match self.receiver.try_recv() {
            Ok(result) => Some(result),
            Err(TryRecvError::Empty) => None,
            Err(TryRecvError::Disconnected) => Some(Err(ShellError::Dialog(
                "the file panel ended without a result".into(),
            ))),
        }
    }
}

enum Panel {
    Open(Retained<NSOpenPanel>),
    Save(Retained<NSSavePanel>),
}

impl Panel {
    fn save_panel(&self) -> &NSSavePanel {
        match self {
            Panel::Open(panel) => panel,
            Panel::Save(panel) => panel,
        }
    }

    fn chosen(&self) -> Vec<PathBuf> {
        let urls: Vec<Retained<NSURL>> = match self {
            Panel::Open(panel) => panel.URLs().to_vec(),
            Panel::Save(panel) => panel.URL().into_iter().collect(),
        };
        urls.iter().filter_map(|url| url.to_file_path()).collect()
    }
}

fn extensions(spec: &DialogSpec) -> Vec<Retained<NSString>> {
    let mut seen: Vec<&str> = Vec::new();
    spec.filters
        .iter()
        .flat_map(|filter| filter.extensions.iter())
        .map(|extension| extension.trim_start_matches('.'))
        .filter(|extension| !extension.is_empty() && *extension != "*")
        .filter(|extension| match seen.contains(extension) {
            true => false,
            false => {
                seen.push(extension);
                true
            }
        })
        .map(NSString::from_str)
        .collect()
}

fn build(mtm: MainThreadMarker, spec: &DialogSpec) -> Panel {
    let panel = match &spec.kind {
        DialogKind::OpenFile { multiple } => {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(true);
            panel.setCanChooseDirectories(false);
            panel.setAllowsMultipleSelection(*multiple);
            Panel::Open(panel)
        }
        DialogKind::PickFolder => {
            let panel = NSOpenPanel::openPanel(mtm);
            panel.setCanChooseFiles(false);
            panel.setCanChooseDirectories(true);
            panel.setAllowsMultipleSelection(false);
            Panel::Open(panel)
        }
        DialogKind::SaveFile { suggested_name } => {
            let panel = NSSavePanel::savePanel(mtm);
            panel.setNameFieldStringValue(&NSString::from_str(suggested_name));
            Panel::Save(panel)
        }
    };
    let save = panel.save_panel();
    let title = NSString::from_str(&spec.title);
    save.setTitle(Some(&title));
    save.setMessage(Some(&title));
    save.setCanCreateDirectories(true);
    let allowed = extensions(spec);
    if !allowed.is_empty() {
        #[allow(deprecated)]
        save.setAllowedFileTypes(Some(&NSArray::from_retained_slice(&allowed)));
    }
    panel
}

pub fn open(
    spec: &DialogSpec,
    owner: Option<Window>,
    waker: Waker,
) -> Result<DialogJob, ShellError> {
    let mtm =
        MainThreadMarker::new().expect("invariant: file panels are opened on the main thread");
    let panel = build(mtm, spec);
    let (sender, receiver) = channel();
    let save = panel.save_panel().retain();
    let finish = RcBlock::new(move |response: NSModalResponse| {
        let chosen = match response == NSModalResponseOK {
            true => panel.chosen(),
            false => Vec::new(),
        };
        if sender.send(Ok(chosen)).is_ok() {
            waker.wake();
        }
    });
    let parent = owner.and_then(|window| crate::state::native(window.id()));
    match parent {
        Some(parent) => save.beginSheetModalForWindow_completionHandler(&parent, &finish),
        None => save.beginWithCompletionHandler(&finish),
    }
    Ok(DialogJob { receiver })
}
