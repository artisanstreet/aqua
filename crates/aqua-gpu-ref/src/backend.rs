use std::collections::HashMap;
use std::time::Instant;

use aqua_graph::{validate, Extent, FramePlan};
use aqua_protocol::{
    FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities, GpuError,
    GpuReadback, PresentHint, PresentTimes, RecoveryReport, SurfaceId, SurfacePixels,
    SurfaceTarget, TextureOp, UploadBatch,
};

use crate::frame::Frame;
use crate::kernels::Kernels;
use crate::raster::{to_rgba8, Layer};
use crate::sprites::{TextureError, Textures};

struct Surface {
    extent: Extent,
    pixels: Option<Layer>,
}

pub struct RefBackend {
    capabilities: GpuCapabilities,
    surfaces: HashMap<SurfaceId, Surface>,
    textures: Textures,
    kernels: Kernels,
    next_surface: u32,
}

impl Default for RefBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl RefBackend {
    pub fn new() -> Self {
        Self {
            capabilities: GpuCapabilities {
                backend: "cpu-reference".into(),
                adapter: "aqua-gpu-ref".into(),
                tier: FeatureTier::Reference,
                max_texture_extent: 16384,
                heap_alignment: 64 * 1024,
                features: Features {
                    compute: true,
                    custom_passes: true,
                    color_matrix: true,
                    enhanced_barriers: false,
                    tearing: false,
                    software: true,
                    window_surfaces: false,
                },
            },
            surfaces: HashMap::new(),
            textures: Textures::default(),
            kernels: Kernels::standard(),
            next_surface: 0,
        }
    }

    pub fn kernels_mut(&mut self) -> &mut Kernels {
        &mut self.kernels
    }

    pub fn textures(&self) -> &Textures {
        &self.textures
    }
}

fn texture_failure(error: TextureError) -> GpuError {
    match error {
        TextureError::UnsupportedFormat => GpuError::Unsupported("persistent texture format"),
        other => GpuError::DeviceFailure(format!("texture upload: {other:?}")),
    }
}

impl GpuBackend for RefBackend {
    type Error = GpuError;

    fn capabilities(&self) -> &GpuCapabilities {
        &self.capabilities
    }

    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, GpuError> {
        match target {
            SurfaceTarget::Offscreen(extent) => {
                let id = SurfaceId(self.next_surface);
                self.next_surface += 1;
                self.surfaces.insert(
                    id,
                    Surface {
                        extent,
                        pixels: None,
                    },
                );
                Ok(id)
            }
            SurfaceTarget::Window(_) | SurfaceTarget::Native(_) => {
                Err(GpuError::Unsupported("window surfaces"))
            }
        }
    }

    fn upload(&mut self, batch: UploadBatch) -> Result<(), GpuError> {
        for upload in batch.textures {
            let handle = upload.handle;
            match upload.op {
                TextureOp::Create { extent, format } => self
                    .textures
                    .create(handle, extent, format)
                    .map_err(texture_failure)?,
                TextureOp::Write { region, data } => self
                    .textures
                    .write(handle, region.x, region.y, region.extent, &data)
                    .map_err(texture_failure)?,
                TextureOp::Release => {
                    self.textures.release(handle);
                }
            }
        }
        Ok(())
    }

    fn execute(&mut self, plan: &FramePlan, inputs: &FrameInputs) -> Result<FrameReport, GpuError> {
        let started = Instant::now();
        validate(plan)?;
        let surface = self
            .surfaces
            .get_mut(&inputs.surface)
            .ok_or(GpuError::UnknownSurface(inputs.surface))?;
        if surface.extent != plan.extent {
            return Err(GpuError::ExtentMismatch {
                plan: plan.extent,
                surface: surface.extent,
            });
        }
        surface.pixels = Some(Frame::new(plan, &self.textures, &self.kernels).run()?);
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
        Ok(RecoveryReport {
            rebuilt_surfaces: 0,
        })
    }

    fn resize_surface(&mut self, surface: SurfaceId, extent: Extent) -> Result<(), GpuError> {
        let entry = self
            .surfaces
            .get_mut(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        entry.extent = extent;
        entry.pixels = None;
        Ok(())
    }

    fn destroy_surface(&mut self, surface: SurfaceId) -> Result<(), GpuError> {
        self.surfaces
            .remove(&surface)
            .map(|_| ())
            .ok_or(GpuError::UnknownSurface(surface))
    }
}

impl GpuReadback for RefBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, GpuError> {
        let entry = self
            .surfaces
            .get(&surface)
            .ok_or(GpuError::UnknownSurface(surface))?;
        let layer = entry
            .pixels
            .as_ref()
            .ok_or_else(|| GpuError::DeviceFailure("surface has no executed frame".into()))?;
        Ok(SurfacePixels {
            extent: entry.extent,
            rgba8: to_rgba8(layer),
        })
    }
}
