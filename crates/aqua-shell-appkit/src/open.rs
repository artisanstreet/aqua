use std::path::Path;

use aqua_shell::open::{Destination, ExternalTarget};
use objc2_app_kit::NSWorkspace;
use objc2_foundation::{NSArray, NSString, NSURL};

pub use aqua_shell::open::OpenError;

pub trait Workspace {
    fn open_url(&self, url: &url::Url) -> Result<(), OpenError>;
    fn open_file(&self, path: &Path) -> Result<(), OpenError>;
    fn reveal(&self, path: &Path) -> Result<(), OpenError>;
}

pub struct SystemWorkspace;

fn file_url(path: &Path) -> Result<objc2::rc::Retained<NSURL>, OpenError> {
    NSURL::from_file_path(path).ok_or(OpenError::InvalidPath)
}

fn opened(url: &NSURL, what: &str) -> Result<(), OpenError> {
    match NSWorkspace::sharedWorkspace().openURL(url) {
        true => Ok(()),
        false => Err(OpenError::Desktop(format!(
            "NSWorkspace found no application to open the {what}"
        ))),
    }
}

impl Workspace for SystemWorkspace {
    fn open_url(&self, url: &url::Url) -> Result<(), OpenError> {
        let native =
            NSURL::URLWithString(&NSString::from_str(url.as_str())).ok_or(OpenError::InvalidUrl)?;
        opened(&native, &format!("{} URL", url.scheme()))
    }

    fn open_file(&self, path: &Path) -> Result<(), OpenError> {
        opened(&*file_url(path)?, "file")
    }

    fn reveal(&self, path: &Path) -> Result<(), OpenError> {
        let selection = NSArray::from_retained_slice(&[file_url(path)?]);
        NSWorkspace::sharedWorkspace().activateFileViewerSelectingURLs(&selection);
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenTarget(ExternalTarget);

impl OpenTarget {
    pub fn url(input: &str) -> Result<Self, OpenError> {
        ExternalTarget::url(input).map(Self)
    }

    pub fn path(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::path(path).map(Self)
    }

    pub fn reveal(path: &Path) -> Result<Self, OpenError> {
        ExternalTarget::reveal(path).map(Self)
    }

    pub fn execute(&self) -> Result<(), OpenError> {
        self.execute_with(&SystemWorkspace)
    }

    pub fn execute_with(&self, workspace: &dyn Workspace) -> Result<(), OpenError> {
        match self.0.destination() {
            Destination::Url(url) => workspace.open_url(url),
            Destination::Path(path) => workspace.open_file(path),
            Destination::Reveal(path) => workspace.reveal(path),
        }
    }
}
