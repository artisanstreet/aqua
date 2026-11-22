use aqua_graph::TextureFormat;
use ash::vk;

use crate::half::{bgra8_rows_to_rgba8, rgba16f_rows_to_rgba8};

pub fn vulkan(format: TextureFormat) -> vk::Format {
    match format {
        TextureFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
        TextureFormat::Rgba32Float => vk::Format::R32G32B32A32_SFLOAT,
        TextureFormat::Rgba8Unorm => vk::Format::R8G8B8A8_UNORM,
        TextureFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
        TextureFormat::R8Unorm => vk::Format::R8_UNORM,
    }
}

pub fn persistent(format: TextureFormat) -> Option<vk::Format> {
    match format {
        TextureFormat::R8Unorm | TextureFormat::Rgba8Unorm | TextureFormat::Rgba32Float => {
            Some(vulkan(format))
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
    pub fn vulkan(self) -> vk::Format {
        match self {
            SurfaceFormat::Rgba16Float => vk::Format::R16G16B16A16_SFLOAT,
            SurfaceFormat::Bgra8Unorm => vk::Format::B8G8R8A8_UNORM,
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
