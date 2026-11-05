use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use aqua_graph::{CustomPassSpec, CustomShaderId, Extent};
use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Direct3D11::*;

use super::Error;
use super::compile::compile;

pub const MAX_INPUTS: usize = 8;
pub const ROW_BOX_BLUR: CustomShaderId = CustomShaderId(1);

const VERTEX_SOURCE: &str = "struct Output { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
Output vs_custom(uint vertex : SV_VertexID)\n\
{\n\
    float2 corner = float2(vertex & 1, vertex >> 1);\n\
    Output output;\n\
    output.position = float4(corner.x * 2.0 - 1.0, 1.0 - corner.y * 2.0, 0.0, 1.0);\n\
    output.uv = corner;\n\
    return output;\n\
}\n";

fn identity(spec: &CustomPassSpec) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    spec.name.hash(&mut hasher);
    spec.entry.hash(&mut hasher);
    spec.hlsl.hash(&mut hasher);
    hasher.finish()
}

pub struct CustomPipelines {
    device: ID3D11Device,
    vertex: Option<ID3D11VertexShader>,
    built: HashMap<u64, ID3D11PixelShader>,
}

impl CustomPipelines {
    pub fn new(device: &ID3D11Device) -> Self {
        Self {
            device: device.clone(),
            vertex: None,
            built: HashMap::new(),
        }
    }

    pub fn vertex(&mut self) -> Result<ID3D11VertexShader, Error> {
        if let Some(vertex) = &self.vertex {
            return Ok(vertex.clone());
        }
        let bytes = compile(VERTEX_SOURCE, "vs_custom", "vs_5_0")?;
        let mut shader = None;
        unsafe {
            self.device
                .CreateVertexShader(&bytes, None, Some(&mut shader))?
        };
        let shader = shader.ok_or(Error::NoAdapter)?;
        self.vertex = Some(shader.clone());
        Ok(shader)
    }

    pub fn pixel(&mut self, spec: &CustomPassSpec) -> Result<ID3D11PixelShader, Error> {
        let key = identity(spec);
        if let Some(found) = self.built.get(&key) {
            return Ok(found.clone());
        }
        let bytes = compile(&spec.hlsl, &spec.entry, "ps_5_0")?;
        let mut shader = None;
        unsafe {
            self.device
                .CreatePixelShader(&bytes, None, Some(&mut shader))?
        };
        let shader = shader.ok_or(Error::NoAdapter)?;
        self.built.insert(key, shader.clone());
        Ok(shader)
    }
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

pub struct ComputePipelines {
    device: ID3D11Device,
    sources: HashMap<u32, Vec<u8>>,
    built: HashMap<u32, ID3D11ComputeShader>,
}

impl ComputePipelines {
    pub fn new(device: &ID3D11Device) -> Self {
        Self {
            device: device.clone(),
            sources: HashMap::new(),
            built: HashMap::new(),
        }
    }

    pub fn register(&mut self, id: CustomShaderId, hlsl: &str, entry: &str) -> Result<(), Error> {
        let bytes = compile(hlsl, entry, "cs_5_0")?;
        self.sources.insert(id.0, bytes);
        self.built.remove(&id.0);
        Ok(())
    }

    pub fn get(&mut self, id: CustomShaderId) -> Result<ID3D11ComputeShader, Error> {
        if let Some(found) = self.built.get(&id.0) {
            return Ok(found.clone());
        }
        let bytes: &[u8] = match (self.sources.get(&id.0), id == ROW_BOX_BLUR) {
            (Some(bytes), _) => bytes,
            (None, true) => {
                aqua_shaders::find_dxbc("cs_row_box_blur")
                    .ok_or(Error::MissingShader("cs_row_box_blur"))?
                    .bytes
            }
            (None, false) => return Err(Error::Unsupported("unregistered compute shader")),
        };
        let mut shader = None;
        unsafe {
            self.device
                .CreateComputeShader(bytes, None, Some(&mut shader))?
        };
        let shader = shader.ok_or(Error::NoAdapter)?;
        self.built.insert(id.0, shader.clone());
        Ok(shader)
    }
}
