use aqua_graph::{ResourceState, TextureFormat, TextureRef};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

pub fn dxgi(format: TextureFormat) -> DXGI_FORMAT {
    match format {
        TextureFormat::Rgba16Float => DXGI_FORMAT_R16G16B16A16_FLOAT,
        TextureFormat::Rgba8Unorm => DXGI_FORMAT_R8G8B8A8_UNORM,
        TextureFormat::Bgra8Unorm => DXGI_FORMAT_B8G8R8A8_UNORM,
        TextureFormat::R8Unorm => DXGI_FORMAT_R8_UNORM,
    }
}

pub const SHADER_READ: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATES(
    D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE.0 | D3D12_RESOURCE_STATE_NON_PIXEL_SHADER_RESOURCE.0,
);

pub fn state_for(reference: &TextureRef, wanted: ResourceState) -> Option<D3D12_RESOURCE_STATES> {
    match (reference, wanted) {
        (TextureRef::Persistent(_), ResourceState::ShaderRead) => {
            Some(D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE)
        }
        _ => state(wanted),
    }
}

pub fn state(state: ResourceState) -> Option<D3D12_RESOURCE_STATES> {
    match state {
        ResourceState::Undefined => None,
        ResourceState::Present => Some(D3D12_RESOURCE_STATE_COMMON),
        ResourceState::RenderTarget => Some(D3D12_RESOURCE_STATE_RENDER_TARGET),
        ResourceState::ShaderRead => Some(SHADER_READ),
        ResourceState::CopySource => Some(D3D12_RESOURCE_STATE_COPY_SOURCE),
        ResourceState::CopyDest => Some(D3D12_RESOURCE_STATE_COPY_DEST),
        ResourceState::UnorderedAccess => Some(D3D12_RESOURCE_STATE_UNORDERED_ACCESS),
    }
}
