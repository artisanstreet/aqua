use std::collections::HashMap;
use std::time::Instant;

use aqua_graph::{Extent, FramePlan, TextureFormat, TextureHandle, validate};
use aqua_protocol::{
    FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError,
    GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, TextureOp, UploadBatch,
};
use aqua_render::{RenderError, Renderer, Scene};

use crate::lower::lower;

pub struct WgpuBackend {
    renderer: Renderer,
    capabilities: GpuCapabilities,
    surfaces: HashMap<SurfaceId, Extent>,
    textures: HashMap<TextureHandle, (Extent, TextureFormat)>,
    next_surface: u32,
    current_extent: Extent,
    last_executed: Option<SurfaceId>,
}

fn device_failure(error: RenderError) -> GpuError {
    GpuError::DeviceFailure(error.to_string())
}

impl WgpuBackend {
    pub fn headless() -> Result<Self, GpuError> {
        let extent = Extent {
            width: 1,
            height: 1,
        };
        let renderer = pollster::block_on(Renderer::headless(extent.width, extent.height))
            .map_err(device_failure)?;
        let capabilities = GpuCapabilities {
            backend: "wgpu".into(),
            adapter: "aqua-render".into(),
            tier: FeatureTier::Baseline,
            max_texture_extent: renderer.device().limits().max_texture_dimension_2d,
            heap_alignment: 256,
            features: Features::default(),
        };
        Ok(Self {
            renderer,
            capabilities,
            surfaces: HashMap::new(),
            textures: HashMap::new(),
            next_surface: 0,
            current_extent: extent,
            last_executed: None,
        })
    }

    pub fn renderer(&self) -> &Renderer {
        &self.renderer
    }
}

pub fn adapter_missing(error: &GpuError) -> bool {
    matches!(error, GpuError::DeviceFailure(detail) if detail.starts_with("no usable GPU adapter"))
}

impl GpuBackend for WgpuBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Offscreen(extent) => {
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(id, extent);
                Ok(id)
            }
            SurfaceTarget::Window(_) | SurfaceTarget::Native(_) => {
                Err(GpuError::Unsupported("window surfaces"))
            }
        }
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        for upload in batch.textures {
            match upload.op {
                TextureOp::Create { extent, format } => {
                    self.textures.insert(upload.handle, (extent, format));
                }
                TextureOp::Write { region, data } => {
                    let (extent, format) = self
                        .textures
                        .get(&upload.handle)
                        .copied()
                        .ok_or(GpuError::UnknownTexture(upload.handle))?;
                    let fits = region.x.saturating_add(region.extent.width) <= extent.width
                        && region.y.saturating_add(region.extent.height) <= extent.height;
                    let sized =
                        data.len() as u64 == region.extent.texels() * format.bytes_per_texel();
                    if !fits || !sized {
                        return Err(GpuError::UploadRejected(upload.handle));
                    }
                }
                TextureOp::Release => {
                    self.textures
                        .remove(&upload.handle)
                        .ok_or(GpuError::UnknownTexture(upload.handle))?;
                }
            }
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        validate(plan)?;
        let extent = *self
            .surfaces
            .get(&inputs.surface)
            .ok_or(GpuError::UnknownSurface(inputs.surface))?;
        if extent != plan.extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: extent,
            });
        }
        if self.current_extent != extent {
            self.renderer
                .set_size(extent.width, extent.height, 1.0)
                .map_err(device_failure)?;
            self.current_extent = extent;
        }
        let mut scene = Scene::new();
        lower(plan)?.into_iter().for_each(|item| {
            scene.insert(item);
        });
        let frame = self.renderer.prepare(&mut scene);
        self.renderer.draw(&frame).map_err(device_failure)?;
        self.last_executed = Some(inputs.surface);
        Ok(FrameReport::from_plan(plan, started.elapsed()))
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
        Err(GpuError::Unsupported("device recovery"))
    }
}

impl GpuReadback for WgpuBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let extent = *self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        if self.last_executed != Some(surface) {
            return Err(GpuError::DeviceFailure(
                "surface was not the last one executed".into(),
            ));
        }
        let rgba8 = self.renderer.readback().map_err(device_failure)?;
        Ok(SurfacePixels { extent, rgba8 })
    }
}
