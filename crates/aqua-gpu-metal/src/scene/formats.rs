use aqua_graph::TextureFormat;
use objc2_metal::MTLPixelFormat;

use crate::half::{bgra8_rows_to_rgba8, rgba16f_rows_to_rgba8};

pub fn metal(format: TextureFormat) -> MTLPixelFormat {
    match format {
        TextureFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
        TextureFormat::Rgba32Float => MTLPixelFormat::RGBA32Float,
        TextureFormat::Rgba8Unorm => MTLPixelFormat::RGBA8Unorm,
        TextureFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        TextureFormat::R8Unorm => MTLPixelFormat::R8Unorm,
    }
}

pub fn persistent(format: TextureFormat) -> Option<MTLPixelFormat> {
    match format {
        TextureFormat::R8Unorm | TextureFormat::Rgba8Unorm | TextureFormat::Rgba32Float => {
            Some(metal(format))
        }
        TextureFormat::Rgba16Float | TextureFormat::Bgra8Unorm => None,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceFormat {
    Rgba16Float,
    Bgra8Unorm,
}

impl SurfaceFormat {
    pub fn metal(self) -> MTLPixelFormat {
        match self {
            SurfaceFormat::Rgba16Float => MTLPixelFormat::RGBA16Float,
            SurfaceFormat::Bgra8Unorm => MTLPixelFormat::BGRA8Unorm,
        }
    }

    pub fn texel_bytes(self) -> u64 {
        match self {
            SurfaceFormat::Rgba16Float => 8,
            SurfaceFormat::Bgra8Unorm => 4,
        }
    }

    pub fn to_rgba8(self, bytes: &[u8], width: usize, height: usize) -> Vec<u8> {
        let pitch = width * self.texel_bytes() as usize;
        match self {
            SurfaceFormat::Rgba16Float => rgba16f_rows_to_rgba8(bytes, pitch, width, height),
            SurfaceFormat::Bgra8Unorm => bgra8_rows_to_rgba8(bytes, pitch, width, height),
        }
    }
}
