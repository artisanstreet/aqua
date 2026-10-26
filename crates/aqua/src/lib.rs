//! Aqua public facade.
//!
//! Thin by design. This crate owns application orchestration only: the
//! [`runtime::Aqua`] event loop, [`config::AquaConfig`], the root [`App`]
//! builder, shared [`aqua_text::SharedFonts`] ownership, and the
//! [`prelude`]. Everything else is peer-owned (see table below) and consumed
//! here through published contracts.
//!
//! | Responsibility | Owner |
//! |---|---|
//! | Canonical types (IDs, geometry, color, scene vocabulary, input) | `aqua-core` |
//! | Window translation, scheduler, materials, services | `aqua-platform` |
//! | Reactive graph, scopes, batching, flush boundary | `aqua-reactive` |
//! | Text shaping, editing, shared fonts, run registry | `aqua-text` |
//! | GPU composition, scene store, surface presentation | `aqua-render` |
//! | Layout, components, focus, styling, overlay, motion | `aqua-ui` |
//! | `ui!` authoring macro | `aqua-macros` |
//!
//! No bootstrap renderers, signal shims, text models, focus/overlay systems,
//! or DSL engines live here. Pending peer APIs are consumed from published
//! contracts and fixed at integration; missing pieces are listed in the
//! crate README, never shadowed by duplicates.

pub mod app;
pub mod config;
pub mod error;
pub mod factory;
pub mod runtime;
/// High-level UI host (overlay-agent owned): `UiApp` over `View`/`UiTree`.
/// Re-exported here so the facade path (`aqua::UiApp`) is stable while the
/// owner iterates on the implementation.
pub mod ui_app;

/// Canonical root builder + setup vocabulary (see [`app`]).
/// Authoring builders and the resolve seam source from [`config`].
pub use app::{App, Context, IntoView, ReactiveView};
/// Custom-titlebar drag regions (platform-owned canonical type).
pub use aqua_platform::chrome::DragRegions;
pub use config::{
    AquaConfig, Config, ConfigError, Material, ResolvedConfig, ResolvedMaterial, Theme, Titlebar,
    Window,
};
/// Typed [`App`] pipeline failures.
pub use error::Error;
/// Canonical collection builders + table-track helper (single source:
/// `aqua_ui::collection`, re-exported here for the `aqua::` path).
pub use factory::{fr, Column, ColumnBuilder, ColumnWidth, List, Table};
/// Low-level runtime: prefer the root [`App`] builder for new code.
pub use runtime::{AppCtx, Application, Aqua, AquaUserEvent, ExternalFrame};
pub use ui_app::{UiApp, UiAppError};

/// The `ui!` authoring macro (see `docs/UI_MACRO.md`). It expands to
/// builder calls against `aqua::ui` — the re-export of
/// `aqua_ui::element::ui` — so macro output and direct use agree.
pub use aqua_macros::ui;
/// Builder vocabulary for `ui!` expansion and direct use
/// (`aqua_ui::element::ui`).
pub use aqua_ui::element::ui;
/// Canonical styling vocabulary (single source: `aqua-ui::style`).
/// `Style` is the partial authoring type; `px`/`pct`/`auto` build
/// [`Length`](aqua_ui::style::Length) values. See `docs/STYLING.md`
/// and the normative `docs/STYLE_CONTRACT.md`.
pub use aqua_ui::style::{auto, pct, px, Style, Typography};
/// Lower-level resolved types (compatibility; prefer `Style` for authoring).
pub use aqua_ui::style::{Border, Length, Paint, Shadow};

/// Everyday imports for Aqua applications.
pub mod prelude {
    pub use crate::{
        auto, fr, pct, px, ui, App, AppCtx, Application, Aqua, AquaConfig, AquaUserEvent, Column,
        ColumnBuilder, ColumnWidth, Config, Context, DragRegions, Error, ExternalFrame, IntoView,
        List, Material, ReactiveView, ResolvedMaterial, Style, Table, Theme, Titlebar, Typography,
        UiApp, UiAppError, Window,
    };
    // Builder vocabulary for ui! expansion and direct authoring.
    // NOTE: `ui::{column, row}` are deliberately NOT glob-re-exported here:
    // a bare `column`/`row` in user scope would turn `Some(row)`-style
    // bindings into constant patterns (E0308). Import them qualified
    // (`use aqua::ui::column;`) or use `"column"`/`Axis::Column` instead.
    pub use aqua_ui::element::ui as ui_builders;
    // Canonical vocabulary (single source: aqua-core).
    pub use aqua_core::{
        DisplayItem, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Oklch,
        Point, PointerButton, PointerEvent, Rect, TextRun,
    };
    // Platform services (single source: aqua-platform).
    pub use aqua_platform::events::AquaInputEvent;
    pub use aqua_platform::window::WindowOptions;
    pub use aqua_platform::{
        a11y::{a11y_capability, A11yAction, A11yActionRequest, A11yCapability, A11yNode},
        dialogs::{DialogCapability, DialogRequest, FileFilter},
        material::{MaterialCapability, WindowMaterial},
        prefs::{OsPreferences, PrefState, PreferenceSource},
        services::{Clipboard, ClipboardBackend},
    };
    pub use aqua_platform::{
        AquaWindowId, FrameReason, ObservedWindowState, Scheduler, WindowEvent, WindowRequest,
    };
    // Reactive graph (single source: aqua-reactive).
    pub use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
    // Text (single source: aqua-text).
    pub use aqua_text::{SharedFonts, SharedRuns, TextEditor, TextLayout};
    // Scene store + presentation (single source: aqua-render).
    pub use aqua_render::{Scene, SurfaceFacade};
}
