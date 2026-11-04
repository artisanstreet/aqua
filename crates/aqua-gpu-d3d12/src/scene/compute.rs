use std::collections::HashMap;
use std::mem::transmute_copy;

use aqua_graph::CustomShaderId;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::compile::{compile, serialize_root};
use super::custom::MAX_INPUTS;
use super::Error;

pub const ROW_BOX_BLUR: CustomShaderId = CustomShaderId(1);
pub const PARAMETER_DWORDS: u32 = 4;
pub const DESCRIPTORS: u32 = MAX_INPUTS + 1;

pub struct ComputePipelines {
    device: ID3D12Device,
    root_signature: ID3D12RootSignature,
    sources: HashMap<u32, Vec<u8>>,
    built: HashMap<u32, ID3D12PipelineState>,
}

fn compute_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, Error> {
    let inputs = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: MAX_INPUTS,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let output = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_UAV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let table = |range: &D3D12_DESCRIPTOR_RANGE| D3D12_ROOT_PARAMETER {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                NumDescriptorRanges: 1,
                pDescriptorRanges: range,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: PARAMETER_DWORDS,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        table(&inputs),
        table(&output),
    ];
    serialize_root(
        device,
        &D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: parameters.len() as u32,
            pParameters: parameters.as_ptr(),
            NumStaticSamplers: 0,
            pStaticSamplers: std::ptr::null(),
            Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
        },
    )
}

impl ComputePipelines {
    pub fn create(device: &ID3D12Device) -> Result<Self, Error> {
        let builtin =
            aqua_shaders::find("cs_row_box_blur").ok_or(Error::MissingShader("cs_row_box_blur"))?;
        let mut sources = HashMap::new();
        sources.insert(ROW_BOX_BLUR.0, builtin.bytes.to_vec());
        Ok(Self {
            device: device.clone(),
            root_signature: compute_root_signature(device)?,
            sources,
            built: HashMap::new(),
        })
    }

    pub fn register(&mut self, id: CustomShaderId, hlsl: &str, entry: &str) -> Result<(), Error> {
        let bytes = compile(hlsl, entry, "cs_5_1")?;
        self.sources.insert(id.0, bytes);
        self.built.remove(&id.0);
        Ok(())
    }

    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    pub fn get(&mut self, id: CustomShaderId) -> Result<ID3D12PipelineState, Error> {
        if let Some(found) = self.built.get(&id.0) {
            return Ok(found.clone());
        }
        let bytes = self
            .sources
            .get(&id.0)
            .ok_or(Error::Unsupported("unregistered compute shader"))?;
        let desc = D3D12_COMPUTE_PIPELINE_STATE_DESC {
            pRootSignature: unsafe { transmute_copy(&self.root_signature) },
            CS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr() as *const _,
                BytecodeLength: bytes.len(),
            },
            ..Default::default()
        };
        let state: ID3D12PipelineState = unsafe { self.device.CreateComputePipelineState(&desc)? };
        self.built.insert(id.0, state.clone());
        Ok(state)
    }
}

pub fn uav_desc(format: DXGI_FORMAT) -> D3D12_UNORDERED_ACCESS_VIEW_DESC {
    D3D12_UNORDERED_ACCESS_VIEW_DESC {
        Format: format,
        ViewDimension: D3D12_UAV_DIMENSION_TEXTURE2D,
        Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
            Texture2D: D3D12_TEX2D_UAV {
                MipSlice: 0,
                PlaneSlice: 0,
            },
        },
    }
}

pub fn dispatch(
    list: &ID3D12GraphicsCommandList,
    pipelines: &ComputePipelines,
    state: &ID3D12PipelineState,
    params: &[f32; 4],
    inputs: D3D12_GPU_DESCRIPTOR_HANDLE,
    output: D3D12_GPU_DESCRIPTOR_HANDLE,
    groups: [u32; 3],
) {
    unsafe {
        list.SetComputeRootSignature(pipelines.root_signature());
        list.SetPipelineState(state);
        list.SetComputeRoot32BitConstants(0, PARAMETER_DWORDS, params.as_ptr() as *const _, 0);
        list.SetComputeRootDescriptorTable(1, inputs);
        list.SetComputeRootDescriptorTable(2, output);
        list.Dispatch(groups[0], groups[1], groups[2]);
    }
}
