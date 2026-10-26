//! `aqua-core`: small platform-neutral contract types shared by every Aqua crate.
//!
//! No winit/wgpu dependency is allowed here. Scene data references GPU-backed
//! resources through IDs and descriptors, never through native handles.

pub mod arena;
pub mod color;
pub mod event;
pub mod geometry;
pub mod id;
pub mod scene;
pub mod semantic;

pub use arena::Arena;
pub use color::Oklch;
pub use event::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    PointerKind, ScrollDelta,
};
pub use geometry::{CornerRadii, Insets, Point, Rect, Size, Transform2D};
pub use id::{ImageId, NodeId, RawId, ViewportId};
pub use scene::{
    decode_layer, media_layer, viewport_layer, DisplayItem, LayerKind, TextRun,
    LAYER_MAX_GENERATION, LAYER_TAG_MASK, LAYER_TAG_MEDIA, LAYER_TAG_VIEWPORT,
};
pub use semantic::{CollectionSemantics, Role, SemanticNode};
