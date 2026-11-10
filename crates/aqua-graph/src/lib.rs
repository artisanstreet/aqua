#![forbid(unsafe_code)]

pub mod analysis;
pub mod blur;
pub mod curve_paint;
pub mod graph;
pub mod linear;
pub mod paint;
pub mod plan;
pub mod slug;
pub mod tone;
pub mod validate;

pub use blur::{blur_chain, depth_for, BlurDepth, BlurOutput, BlurRequest};
pub use graph::{PlanLimits, PlanStats, RenderGraph, ResourceDesc};
pub use linear::Linear2;
pub use paint::{
    plan_paint, plan_paint_with, BackdropBlur, Glyph, GlyphList, GlyphRun, GradientQuad, Image,
    PaintCommand, Quad, Shadow, SlugList, SlugRun,
};
pub use plan::{
    Barrier, Blend, ColorMatrix, Coverage, CustomPassSpec, CustomShaderId, Extent, FilterParams,
    FramePlan, GlyphInstance, Gradient, GradientStop, ImageInstance, LoadOp, Pass, PassKind,
    QuadInstance, ResourceState, ShaderId, ShadowCast, ShadowInstance, SolidColor, SolidInstance,
    SpecConstant, TextureFormat, TextureHandle, TextureRef, TransientId, TransientTexture,
};
pub use slug::SlugInstance;
pub use tone::ToneCurve;
pub use validate::{validate, PlanError};
