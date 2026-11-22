use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_graph::{Extent, FramePlan};
use aqua_protocol::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, UploadBatch,
};
use objc2_metal::MTLTextureUsage;

use super::custom::compiler::{RuntimeCompiler, ShaderCompiler};
use super::formats::SurfaceFormat;
use super::frames::Frames;
use super::persistent::{staging_bytes, PersistentStats, PersistentTextures};
use super::pipeline::{Kind, PipelineStats};
use super::renderer::{SceneOptions, SceneRenderer, SceneStats};
use super::Error;
use crate::hal::budget::{pressure, trim_for, MemoryBudget, Trim, Visibility};
use crate::hal::{Device, DeviceInfo, DeviceOptions, Texture, TextureDesc};
use crate::ledger::Kind as LedgerKind;

pub const HEAP_ALIGNMENT: u64 = 65_536;

#[derive(Clone, Debug, PartialEq)]
pub struct OffscreenOptions {
    pub device: DeviceOptions,
    pub format: SurfaceFormat,
    pub scene: SceneOptions,
    pub pipeline_cache: Option<Vec<u8>>,
    pub compiler: ShaderCompiler,
}

impl Default for OffscreenOptions {
    fn default() -> Self {
        Self {
            device: DeviceOptions::from_env(),
            format: SurfaceFormat::Rgba16Float,
            scene: SceneOptions::default(),
            pipeline_cache: None,
            compiler: ShaderCompiler::from_env(),
        }
    }
}

struct Surface {
    texture: Texture,
    executed: bool,
}

struct Gpu {
    textures: PersistentTextures,
    renderer: SceneRenderer,
    frames: Frames,
    pending: Vec<UploadBatch>,
    device: Arc<Device>,
}

pub struct OffscreenBackend {
    options: OffscreenOptions,
    capabilities: GpuCapabilities,
    surfaces: HashMap<SurfaceId, Surface>,
    compiler: RuntimeCompiler,
    gpu: Gpu,
    next_surface: u32,
    last_scene_stats: SceneStats,
}

pub(crate) fn capabilities(info: &DeviceInfo, custom_passes: bool) -> GpuCapabilities {
    GpuCapabilities {
        backend: "metal".into(),
        adapter: info.name.clone(),
        tier: FeatureTier::Baseline,
        max_texture_extent: info.max_texture_extent,
        heap_alignment: HEAP_ALIGNMENT,
        features: Features {
            compute: true,
            custom_passes,
            color_matrix: true,
            enhanced_barriers: false,
            tearing: false,
            software: false,
            window_surfaces: false,
        },
    }
}

fn create_surface(
    device: &Device,
    extent: Extent,
    format: SurfaceFormat,
) -> Result<Surface, Error> {
    let texture = Texture::new(
        device,
        TextureDesc {
            extent,
            format: format.metal(),
            usage: MTLTextureUsage::RenderTarget | MTLTextureUsage::ShaderRead,
            label: "offscreen_surface",
            kind: LedgerKind::Texture,
            bytes: extent.texels() * format.texel_bytes(),
        },
    )?;
    Ok(Surface {
        texture,
        executed: false,
    })
}

impl Gpu {
    fn new(
        options: &OffscreenOptions,
        cache: Option<&[u8]>,
        compiler: &RuntimeCompiler,
    ) -> Result<Self, Error> {
        let device = Device::create(&options.device)?;
        let mut renderer = SceneRenderer::new(&device, cache, options.scene, compiler.clone())?;
        renderer.warm(options.format.metal())?;
        Ok(Self {
            textures: PersistentTextures::new(&device),
            frames: Frames::new(&device),
            renderer,
            pending: Vec::new(),
            device,
        })
    }
}

impl OffscreenBackend {
    pub fn new(options: OffscreenOptions) -> Result<Self, Error> {
        let compiler = RuntimeCompiler::load(&options.compiler);
        let gpu = Gpu::new(&options, options.pipeline_cache.as_deref(), &compiler)?;
        Ok(Self {
            capabilities: capabilities(gpu.device.info(), gpu.renderer.custom_passes()),
            options,
            compiler,
            surfaces: HashMap::new(),
            gpu,
            next_surface: 0,
            last_scene_stats: SceneStats::default(),
        })
    }

    pub fn with_format(device: DeviceOptions, format: SurfaceFormat) -> Result<Self, Error> {
        Self::new(OffscreenOptions {
            device,
            format,
            ..OffscreenOptions::default()
        })
    }

    pub fn device_info(&self) -> &DeviceInfo {
        self.gpu.device.info()
    }

    pub fn runtime_compiler(&self) -> &RuntimeCompiler {
        &self.compiler
    }

    pub fn register_compute(
        &mut self,
        id: aqua_graph::CustomShaderId,
        hlsl: &str,
        entry: &str,
    ) -> Result<(), GpuError> {
        self.gpu
            .renderer
            .register_compute(id, hlsl, entry)
            .map_err(GpuError::from)
    }

    pub fn validating(&self) -> bool {
        self.gpu.device.validating()
    }

    pub fn metal_object_classes(&self) -> String {
        self.gpu.device.object_classes()
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.gpu.device.validation_errors()
    }

    pub fn inject_device_removal(&mut self) {
        self.gpu.device.inject_loss();
    }

