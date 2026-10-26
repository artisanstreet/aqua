//! `aqua-ui`: retained UI tree, typed styles, Taffy layout, hit-testing.
//!
//! The tree never touches GPU or windows: layout runs headless through
//! Taffy and scene emission produces renderer-neutral [`DisplayItem`]s
//! (`aqua_core::DisplayItem`).
//!
//! Module owners: `tree` (retained nodes, mount/reconcile, commands),
//! `layout` (measure hook), `style` (typed styles), `events` (hit/focus/
//! scroll/edit dispatch). `overlay`/`motion` belong to the overlays agent
//! and `element`/`binding` to the macro agent; their `pub mod` declarations
//! land here when their files do (core owner adds the two lines on notice).

pub mod binding;
pub mod collection;
pub mod collection_runtime;
pub mod element;
pub mod events;
pub mod layout;
pub mod motion;
pub mod overlay;
pub mod overlay_host;
pub mod style;
pub mod tree;
pub mod virtualization;

pub use layout::{CosmicMeasurer, NaiveMeasurer, TextMeasurer};
pub use tree::{
    hash_key, ActionCallback, ActionEvent, ActionKind, BoxContent, CanvasContent,
    CanvasPaintCallback, CommandSink, FrameStats, KeyHash, LeafCtx, LeafText, MediaContent,
    NodeKind, NodeRecord, NodeSemantics, NodeSpec, ScrollContent, TextBacking, TextContent,
    UiCommand, UiTree, ViewportContent,
};
