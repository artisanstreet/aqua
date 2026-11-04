#![forbid(unsafe_code)]

pub mod analysis;
pub mod blur;
pub mod graph;
pub mod paint;
pub mod plan;
pub mod validate;

pub use blur::{BlurOutput, BlurRequest, blur_chain, levels_for};
pub use graph::{PlanLimits, PlanStats, RenderGraph, ResourceDesc};
pub use paint::{
    BackdropBlur, Glyph, GlyphRun, GradientQuad, Image, PaintCommand, Quad, Shadow, plan_paint,
    plan_paint_with,
};
pub use plan::{
    Barrier, CustomPassSpec, CustomShaderId, Extent, FilterParams, FramePlan, GlyphInstance,
    Gradient, GradientStop, ImageInstance, LoadOp, Pass, PassKind, QuadInstance, ResourceState,
    ShaderId, ShadowInstance, SpecConstant, TextureFormat, TextureHandle, TextureRef, TransientId,
    TransientTexture,
};
pub use validate::{PlanError, validate};
