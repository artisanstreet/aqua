//! `aqua-core`: small platform-neutral contract types shared by every Aqua crate.
//!
//! No winit/wgpu dependency is allowed here. Scene data references GPU-backed
//! resources through IDs and descriptors, never through native handles.

pub mod arena;
pub mod click;
pub mod color;
pub mod cursor;
pub mod event;
pub mod geometry;
pub mod gpui_mesh;
pub mod id;
pub mod image;
pub mod scene;
pub mod semantic;
pub mod vector;

pub use arena::Arena;
pub use click::ClickSettings;
pub use color::{
    ColorProfile, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, GpuiHsla,
    GpuiTextColor, Oklch,
};
pub use cursor::CursorIcon;
pub use event::{
    ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, PointerButton, PointerEvent,
    PointerKind, ScrollDelta,
};
pub use geometry::{CornerRadii, Insets, Point, Rect, Size, Transform2D};
pub use gpui_mesh::{GpuiMesh, GpuiMeshStyle, GpuiMeshVertex};
pub use id::{ImageId, NodeId, RawId, ViewportId};
pub use scene::{
    decode_layer, media_layer, viewport_layer, DisplayItem, LayerKind, TextRun,
    LAYER_MAX_GENERATION, LAYER_TAG_MASK, LAYER_TAG_MEDIA, LAYER_TAG_VIEWPORT,
};
pub use semantic::{CollectionSemantics, NumericRangeSemantics, Role, SemanticNode};

pub use image::ImageRef;
