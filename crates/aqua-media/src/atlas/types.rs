#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum PageFormat {
    Mask8,
    Color8,
}

impl PageFormat {
    pub fn bytes_per_texel(self) -> u32 {
        match self {
            PageFormat::Mask8 => 1,
            PageFormat::Color8 => 4,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PageId {
    pub format: PageFormat,
    pub index: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Placement {
    pub page: PageId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub page_width: u32,
    pub page_height: u32,
    pub bearing: [i32; 2],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Slot {
    Blank { bearing: [i32; 2] },
    Resident(Placement),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    pub bearing: [i32; 2],
    pub data: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PageCreate {
    pub id: PageId,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TexelUpload {
    pub page: PageId,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub data: Vec<u8>,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AtlasUpdates {
    pub released: Vec<PageId>,
    pub created: Vec<PageCreate>,
    pub uploads: Vec<TexelUpload>,
}

impl AtlasUpdates {
    pub fn is_empty(&self) -> bool {
        self.released.is_empty() && self.created.is_empty() && self.uploads.is_empty()
    }

    pub fn upload_bytes(&self) -> u64 {
        self.uploads
            .iter()
            .map(|upload| upload.data.len() as u64)
            .sum()
    }

    pub fn merge(&mut self, other: AtlasUpdates) {
        self.released.extend(other.released);
        self.created.extend(other.created);
        self.uploads.extend(other.uploads);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AtlasError {
    TooLarge {
        width: u32,
        height: u32,
        max_edge: u32,
    },
    DataSize {
        expected: usize,
        actual: usize,
    },
}

impl std::fmt::Display for AtlasError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AtlasError::TooLarge {
                width,
                height,
                max_edge,
            } => {
                write!(
                    f,
                    "{width}x{height} exceeds the atlas edge limit {max_edge}"
                )
            }
            AtlasError::DataSize { expected, actual } => {
                write!(f, "texel data is {actual} bytes, expected {expected}")
            }
        }
    }
}

impl std::error::Error for AtlasError {}
