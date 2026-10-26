//! `aqua-render`: shared wgpu 2D renderer for Aqua.
//!
//! Ownership: this crate owns GPU composition — the retained scene store,
//! damage tracking, effect passes, scratch resource pools, the text raster
//! cache, and the surface presentation facade. It does NOT own:
//! - canonical vocabulary (`Oklch`, `Rect`, `NodeId`, `DisplayItem`,
//!   `TextRun`): owned by `aqua-core`, re-exported here (contract §1, §4);
//! - `src/media.rs` / `src/viewport.rs` (media3d agent): feed frames through
//!   [`Renderer::register_external`] and [`CustomPass`], documented below;
//! - text shaping policy (`aqua-text`): the renderer rasterizes through
//!   [`text::TextProvider`] (glyphon 0.12 / cosmic-text 0.19 agreed versions).
//!
//! ## Extension contract for media.rs / viewport.rs owners
//!
//! - Per-frame textures: call [`Renderer::register_external`] each frame,
//!   reference the `layer` id from `DisplayItem::External`. Views must stay
//!   alive through `draw`; the renderer only samples them.
//! - Programmable rendering (Viewport3D, custom filters): implement
//!   [`effects::CustomPass`] and register via [`Renderer::add_custom_pass`].
//!   Backdrop readers receive a real snapshot copy; writers target the base.
//!   Scheduling, damage (`CustomPass::bounds`) and lifetime rules match
//!   built-in passes.
//!
//! ## Partial-update contract (honest)
//!
//! - A clean damage set means NO frame is produced (facade skips present).
//! - A presented frame re-encodes commands but instance uploads are
//!   range-diffed against the retained mirror; diagnostics report
//!   `nodes_total` vs `nodes_updated` and exact upload bytes.
//! - Damage is conservative first: structural/scale/unmeasured changes set
//!   [`damage::DamageSet::full`]. See [`damage`] for the invariant.
//!
//! ## Unsupported (explicit, never silent)
//!
//! See [`effects::EffectCapabilities::unsupported`].

pub mod damage;
pub mod effects;
pub mod media;
pub mod renderer;
pub mod resource;
pub mod scene;
pub mod surface;
pub mod text;
pub mod viewport;

// Canonical vocabulary, re-exported (exactly one scene vocabulary).
pub use aqua_core::geometry::Rect;
pub use aqua_core::id::RawId;
pub use aqua_core::scene::{DisplayItem, TextRun};
pub use aqua_core::{NodeId, Oklch};

pub use damage::{DamageSet, DamageTracker, InvalidationReason};
pub use effects::{BlurParams, CustomPass, EffectCapabilities};
pub use renderer::{FramePlan, RenderError, Renderer};
pub use resource::{RenderDiagnostics, ResourceBudget, TextureDesc, TexturePoolModel};
pub use scene::{inflate, Scene};
pub use surface::SurfaceFacade;
pub use text::{GlyphonProvider, ShapedRun, TextBackend, TextFrame, TextProvider, TextStats};
