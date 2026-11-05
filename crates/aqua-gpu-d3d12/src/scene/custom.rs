use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::mem::transmute_copy;

use aqua_graph::{CustomPassSpec, Extent};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::Error;
use super::compile::{compile, linear_clamp_sampler, serialize_root};

pub const MAX_INPUTS: u32 = 8;
pub const PARAMETER_ALIGNMENT: usize = 256;

const VERTEX_SOURCE: &str = "struct Output { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
Output vs_custom(uint vertex : SV_VertexID)\n\
{\n\
    float2 corner = float2(vertex & 1, vertex >> 1);\n\
    Output output;\n\
    output.position = float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);\n\
    output.uv = corner;\n\
    return output;\n\
}\n";

pub struct CustomPipelines {
    device: ID3D12Device,
    root_signature: ID3D12RootSignature,
    vertex: Vec<u8>,
    built: HashMap<(u64, i32), ID3D12PipelineState>,
}

fn identity(spec: &CustomPassSpec) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.hash(&mut hasher);
    spec.entry.hash(&mut hasher);
    spec.hlsl.hash(&mut hasher);
    hasher.finish()
}

fn custom_root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, Error> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: MAX_INPUTS,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_CBV,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Descriptor: D3D12_ROOT_DESCRIPTOR {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE {
                    NumDescriptorRanges: 1,
                    pDescriptorRanges: &range,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        },
    ];
    let sampler = D3D12_STATIC_SAMPLER_DESC {
        ShaderVisibility: D3D12_SHADER_VISIBILITY_PIXEL,
        ..linear_clamp_sampler()
    };
    serialize_root(
        device,
        &D3D12_ROOT_SIGNATURE_DESC {
            NumParameters: parameters.len() as u32,
            pParameters: parameters.as_ptr(),
            NumStaticSamplers: 1,
            pStaticSamplers: &sampler,
            Flags: D3D12_ROOT_SIGNATURE_FLAG_NONE,
        },
    )
}

impl CustomPipelines {
    pub fn create(device: &ID3D12Device) -> Result<Self, Error> {
        Ok(Self {
            device: device.clone(),
            root_signature: custom_root_signature(device)?,
            vertex: compile(VERTEX_SOURCE, "vs_custom", "vs_5_1")?,
            built: HashMap::new(),
        })
    }

    pub fn root_signature(&self) -> &ID3D12RootSignature {
        &self.root_signature
    }

    pub fn get(
        &mut self,
        spec: &CustomPassSpec,
        format: DXGI_FORMAT,
    ) -> Result<ID3D12PipelineState, Error> {
        let key = (identity(spec), format.0);
        if let Some(found) = self.built.get(&key) {
            return Ok(found.clone());
        }
        let pixel = compile(&spec.hlsl, &spec.entry, "ps_5_1")?;
        let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
        formats[0] = format;
        let mut blend = D3D12_BLEND_DESC::default();
        blend.RenderTarget[0].RenderTargetWriteMask = D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8;
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: unsafe { transmute_copy(&self.root_signature) },
            VS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: self.vertex.as_ptr() as *const _,
                BytecodeLength: self.vertex.len(),
            },
            PS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: pixel.as_ptr() as *const _,
                BytecodeLength: pixel.len(),
            },
            BlendState: blend,
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            NumRenderTargets: 1,
            RTVFormats: formats,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let state: ID3D12PipelineState = unsafe { self.device.CreateGraphicsPipelineState(&desc)? };
        self.built.insert(key, state.clone());
        Ok(state)
    }
}

pub fn pack_parameters(bytes: &mut Vec<u8>, params: &[f32]) -> u64 {
    bytes.resize(bytes.len().next_multiple_of(PARAMETER_ALIGNMENT), 0);
    let offset = bytes.len() as u64;
    bytes.extend_from_slice(bytemuck::cast_slice(params));
    bytes.resize(
        bytes
            .len()
            .max(offset as usize + 16)
            .next_multiple_of(PARAMETER_ALIGNMENT),
        0,
    );
    offset
}

pub struct CustomDraw {
    pub rtv: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub extent: Extent,
    pub parameters: u64,
    pub inputs: D3D12_GPU_DESCRIPTOR_HANDLE,
    pub bounds: RECT,
}

pub fn scissor_of(bounds: aqua_core::geometry::Rect, extent: Extent) -> RECT {
    let clamp = |value: f32, limit: u32| (value.max(0.0) as i64).min(limit as i64) as i32;
    RECT {
        left: clamp(bounds.x.floor(), extent.width),
        top: clamp(bounds.y.floor(), extent.height),
        right: clamp((bounds.x + bounds.w).ceil(), extent.width),
        bottom: clamp((bounds.y + bounds.h).ceil(), extent.height),
    }
}

pub fn draw(
    list: &ID3D12GraphicsCommandList,
    pipelines: &CustomPipelines,
    state: &ID3D12PipelineState,
    call: &CustomDraw,
) -> bool {
    let covered = call.bounds.right > call.bounds.left && call.bounds.bottom > call.bounds.top;
    if covered {
        unsafe {
            list.SetGraphicsRootSignature(pipelines.root_signature());
            list.OMSetRenderTargets(1, Some(&call.rtv), false, None);
            list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: call.extent.width as f32,
                Height: call.extent.height as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            list.RSSetScissorRects(&[call.bounds]);
            list.SetPipelineState(state);
            list.SetGraphicsRootConstantBufferView(0, call.parameters);
            list.SetGraphicsRootDescriptorTable(1, call.inputs);
            list.DrawInstanced(4, 1, 0, 0);
        }
    }
    covered
}