    pub fn device_status(&self) -> DeviceStatus {
        match self.gpu.device.is_lost() {
            true => DeviceStatus::Lost,
            false => DeviceStatus::Healthy,
        }
    }

    pub fn gpu_time(&self) -> Option<Duration> {
        self.gpu.frames.gpu_time()
    }

    pub fn last_scene_stats(&self) -> SceneStats {
        self.last_scene_stats
    }

    pub fn transient_bytes(&self) -> u64 {
        self.gpu.renderer.transient_bytes()
    }

    pub fn transient_textures(&self) -> usize {
        self.gpu.renderer.transient_textures()
    }

    pub fn ring_bytes(&self) -> u64 {
        self.gpu.frames.ring_bytes()
    }

    pub fn persistent_stats(&self) -> PersistentStats {
        self.gpu.textures.stats()
    }

    pub fn pipeline_stats(&self) -> PipelineStats {
        self.gpu.renderer.pipeline_stats()
    }

    pub fn late_pipelines(&self) -> &[Kind] {
        self.gpu.renderer.late_pipelines()
    }

    pub fn finish_startup(&mut self) {
        self.gpu.renderer.close_startup();
    }

    pub fn pipeline_cache(&self) -> Vec<u8> {
        self.gpu.renderer.pipeline_cache()
    }

    pub fn memory_budget(&self) -> MemoryBudget {
        self.gpu.device.memory_budget()
    }

    pub fn trim(&mut self, trim: &Trim) -> u64 {
        let mut freed = self.gpu.renderer.trim(trim);
        if trim.shrink_upload_ring {
            freed += self.gpu.frames.ring_bytes();
            self.gpu.frames.shrink();
        }
        if trim.evict_idle_resources {
            freed += self.gpu.textures.drain_pool();
        }
        freed
    }

    pub fn service_budget(&mut self, visibility: Visibility) -> u64 {
        let budget = self.memory_budget();
        self.trim(&trim_for(pressure(budget), visibility))
    }

    fn run_frame(&mut self, plan: &FramePlan, surface: SurfaceId) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        aqua_graph::validate(plan)?;
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if entry.texture.extent() != plan.extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: entry.texture.extent(),
            });
        }
        let gpu = &mut self.gpu;
        let ring = gpu.renderer.prepare(plan) + staging_bytes(&gpu.pending);
        let mut frame = gpu.frames.begin(ring).map_err(Error::from)?;
        let pending = std::mem::take(&mut gpu.pending);
        let recorded = gpu.textures.apply(&mut frame, &pending).and_then(|_| {
            gpu.renderer
                .record(&mut frame, plan, entry.texture.view(), &gpu.textures)
        });
        frame.submit();
        let stats = recorded?;
        entry.executed = true;
        self.last_scene_stats = stats;
        Ok(FrameReport {
            passes: stats.passes,
            instances: stats.instances,
            cpu_time: started.elapsed(),
            device: DeviceStatus::Healthy,
            barriers: 0,
            transient_bytes: gpu.renderer.transient_bytes(),
            gpu_time: gpu.frames.gpu_time(),
        })
    }
}

impl GpuBackend for OffscreenBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Offscreen(extent) => {
                let surface = create_surface(&self.gpu.device, extent, self.options.format)?;
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(id, surface);
                Ok(id)
            }
            SurfaceTarget::Window(_)
            | SurfaceTarget::Native(_)
            | SurfaceTarget::Wayland(_)
            | SurfaceTarget::Metal(_) => Err(GpuError::Unsupported(
                "window surfaces on the offscreen backend",
            )),
        }
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.surfaces
            .remove(&surface)
            .map(drop)
            .ok_or(GpuError::UnknownSurface(surface))
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        if !batch.textures.is_empty() {
            self.gpu.pending.push(batch);
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        self.run_frame(plan, inputs.surface)
    }

    fn present(
        &mut self,
        surface: SurfaceId,
        _hint: PresentHint,
    ) -> Result<PresentTimes, GpuError> {
        match self.surfaces.contains_key(&surface) {
            true => Ok(PresentTimes::default()),
            false => Err(GpuError::UnknownSurface(surface)),
        }
    }

    fn recover(&mut self) -> Result<RecoveryReport, GpuError> {
        let extents: Vec<(SurfaceId, Extent)> = self
            .surfaces
            .iter()
            .map(|(id, entry)| (*id, entry.texture.extent()))
            .collect();
        self.surfaces.clear();
        self.gpu = Gpu::new(&self.options, None, &self.compiler)?;
        self.capabilities = capabilities(self.gpu.device.info(), self.gpu.renderer.custom_passes());
        for (id, extent) in &extents {
            self.surfaces.insert(
                *id,
                create_surface(&self.gpu.device, *extent, self.options.format)?,
            );
        }
        Ok(RecoveryReport {
            rebuilt_surfaces: extents.len() as u32,
        })
    }
}

impl GpuReadback for OffscreenBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if !entry.executed {
            return Err(GpuError::DeviceFailure(
                "surface has no executed frame".into(),
            ));
        }
        let extent = entry.texture.extent();
        let format = self.options.format;
        let bytes = self
            .gpu
            .frames
            .read_texture(entry.texture.view(), format.texel_bytes())
            .map_err(Error::from)?;
        Ok(SurfacePixels {
            extent,
            rgba8: format.to_rgba8(&bytes, extent.width as usize, extent.height as usize),
        })
    }
}
