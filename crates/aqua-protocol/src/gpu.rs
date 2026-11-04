use std::time::Duration;

use aqua_core::geometry::Rect;
use aqua_graph::{Extent, FramePlan, PlanLimits, TextureFormat, TextureHandle};

use crate::window::WindowId;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct SurfaceId(pub u32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FeatureTier {
    Reference,
    Baseline,
    Bindless,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Features {
    pub compute: bool,
    pub custom_passes: bool,
    pub enhanced_barriers: bool,
    pub tearing: bool,
    pub software: bool,
    pub window_surfaces: bool,
}

impl Features {
    pub const REFERENCE: Features = Features {
        compute: true,
        custom_passes: false,
        enhanced_barriers: false,
        tearing: false,
        software: true,
        window_surfaces: false,
    };
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GpuCapabilities {
    pub backend: String,
    pub adapter: String,
    pub tier: FeatureTier,
    pub max_texture_extent: u32,
    pub heap_alignment: u64,
    pub features: Features,
}

impl GpuCapabilities {
    pub fn plan_limits(&self) -> PlanLimits {
        PlanLimits {
            heap_alignment: self.heap_alignment,
            max_texture_extent: self.max_texture_extent,
            bindless: self.tier == FeatureTier::Bindless,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SurfaceTarget {
    Offscreen(Extent),
    Window(WindowId),
    Native(NativeSurface),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct NativeSurface {
    pub window: WindowId,
    pub hwnd: u64,
    pub extent: Extent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TextureRegion {
    pub x: u32,
    pub y: u32,
    pub extent: Extent,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TextureOp {
    Create {
        extent: Extent,
        format: TextureFormat,
    },
    Write {
        region: TextureRegion,
        data: Vec<u8>,
    },
    Release,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TextureUpload {
    pub handle: TextureHandle,
    pub op: TextureOp,
}

impl UploadBatch {
    pub fn upload_bytes(&self) -> u64 {
        self.textures
            .iter()
            .map(|upload| match &upload.op {
                TextureOp::Write { data, .. } => data.len() as u64,
                TextureOp::Create { .. } | TextureOp::Release => 0,
            })
            .sum()
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UploadBatch {
    pub textures: Vec<TextureUpload>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameInputs {
    pub surface: SurfaceId,
}

impl FrameInputs {
    pub const fn new(surface: SurfaceId) -> Self {
        Self { surface }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DeviceStatus {
    Healthy,
    Lost,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameReport {
    pub passes: u32,
    pub instances: u32,
    pub cpu_time: Duration,
    pub device: DeviceStatus,
    pub barriers: u32,
    pub transient_bytes: u64,
    pub gpu_time: Option<Duration>,
}

impl FrameReport {
    pub fn from_plan(plan: &FramePlan, cpu_time: Duration) -> Self {
        Self {
            passes: plan.passes.len() as u32,
            instances: plan.instance_count() as u32,
            cpu_time,
            device: DeviceStatus::Healthy,
            barriers: plan.barrier_count() as u32,
            transient_bytes: plan.heap_bytes,
            gpu_time: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PresentHint {
    Immediate,
    Vsync,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PresentTimes {
    pub queued: Duration,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecoveryReport {
    pub rebuilt_surfaces: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GpuError {
    Unsupported(&'static str),
    UnknownSurface(SurfaceId),
    UnknownTexture(TextureHandle),
    UploadRejected(TextureHandle),
    PlanRejected(aqua_graph::PlanError),
    ExtentMismatch { plan: Extent, surface: Extent },
    DeviceFailure(String),
}

impl std::fmt::Display for GpuError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GpuError::Unsupported(what) => write!(f, "unsupported by this backend: {what}"),
            GpuError::UnknownSurface(id) => write!(f, "unknown surface {}", id.0),
            GpuError::UnknownTexture(handle) => write!(f, "unknown texture {}", handle.0),
            GpuError::UploadRejected(handle) => {
                write!(
                    f,
                    "upload to texture {} is out of bounds or mis-sized",
                    handle.0
                )
            }
            GpuError::PlanRejected(error) => write!(f, "plan rejected: {error}"),
            GpuError::ExtentMismatch { plan, surface } => write!(
                f,
                "plan is {}x{} but the surface is {}x{}",
                plan.width, plan.height, surface.width, surface.height
            ),
            GpuError::DeviceFailure(detail) => write!(f, "device failure: {detail}"),
        }
    }
}

impl std::error::Error for GpuError {}

impl From<aqua_graph::PlanError> for GpuError {
    fn from(error: aqua_graph::PlanError) -> Self {
        GpuError::PlanRejected(error)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SurfacePixels {
    pub extent: Extent,
    pub rgba8: Vec<u8>,
}

pub trait GpuBackend {
    type Error: std::error::Error + From<GpuError>;

    fn capabilities(&self) -> &GpuCapabilities;
    fn create_surface(&mut self, target: SurfaceTarget) -> Result<SurfaceId, Self::Error>;
    fn upload(&mut self, batch: UploadBatch) -> Result<(), Self::Error>;
    fn execute(
        &mut self,
        plan: &FramePlan,
        inputs: &FrameInputs,
    ) -> Result<FrameReport, Self::Error>;
    fn present(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
    ) -> Result<PresentTimes, Self::Error>;
    fn recover(&mut self) -> Result<RecoveryReport, Self::Error>;

    fn resize_surface(&mut self, _surface: SurfaceId, _extent: Extent) -> Result<(), Self::Error> {
        Err(GpuError::Unsupported("surface resize").into())
    }

    fn destroy_surface(&mut self, _surface: SurfaceId) -> Result<(), Self::Error> {
        Err(GpuError::Unsupported("surface destruction").into())
    }

    fn wait_for_frame_slot(&mut self, _surface: SurfaceId) -> Result<(), Self::Error> {
        Ok(())
    }

    fn present_with_damage(
        &mut self,
        surface: SurfaceId,
        hint: PresentHint,
        _damage: &[Rect],
    ) -> Result<PresentTimes, Self::Error> {
        self.present(surface, hint)
    }
}

pub trait GpuReadback: GpuBackend {
    fn read_surface(&mut self, surface: SurfaceId) -> Result<SurfacePixels, Self::Error>;
}
