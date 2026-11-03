pub use aqua_scene::damage;
pub mod effects;
pub mod media;
pub mod renderer;
pub mod resource;
pub use aqua_scene::scene;
pub mod surface;
pub mod text;
pub mod vector;
pub mod viewport;

pub use aqua_core::geometry::Rect;
pub use aqua_core::id::RawId;
pub use aqua_core::scene::{DisplayItem, TextRun};
pub use aqua_core::{NodeId, Oklch};

pub use aqua_scene::{inflate, DamageSet, DamageTracker, InvalidationReason, Scene};
pub use effects::{BlurParams, CustomPass, EffectCapabilities};
pub use renderer::{FramePlan, RenderError, Renderer};
pub use resource::{RenderDiagnostics, ResourceBudget, TextureDesc, TexturePoolModel};
pub use surface::SurfaceFacade;
pub use text::{GlyphonProvider, ShapedRun, TextBackend, TextFrame, TextProvider, TextStats};

pub use aqua_core::vector::{
    Brush, Drawing, FillRule, GradientStop, LineCap, LineJoin, PaintedPath, PathCommand, Stroke,
    SvgSource, VectorSource,
};
pub use vector::{VectorBudget, VectorError, VectorStats};

pub use aqua_core::image::{ImageFit, ImageFitError, ImagePlacement};
