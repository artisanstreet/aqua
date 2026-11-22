use std::path::Path;

pub use crate::icon::FileIcon;

pub fn file_icon(_path: &Path, _edge: u32) -> Option<FileIcon> {
    None
}
