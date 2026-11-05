#![forbid(unsafe_code)]

pub mod analysis;
pub mod blur;
pub mod graph;
pub mod linear;
pub mod paint;
pub mod plan;
pub mod slug;
pub mod tone;
pub mod validate;

pub use blur::{BlurDepth, BlurOutput, BlurRequest, blur_chain, depth_for};
pub use graph::{PlanLimits, PlanStats, RenderGraph, ResourceDesc};
pub use linear::Linear2;
pub use paint::{
    BackdropBlur, Glyph, GlyphList, GlyphRun, GradientQuad, Image, PaintCommand, Quad, Shadow,
    SlugRun, plan_paint, plan_paint_with,
};
pub use plan::{
    Barrier, Blend, ColorMatrix, CustomPassSpec, CustomShaderId, Extent, FilterParams, FramePlan,
    GlyphInstance, Gradient, GradientStop, ImageInstance, LoadOp, Pass, PassKind, QuadInstance,
    ResourceState, ShaderId, ShadowCast, ShadowInstance, SpecConstant, TextureFormat,
    TextureHandle, TextureRef, TransientId, TransientTexture,
};
pub use slug::SlugInstance;
pub use tone::ToneCurve;
pub use validate::{PlanError, validate};
