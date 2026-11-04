use std::collections::HashMap;

use aqua_graph::{Extent, FramePlan};

use super::cases::Case;
use aqua_protocol::{
    FrameInputs, GpuBackend, GpuReadback, SurfaceId, SurfacePixels, SurfaceTarget,
};

#[derive(Debug)]
pub enum RunError<E> {
    Backend(E),
    ExtentMismatch,
}

impl<E: std::fmt::Display> std::fmt::Display for RunError<E> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RunError::Backend(error) => write!(f, "{error}"),
            RunError::ExtentMismatch => write!(f, "readback extent differs from the plan"),
        }
    }
}

pub struct Session<'a, B: GpuBackend + GpuReadback> {
    backend: &'a mut B,
    surfaces: HashMap<(u32, u32), SurfaceId>,
}

impl<'a, B: GpuBackend + GpuReadback> Session<'a, B> {
    pub fn new(backend: &'a mut B) -> Self {
        Self {
            backend,
            surfaces: HashMap::new(),
        }
    }

    pub fn backend(&mut self) -> &mut B {
        self.backend
    }

    pub fn surface(&mut self, extent: Extent) -> Result<SurfaceId, RunError<B::Error>> {
        let key = (extent.width, extent.height);
        match self.surfaces.get(&key) {
            Some(id) => Ok(*id),
            None => {
                let id = self
                    .backend
                    .create_surface(SurfaceTarget::Offscreen(extent))
                    .map_err(RunError::Backend)?;
                self.surfaces.insert(key, id);
                Ok(id)
            }
        }
    }

    pub fn render_case(&mut self, case: &Case) -> Result<SurfacePixels, RunError<B::Error>> {
        self.backend
            .upload(case.uploads.clone())
            .map_err(RunError::Backend)?;
        self.render(&case.plan)
    }

    pub fn render(&mut self, plan: &FramePlan) -> Result<SurfacePixels, RunError<B::Error>> {
        let surface = self.surface(plan.extent)?;
        self.backend
            .execute(plan, &FrameInputs { surface })
            .map_err(RunError::Backend)?;
        let pixels = self
            .backend
            .read_surface(surface)
            .map_err(RunError::Backend)?;
        match pixels.extent == plan.extent {
            true => Ok(pixels),
            false => Err(RunError::ExtentMismatch),
        }
    }
}

pub fn render<B: GpuBackend + GpuReadback>(
    backend: &mut B,
    plan: &FramePlan,
) -> Result<SurfacePixels, RunError<B::Error>> {
    Session::new(backend).render(plan)
}
