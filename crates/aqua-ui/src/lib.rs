#![forbid(unsafe_code)]
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
pub mod controls;
pub mod element;
pub mod events;
pub mod incremental;
pub mod input;
pub mod layout;
pub mod live;
pub mod motion;
pub mod overlay;
pub mod overlay_host;
pub mod paint;
pub mod popup;
pub mod program;
pub mod region;
pub mod style;
pub mod tree;
pub mod virtualization;

pub use layout::{NaiveMeasurer, ShapedMeasurer, TextMeasurer};
pub use taffy::style::{
    AlignContent, AlignItems, AlignSelf, FlexDirection, FlexWrap, JustifyContent,
};
pub use tree::{
    hash_key, ActionCallback, ActionEvent, ActionKind, ActivationSource, BoxContent, CanvasContent,
    CanvasPaintCallback, CommandSink, FrameStats, KeyHash, LeafCtx, LeafText, MediaContent,
    NodeKind, NodeRecord, NodeSemantics, NodeSpec, ScrollContent, TextBacking, TextContent,
    TreeFootprint, UiCommand, UiTree, ViewportContent,
};
