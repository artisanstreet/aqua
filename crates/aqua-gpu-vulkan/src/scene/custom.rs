use std::collections::HashMap;
use std::ffi::CString;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use aqua_graph::{CustomPassSpec, CustomShaderId};
use ash::vk;

use super::binding::Layouts;
use super::dxc::{CompileError, Compiler, RuntimeCompiler, Stage};
use super::pipeline::{
    compute_pipeline, graphics_pipeline, shader_module, Blending, GraphicsDesc, Layout, Stages,
};
use super::Error;
use crate::hal::{Device, Garbage};

pub const PARAMETER_BYTES: usize = 256;

const VERTEX_SOURCE: &str =
    "struct Output { float4 position : SV_Position; float2 uv : TEXCOORD0; };\n\
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

pub fn parameters(params: &[f32]) -> Vec<f32> {
    let floats = (params.len() * 4).next_multiple_of(16).max(PARAMETER_BYTES) / 4;
    let mut padded = params.to_vec();
    padded.resize(floats, 0.0);
    padded
}

struct Module {
    module: vk::ShaderModule,
    entry: CString,
}

pub struct Custom {
    device: Arc<Device>,
    compiler: RuntimeCompiler,
    vertex: Option<vk::ShaderModule>,
    fragments: HashMap<u64, Module>,
    pipelines: HashMap<(u64, i32), vk::Pipeline>,
    registered: HashMap<CustomShaderId, Module>,
    compute: HashMap<CustomShaderId, vk::Pipeline>,
}

fn entry_name(entry: &str) -> Result<CString, Error> {
    CString::new(entry).map_err(|_| Error::Shader(CompileError::EntryContainsNul))
}

impl Custom {
    pub fn new(device: &Arc<Device>, compiler: RuntimeCompiler) -> Self {
        Self {
            device: Arc::clone(device),
            compiler,
            vertex: None,
            fragments: HashMap::new(),
            pipelines: HashMap::new(),
            registered: HashMap::new(),
            compute: HashMap::new(),
        }
    }

    pub fn available(&self) -> bool {
        self.compiler.ready().is_some()
    }

    fn compiler(&self, what: &'static str) -> Result<&Compiler, Error> {
        self.compiler.ready().ok_or(Error::Unsupported(what))
    }

    fn module(&self, source: &str, entry: &str, stage: Stage) -> Result<Module, Error> {
        let entry = entry_name(entry)?;
        let compiler = self.compiler("runtime shader compilation")?;
        let words = compiler.compile(source, &entry.to_string_lossy(), stage)?;
        Ok(Module {
            module: shader_module(&self.device, &words)?,
            entry,
        })
    }

    fn vertex(&mut self) -> Result<vk::ShaderModule, Error> {
        if let Some(vertex) = self.vertex {
            return Ok(vertex);
        }
        let Module { module, .. } = self.module(VERTEX_SOURCE, "vs_custom", Stage::Vertex)?;
        self.vertex = Some(module);
        Ok(module)
    }

    pub fn graphics(
        &mut self,
        cache: vk::PipelineCache,
        spec: &CustomPassSpec,
        format: vk::Format,
        layouts: &Layouts,
    ) -> Result<vk::Pipeline, Error> {
        self.compiler("custom passes")?;
        let key = identity(spec);
        if let Some(pipeline) = self.pipelines.get(&(key, format.as_raw())) {
            return Ok(*pipeline);
        }
        let vertex = self.vertex()?;
        if !self.fragments.contains_key(&key) {
            let fragment = self.module(&spec.hlsl, &spec.entry, Stage::Pixel)?;
            self.fragments.insert(key, fragment);
        }
        let fragment = &self.fragments[&key];
        let pipeline = graphics_pipeline(
            &self.device,
            cache,
            GraphicsDesc {
                stages: Stages {
                    vertex: (vertex, c"vs_custom"),
                    pixel: (fragment.module, &fragment.entry),
                },
                layout: Layout::None,
                blend: Blending::Opaque,
                pipeline_layout: layouts.custom(),
                format,
            },
        )?;
        self.pipelines.insert((key, format.as_raw()), pipeline);
        Ok(pipeline)
    }

    pub fn register(&mut self, id: CustomShaderId, hlsl: &str, entry: &str) -> Result<(), Error> {
        self.compiler("registered compute shaders")?;
        if !self.device.info().storage_without_format {
            return Err(Error::Unsupported(
                "registered compute shaders on a device without shaderStorageImageWriteWithoutFormat",
            ));
        }
        let module = self.module(hlsl, entry, Stage::Compute)?;
        if let Some(previous) = self.registered.insert(id, module) {
            self.device.retire(Garbage::ShaderModule(previous.module));
        }
        if let Some(previous) = self.compute.remove(&id) {
            self.device.retire(Garbage::Pipeline(previous));
        }
        Ok(())
    }

    pub fn registered(
        &mut self,
        cache: vk::PipelineCache,
        id: CustomShaderId,
        layouts: &Layouts,
    ) -> Result<Option<vk::Pipeline>, Error> {
        if let Some(pipeline) = self.compute.get(&id) {
            return Ok(Some(*pipeline));
        }
        let Some(module) = self.registered.get(&id) else {
            return Ok(None);
        };
        let pipeline = compute_pipeline(
            &self.device,
            cache,
            module.module,
            &module.entry,
            layouts.custom_compute(),
        )?;
        self.compute.insert(id, pipeline);
        Ok(Some(pipeline))
    }
}

impl Drop for Custom {
    fn drop(&mut self) {
        let device = &self.device;
        self.pipelines
            .values()
            .chain(self.compute.values())
            .for_each(|pipeline| device.retire(Garbage::Pipeline(*pipeline)));
        self.fragments
            .values()
            .chain(self.registered.values())
            .map(|module| module.module)
            .chain(self.vertex)
            .for_each(|module| device.retire(Garbage::ShaderModule(module)));
    }
}
