#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Stage {
    Vertex,
    Pixel,
    Compute,
}

#[derive(Clone, Copy, Debug)]
pub struct Blob {
    pub name: &'static str,
    pub stage: Stage,
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/blobs.rs"));

pub fn find(name: &str) -> Option<&'static Blob> {
    BLOBS.iter().find(|blob| blob.name == name)
}

pub fn find_dxbc(name: &str) -> Option<&'static Blob> {
    DXBC.iter().find(|blob| blob.name == name)
}
