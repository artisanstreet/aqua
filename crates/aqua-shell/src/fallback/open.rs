use std::path::Path;

use crate::open::ExternalTarget;
pub use crate::open::OpenError;

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
        Err(OpenError::Unsupported(
            "this platform has no desktop shell backend",
        ))
    }
}
