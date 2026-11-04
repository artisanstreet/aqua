use std::ffi::c_void;

use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::error::Error;

pub fn heap_properties(kind: D3D12_HEAP_TYPE) -> D3D12_HEAP_PROPERTIES {
    D3D12_HEAP_PROPERTIES {
        Type: kind,
        CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_UNKNOWN,
        MemoryPoolPreference: D3D12_MEMORY_POOL_UNKNOWN,
        CreationNodeMask: 1,
        VisibleNodeMask: 1,
    }
}

pub fn buffer_desc(size: u64) -> D3D12_RESOURCE_DESC {
    D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Alignment: 0,
        Width: size,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        Format: DXGI_FORMAT_UNKNOWN,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_NONE,
    }
}

pub fn create_buffer(
    device: &ID3D12Device,
    kind: D3D12_HEAP_TYPE,
    size: u64,
    state: D3D12_RESOURCE_STATES,
) -> Result<ID3D12Resource, Error> {
    let mut resource: Option<ID3D12Resource> = None;
    unsafe {
        device.CreateCommittedResource(
            &heap_properties(kind),
            D3D12_HEAP_FLAG_NONE,
            &buffer_desc(size),
            state,
            None,
            &mut resource,
        )?;
    }
    resource.ok_or(Error::NoAdapter)
}

pub fn map_whole(resource: &ID3D12Resource) -> Result<*mut u8, Error> {
    let mut pointer: *mut c_void = std::ptr::null_mut();
    unsafe { resource.Map(0, None, Some(&mut pointer))? };
    Ok(pointer as *mut u8)
}
