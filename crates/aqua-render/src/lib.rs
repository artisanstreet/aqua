pub use aqua_scene::damage;
pub mod effects;
pub mod media;
pub mod renderer;
pub mod resource;
pub use aqua_scene::scene;
pub mod surface;
pub mod text;
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
