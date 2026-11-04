use std::collections::HashMap;
use std::mem::transmute_copy;

use windows::core::{s, Interface, PCSTR, PCWSTR};
use windows::Win32::Graphics::Direct3D::*;
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::bindless::{self, Binding};
use super::Error;

pub const CONSTANT_DWORDS: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Kind {
    Quad,
    Shadow,
    Glyph,
    Image,
    Layer,
    Copy,
    BlurDown,
    BlurUp,
}

struct Shaders {
    vertex: &'static str,
    pixel: &'static str,
    instanced: bool,
    blend: bool,
}

fn shaders(kind: Kind) -> Shaders {
    match kind {
        Kind::Quad => Shaders {
            vertex: "vs_quad",
            pixel: "ps_quad",
            instanced: true,
            blend: true,
        },
        Kind::Shadow => Shaders {
            vertex: "vs_shadow",
            pixel: "ps_shadow",
            instanced: true,
            blend: true,
        },
        Kind::Glyph => Shaders {
            vertex: "vs_glyph",
            pixel: "ps_glyph",
            instanced: true,
            blend: true,
        },
        Kind::Image => Shaders {
            vertex: "vs_image",
            pixel: "ps_image",
            instanced: true,
            blend: true,
        },
        Kind::Layer => Shaders {
            vertex: "vs_fullscreen",
            pixel: "ps_layer",
            instanced: false,
            blend: true,
        },
        Kind::Copy => Shaders {
            vertex: "vs_fullscreen",
            pixel: "ps_copy",
            instanced: false,
            blend: false,
        },
        Kind::BlurDown => Shaders {
            vertex: "vs_fullscreen",
            pixel: "ps_blur_down",
            instanced: false,
            blend: false,
        },
        Kind::BlurUp => Shaders {
            vertex: "vs_fullscreen",
            pixel: "ps_blur_up",
            instanced: false,
            blend: false,
        },
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub compiled: u32,
    pub loaded: u32,
}

impl Kind {
    pub const ALL: [Kind; 8] = [
        Kind::Quad,
        Kind::Shadow,
        Kind::Glyph,
        Kind::Image,
        Kind::Layer,
        Kind::Copy,
        Kind::BlurDown,
        Kind::BlurUp,
    ];
}

pub struct Pipelines {
    device: ID3D12Device,
    library: Option<ID3D12PipelineLibrary>,
    stats: PipelineStats,
    binding: Binding,
    pub root_signature: ID3D12RootSignature,
    instance_layout: Vec<D3D12_INPUT_ELEMENT_DESC>,
    shadow_layout: Vec<D3D12_INPUT_ELEMENT_DESC>,
    built: HashMap<(Kind, i32), ID3D12PipelineState>,
}

fn bytecode(binding: Binding, name: &'static str) -> Result<D3D12_SHADER_BYTECODE, Error> {
    let blob = aqua_shaders::find(&binding.shader(name)).ok_or(Error::MissingShader(name))?;
    Ok(D3D12_SHADER_BYTECODE {
        pShaderBytecode: blob.bytes.as_ptr() as *const _,
        BytecodeLength: blob.bytes.len(),
    })
}

fn root_signature(device: &ID3D12Device) -> Result<ID3D12RootSignature, Error> {
    let range = D3D12_DESCRIPTOR_RANGE {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 1,
        BaseShaderRegister: 0,
        RegisterSpace: 0,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let parameters = [
        D3D12_ROOT_PARAMETER {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: CONSTANT_DWORDS,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
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
        Flags: D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT,
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

fn instance_element(name: PCSTR, index: u32) -> D3D12_INPUT_ELEMENT_DESC {
    D3D12_INPUT_ELEMENT_DESC {
        SemanticName: name,
        SemanticIndex: 0,
        Format: DXGI_FORMAT_R32G32B32A32_FLOAT,
        InputSlot: 0,
        AlignedByteOffset: index * 16,
        InputSlotClass: D3D12_INPUT_CLASSIFICATION_PER_INSTANCE_DATA,
        InstanceDataStepRate: 1,
    }
}

fn blend_state(enabled: bool) -> D3D12_BLEND_DESC {
    let mut blend = D3D12_BLEND_DESC::default();
    blend.RenderTarget[0] = D3D12_RENDER_TARGET_BLEND_DESC {
        BlendEnable: enabled.into(),
        LogicOpEnable: false.into(),
        SrcBlend: D3D12_BLEND_ONE,
        DestBlend: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOp: D3D12_BLEND_OP_ADD,
        SrcBlendAlpha: D3D12_BLEND_ONE,
        DestBlendAlpha: D3D12_BLEND_INV_SRC_ALPHA,
        BlendOpAlpha: D3D12_BLEND_OP_ADD,
        LogicOp: D3D12_LOGIC_OP_NOOP,
        RenderTargetWriteMask: D3D12_COLOR_WRITE_ENABLE_ALL.0 as u8,
    };
    blend
}

impl Pipelines {
    pub fn create(
        device: &ID3D12Device,
        cache: Option<&[u8]>,
        binding: Binding,
    ) -> Result<Self, Error> {
        Ok(Self {
            device: device.clone(),
            library: open_library(device, cache),
            stats: PipelineStats::default(),
            binding,
            root_signature: match binding {
                Binding::Table => root_signature(device)?,
                Binding::Heap => bindless::root_signature(device)?,
            },
            instance_layout: vec![
                instance_element(s!("BOUNDS"), 0),
                instance_element(s!("CLIP"), 1),
                instance_element(s!("SHAPE"), 2),
                instance_element(s!("FILL"), 3),
                instance_element(s!("BORDER"), 4),
                instance_element(s!("GRADIENT"), 5),
                instance_element(s!("STOPA"), 6),
                instance_element(s!("STOPB"), 7),
                instance_element(s!("STOPOFFSETS"), 8),
            ],
            shadow_layout: vec![
                instance_element(s!("BOUNDS"), 0),
                instance_element(s!("CLIP"), 1),
                instance_element(s!("SHAPE"), 2),
                instance_element(s!("OFFSET"), 3),
                instance_element(s!("COLOR"), 4),
            ],
            built: HashMap::new(),
        })
    }

    pub fn get(&mut self, kind: Kind, format: DXGI_FORMAT) -> Result<ID3D12PipelineState, Error> {
        if let Some(found) = self.built.get(&(kind, format.0)) {
            return Ok(found.clone());
        }
        let shaders = shaders(kind);
        let glyph_layout = super::sprites::glyph_layout();
        let image_layout = super::sprites::image_layout();
        let layout: &[D3D12_INPUT_ELEMENT_DESC] = match (shaders.instanced, kind) {
            (false, _) => &[],
            (true, Kind::Glyph) => &glyph_layout,
            (true, Kind::Image) => &image_layout,
            (true, Kind::Shadow) => &self.shadow_layout,
            (true, _) => &self.instance_layout,
        };
        let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
        formats[0] = format;
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: unsafe { transmute_copy(&self.root_signature) },
            VS: bytecode(self.binding, shaders.vertex)?,
            PS: bytecode(self.binding, shaders.pixel)?,
            BlendState: blend_state(shaders.blend),
            SampleMask: u32::MAX,
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC::default(),
            InputLayout: D3D12_INPUT_LAYOUT_DESC {
                pInputElementDescs: layout.as_ptr(),
                NumElements: layout.len() as u32,
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
        let name: Vec<u16> = format!(
            "{}\0",
            self.binding
                .pipeline_name(&format!("{kind:?}-{}", format.0))
        )
        .encode_utf16()
        .collect();
        let loaded: Option<ID3D12PipelineState> = self.library.as_ref().and_then(|library| {
            unsafe { library.LoadGraphicsPipeline(PCWSTR(name.as_ptr()), &desc) }.ok()
        });
        let state: ID3D12PipelineState = match loaded {
            Some(state) => {
                self.stats.loaded += 1;
                state
            }
            None => {
                let state: ID3D12PipelineState =
                    unsafe { self.device.CreateGraphicsPipelineState(&desc)? };
                if let Some(library) = &self.library {
                    let _ = unsafe { library.StorePipeline(PCWSTR(name.as_ptr()), &state) };
                }
                self.stats.compiled += 1;
                state
            }
        };
        self.built.insert((kind, format.0), state.clone());
        Ok(state)
    }

    pub fn warm(&mut self, formats: &[DXGI_FORMAT]) -> Result<(), Error> {
        for format in formats {
            for kind in Kind::ALL {
                self.get(kind, *format)?;
            }
        }
        Ok(())
    }

    pub fn stats(&self) -> PipelineStats {
        self.stats
    }

    pub fn serialize(&self) -> Option<Vec<u8>> {
        let library = self.library.as_ref()?;
        let size = unsafe { library.GetSerializedSize() };
        let mut bytes = vec![0u8; size];
        unsafe { library.Serialize(&mut bytes) }.ok()?;
        Some(bytes)
    }
}

fn open_library(device: &ID3D12Device, cache: Option<&[u8]>) -> Option<ID3D12PipelineLibrary> {
    let device1: ID3D12Device1 = device.cast().ok()?;
    cache
        .filter(|bytes| !bytes.is_empty())
        .and_then(|bytes| {
            unsafe { device1.CreatePipelineLibrary::<ID3D12PipelineLibrary>(bytes) }.ok()
        })
        .or_else(|| unsafe { device1.CreatePipelineLibrary::<ID3D12PipelineLibrary>(&[]) }.ok())
}
