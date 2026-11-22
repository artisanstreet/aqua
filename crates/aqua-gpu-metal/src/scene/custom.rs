pub mod compiler;

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use aqua_graph::{CustomPassSpec, CustomShaderId};
use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLColorWriteMask, MTLCompileOptions, MTLComputePipelineState, MTLDevice, MTLFunction,
    MTLLanguageVersion, MTLLibrary, MTLPixelFormat, MTLRenderPipelineDescriptor, MTLSize,
};

use self::compiler::{RuntimeCompiler, Toolchain};
use super::dxc::Stage;
use super::pipeline::{ComputePipeline, RenderPipeline};
use super::spirv_cross::Msl;
use super::Error;
use crate::hal::{describe, Device, Error as DeviceError};

pub const PARAMETER_BYTES: usize = 256;
pub const CUSTOM_INPUTS: usize = 8;

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

type Function = Retained<ProtocolObject<dyn MTLFunction>>;

struct Kernel {
    function: Function,
    threads: MTLSize,
}

#[derive(Clone)]
pub struct Dispatch {
    pub pipeline: ComputePipeline,
    pub threads: MTLSize,
}

pub struct Custom {
    device: Arc<Device>,
    compiler: RuntimeCompiler,
    vertex: Option<Function>,
    fragments: HashMap<u64, Function>,
    pipelines: HashMap<(u64, usize), RenderPipeline>,
    registered: HashMap<CustomShaderId, Kernel>,
    compute: HashMap<CustomShaderId, Dispatch>,
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

    fn toolchain(&self, what: &'static str) -> Result<Arc<Toolchain>, Error> {
        match &self.compiler {
            RuntimeCompiler::Ready(toolchain) => Ok(Arc::clone(toolchain)),
            RuntimeCompiler::Unavailable(_) => Err(Error::Unsupported(what)),
        }
    }

    fn library(&self, msl: &Msl) -> Result<Function, Error> {
        let options = MTLCompileOptions::new();
        options.setLanguageVersion(MTLLanguageVersion::Version2_4);
        #[allow(deprecated)]
        options.setFastMathEnabled(false);
        let rejected = |reason: String| Error::Msl {
            entry: msl.entry.clone(),
            reason,
        };
        let library: Retained<ProtocolObject<dyn MTLLibrary>> = self
            .device
            .raw
            .newLibraryWithSource_options_error(&NSString::from_str(&msl.source), Some(&options))
            .map_err(|error| rejected(describe(&error)))?;
        library
            .newFunctionWithName(&NSString::from_str(&msl.entry))
            .ok_or_else(|| rejected("the library has no function of that name".to_owned()))
    }

    fn function(&self, source: &str, entry: &str, stage: Stage) -> Result<Function, Error> {
        let toolchain = self.toolchain("runtime shader compilation")?;
        let words = toolchain.compiler.compile(source, entry, stage)?;
        let msl = toolchain.translator.translate(&words)?;
        self.library(&msl)
    }

    fn vertex(&mut self) -> Result<Function, Error> {
        if let Some(vertex) = &self.vertex {
            return Ok(vertex.clone());
        }
        let vertex = self.function(VERTEX_SOURCE, "vs_custom", Stage::Vertex)?;
        self.vertex = Some(vertex.clone());
        Ok(vertex)
    }

    pub fn graphics(
        &mut self,
        spec: &CustomPassSpec,
        format: MTLPixelFormat,
    ) -> Result<RenderPipeline, Error> {
        self.toolchain("custom passes")?;
        let key = identity(spec);
        if let Some(pipeline) = self.pipelines.get(&(key, format.0)) {
            return Ok(pipeline.clone());
        }
        let vertex = self.vertex()?;
        let fragment = match self.fragments.get(&key) {
            Some(fragment) => fragment.clone(),
            None => {
                let fragment = self.function(&spec.hlsl, &spec.entry, Stage::Pixel)?;
                self.fragments.insert(key, fragment.clone());
                fragment
            }
        };
        let descriptor = MTLRenderPipelineDescriptor::new();
        descriptor.setLabel(Some(&NSString::from_str(&format!("custom {}", spec.name))));
        descriptor.setVertexFunction(Some(&vertex));
        descriptor.setFragmentFunction(Some(&fragment));
        let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
        attachment.setPixelFormat(format);
        attachment.setWriteMask(MTLColorWriteMask::All);
        attachment.setBlendingEnabled(false);
        let pipeline = self
            .device
            .raw
            .newRenderPipelineStateWithDescriptor_error(&descriptor)
            .map_err(|error| {
                Error::Device(DeviceError::Pipeline {
                    pipeline: format!("custom pass {} for pixel format {}", spec.name, format.0),
                    reason: describe(&error),
                })
            })?;
        self.pipelines.insert((key, format.0), pipeline.clone());
        Ok(pipeline)
    }

    pub fn register(&mut self, id: CustomShaderId, hlsl: &str, entry: &str) -> Result<(), Error> {
        let toolchain = self.toolchain("registered compute shaders")?;
        let words = toolchain.compiler.compile(hlsl, entry, Stage::Compute)?;
        let kernel = toolchain.translator.translate_kernel(&words)?;
        let [width, height, depth] = kernel.threads.map(|count| count as usize);
        let function = self.library(&kernel.msl)?;
        self.registered.insert(
            id,
            Kernel {
                function,
                threads: MTLSize {
                    width,
                    height,
                    depth,
                },
            },
        );
        self.compute.remove(&id);
        Ok(())
    }

    pub fn registered(&mut self, id: CustomShaderId) -> Result<Option<Dispatch>, Error> {
        if let Some(dispatch) = self.compute.get(&id) {
            return Ok(Some(dispatch.clone()));
        }
        let Some(kernel) = self.registered.get(&id) else {
            return Ok(None);
        };
        let pipeline = self
            .device
            .raw
            .newComputePipelineStateWithFunction_error(&kernel.function)
            .map_err(|error| {
                Error::Device(DeviceError::Pipeline {
                    pipeline: format!("registered compute shader {}", id.0),
                    reason: describe(&error),
                })
            })?;
        let threads = kernel.threads;
        if threads.width * threads.height * threads.depth > pipeline.maxTotalThreadsPerThreadgroup()
        {
            return Err(Error::Unsupported(
                "a compute shader with more threads per group than the device allows",
            ));
        }
        let dispatch = Dispatch { pipeline, threads };
        self.compute.insert(id, dispatch.clone());
        Ok(Some(dispatch))
    }
}
