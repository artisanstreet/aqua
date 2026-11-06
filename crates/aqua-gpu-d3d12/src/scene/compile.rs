use std::ffi::CString;

use windows::core::{s, PCSTR};
use windows::Win32::Graphics::Direct3D::Fxc::{D3DCompile, D3DCOMPILE_OPTIMIZATION_LEVEL3};
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D::ID3DInclude;
use windows::Win32::Graphics::Direct3D12::*;

use super::Error;

pub fn blob_bytes(blob: &ID3DBlob) -> Vec<u8> {
    unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    }
    .to_vec()
}

pub fn compile(source: &str, entry: &str, target: &str) -> Result<Vec<u8>, Error> {
    let entry_name = CString::new(entry).map_err(|_| Error::Shader("entry contains NUL".into()))?;
    let target_name =
        CString::new(target).map_err(|_| Error::Shader("target contains NUL".into()))?;
    let mut code: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let result = unsafe {
        D3DCompile(
            source.as_ptr() as *const _,
            source.len(),
            s!("aqua-custom"),
            None,
            None::<&ID3DInclude>,
            PCSTR(entry_name.as_ptr() as *const u8),
            PCSTR(target_name.as_ptr() as *const u8),
            D3DCOMPILE_OPTIMIZATION_LEVEL3,
            0,
            &mut code,
            Some(&mut errors),
        )
    };
    match (result, code) {
        (Ok(()), Some(code)) => Ok(blob_bytes(&code)),
        (Err(error), _) => Err(Error::Shader(
            errors
                .map(|blob| String::from_utf8_lossy(&blob_bytes(&blob)).into_owned())
                .unwrap_or_else(|| error.to_string()),
        )),
        (Ok(()), None) => Err(Error::Shader("compiler returned no code".into())),
    }
}

pub fn serialize_root(
    device: &ID3D12Device,
    desc: &D3D12_ROOT_SIGNATURE_DESC,
) -> Result<ID3D12RootSignature, Error> {
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let serialized = unsafe {
        D3D12SerializeRootSignature(
            desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = serialized {
        return Err(Error::RootSignature(
            errors
                .map(|blob| String::from_utf8_lossy(&blob_bytes(&blob)).into_owned())
                .unwrap_or_else(|| error.to_string()),
        ));
    }
    let blob = blob.ok_or(Error::NoAdapter)?;
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    };
    Ok(unsafe { device.CreateRootSignature(0, bytes)? })
}

pub fn linear_clamp_sampler() -> D3D12_STATIC_SAMPLER_DESC {
    D3D12_STATIC_SAMPLER_DESC {
        Filter: D3D12_FILTER_MIN_MAG_MIP_LINEAR,
        AddressU: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressV: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        AddressW: D3D12_TEXTURE_ADDRESS_MODE_CLAMP,
        MipLODBias: 0.0,
        MaxAnisotropy: 1,
        ComparisonFunc: D3D12_COMPARISON_FUNC_NEVER,
        BorderColor: D3D12_STATIC_BORDER_COLOR_TRANSPARENT_BLACK,
        MinLOD: 0.0,
        MaxLOD: D3D12_FLOAT32_MAX,
        ShaderRegister: 0,
        RegisterSpace: 0,
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }
}
