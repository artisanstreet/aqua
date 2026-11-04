#![forbid(unsafe_code)]

pub mod analysis;
pub mod blur;
pub mod graph;
pub mod paint;
pub mod plan;
pub mod tone;
pub mod validate;

pub use blur::{blur_chain, depth_for, BlurDepth, BlurOutput, BlurRequest};
pub use graph::{PlanLimits, PlanStats, RenderGraph, ResourceDesc};
pub use paint::{
    plan_paint, plan_paint_with, BackdropBlur, Glyph, GlyphRun, GradientQuad, Image, PaintCommand,
    Quad, Shadow,
};
pub use plan::{
    Barrier, Blend, ColorMatrix, CustomPassSpec, CustomShaderId, Extent, FilterParams, FramePlan,
    GlyphInstance, Gradient, GradientStop, ImageInstance, LoadOp, Pass, PassKind, QuadInstance,
    ResourceState, ShaderId, ShadowInstance, SpecConstant, TextureFormat, TextureHandle,
    TextureRef, TransientId, TransientTexture,
};
pub use validate::{validate, PlanError};
