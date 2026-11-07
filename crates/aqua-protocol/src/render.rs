use aqua_core::{DisplayItem, FillPatch, NodeId, Size};
use aqua_motion::{MotionCommand, MotionEvent};

use crate::gpu::FrameReport;
use crate::time::Instant;

pub type FrameId = u64;

#[derive(Clone, Debug, Default, PartialEq)]
pub struct SceneDiff {
    pub fills: Vec<FillPatch>,
    pub upserts: Vec<(NodeId, DisplayItem)>,
    pub order: Option<Vec<NodeId>>,
}

impl SceneDiff {
    pub fn is_empty(&self) -> bool {
        self.fills.is_empty() && self.upserts.is_empty() && self.order.is_none()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderCommand {
    Scene(SceneDiff),
    Motion(MotionCommand),
    Viewport { size: Size, scale: f32 },
}

#[derive(Clone, Debug, PartialEq)]
pub enum RenderEvent {
    Presented(FrameId),
    DeviceLost,
    DeviceRecovered,
    Skipped,
    Motion(MotionEvent),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Feedback {
    Presented(FrameId),
    DeviceLost,
    DeviceRecovered,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderInput {
    pub commands: Vec<RenderCommand>,
    pub feedback: Vec<Feedback>,
    pub present_at: Instant,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RenderWake {
    Idle,
    NextFrame,
    At(Instant),
}

#[derive(Clone, Debug, PartialEq)]
pub struct RenderOutput {
    pub events: Vec<RenderEvent>,
    pub frame: Option<FrameId>,
    pub wake: RenderWake,
}

#[derive(Clone, Debug, PartialEq)]
pub enum GpuEvent {
    FrameExecuted { frame: FrameId, report: FrameReport },
    Presented(FrameId),
    DeviceLost,
    DeviceRecovered,
}

impl GpuEvent {
    pub fn feedback(&self) -> Option<Feedback> {
        match self {
            GpuEvent::FrameExecuted { .. } => None,
            GpuEvent::Presented(frame) => Some(Feedback::Presented(*frame)),
            GpuEvent::DeviceLost => Some(Feedback::DeviceLost),
            GpuEvent::DeviceRecovered => Some(Feedback::DeviceRecovered),
        }
    }
}
