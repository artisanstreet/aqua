use aqua_graph::TextureFormat;
use windows::Win32::Graphics::Dxgi::Common::*;

pub fn dxgi(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        TextureFormat::Rgba32Float => DXGI_FORMAT_R32G32B32A32_FLOAT,
        TextureFormat::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        TextureFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        TextureFormat::R8Unorm => DXGI_FORMAT_R8_UNORM,
    }
}

pub fn persistent(format: TextureFormat) -> Option<DXGI_FORMAT> {
    match format {
        TextureFormat::R8Unorm | TextureFormat::Rgba8Unorm | TextureFormat::Rgba32Float => {
            Some(dxgi(format))
        }
        TextureFormat::Rgba16Float | TextureFormat::Bgra8Unorm => None,
    }
}
