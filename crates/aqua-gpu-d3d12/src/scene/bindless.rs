use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;

use super::pipeline::{CONSTANT_DWORDS, MATRIX_DWORDS, ROOT_DWORDS};
use super::Error;
use crate::hal::tier::Tier;

pub const INDEX_DWORD: u32 = 14;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Table,
    Heap,
}

impl Binding {
    pub fn of(tier: Tier) -> Self {
        match tier {
            Tier::Baseline => Binding::Table,
            Tier::Bindless => Binding::Heap,
        }
    }

    pub fn shader(self, name: &str) -> String {
        match (self, name.starts_with("ps_")) {
            (Binding::Heap, true) => format!("{name}_bl"),
            _ => name.to_string(),
        }
    }

    pub fn pipeline_name(self, base: &str) -> String {
        match self {
            Binding::Table => base.to_string(),
            Binding::Heap => format!("{base}-bl"),
        }
    }

    pub fn set_constants(
        self,
        list: &ID3D12GraphicsCommandList,
        values: &[f32; 16],
        descriptor: u32,
    ) {
        let mut words = *values;
        if self == Binding::Heap {
            words[INDEX_DWORD as usize] = f32::from_bits(descriptor);
        }
        unsafe {
            list.SetGraphicsRoot32BitConstants(0, CONSTANT_DWORDS, words.as_ptr() as *const _, 0)
        };
    }

    pub fn set_matrix(self, list: &ID3D12GraphicsCommandList, matrix: &[f32; 20]) {
        unsafe {
            list.SetGraphicsRoot32BitConstants(
                0,
                MATRIX_DWORDS,
                matrix.as_ptr() as *const _,
                CONSTANT_DWORDS,
            )
        };
    }

    pub fn bind_table(self, list: &ID3D12GraphicsCommandList, table: D3D12_GPU_DESCRIPTOR_HANDLE) {
        if self == Binding::Table {
            unsafe { list.SetGraphicsRootDescriptorTable(1, table) };
        }
    }

    pub fn select(
        self,
        list: &ID3D12GraphicsCommandList,
        table: D3D12_GPU_DESCRIPTOR_HANDLE,
        descriptor: u32,
    ) {
        match self {
            Binding::Table => unsafe { list.SetGraphicsRootDescriptorTable(1, table) },
            Binding::Heap => unsafe {
                list.SetGraphicsRoot32BitConstant(0, descriptor, INDEX_DWORD)
            },
        }
    }
}

pub fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, Error> {
    let parameters = [D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            Constants: D3D12_ROOT_CONSTANTS {
                ShaderRegister: 0,
                RegisterSpace: 0,
                Num32BitValues: ROOT_DWORDS,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    }];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
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
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
    };
    let desc = D3D12_ROOT_SIGNATURE_DESC {
        NumParameters: parameters.len() as u32,
        pParameters: parameters.as_ptr(),
        NumStaticSamplers: 1,
        pStaticSamplers: &sampler,
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
            | D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED,
    };
    let mut blob: Option<ID3DBlob> = None;
    let mut errors: Option<ID3DBlob> = None;
    let serialized = unsafe {
        D3D12SerializeRootSignature(
            &desc,
            D3D_ROOT_SIGNATURE_VERSION_1,
            &mut blob,
            Some(&mut errors),
        )
    };
    if let Err(error) = serialized {
        let detail = errors
            .map(|blob| unsafe {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    blob.GetBufferPointer() as *const u8,
                    blob.GetBufferSize(),
                ))
                .into_owned()
            })
            .unwrap_or_else(|| error.to_string());
        return Err(Error::RootSignature(detail));
    }
    let blob = blob.ok_or(Error::NoAdapter)?;
    let bytes = unsafe {
        std::slice::from_raw_parts(blob.GetBufferPointer() as *const u8, blob.GetBufferSize())
    };
    Ok(unsafe { device.CreateRootSignature(0, bytes)? })
}
