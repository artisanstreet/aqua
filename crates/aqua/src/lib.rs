pub mod app;
pub mod config;
pub mod error;
pub mod factory;
pub mod runtime;
pub mod ui_app;

pub use app::{App, Context, IntoView, ReactiveView};
pub use aqua_platform::chrome::DragRegions;
pub use config::{
    AquaConfig, Config, ConfigError, Material, ResolvedConfig, ResolvedMaterial, Theme, Titlebar,
    Window,
};
pub use error::Error;
pub use factory::{fr, Column, ColumnBuilder, ColumnWidth, List, Table};
pub use runtime::{AppCtx, Application, Aqua, AquaUserEvent, ExternalFrame};
pub use ui_app::{UiApp, UiAppError};

pub use aqua_protocol::{Input, Instant, Output};
pub use aqua_runtime::Scheduler;

pub use aqua_ui::element::ui;
pub use aqua_ui::style::{auto, pct, px, Style, Typography};
pub use aqua_ui::style::{Border, Length, Paint, Shadow};

pub mod prelude {
    pub use crate::{
        auto, fr, pct, px, ui, App, AppCtx, Application, Aqua, AquaConfig, AquaUserEvent, Column,
        ColumnBuilder, ColumnWidth, Config, Context, DragRegions, Error, ExternalFrame, IntoView,
        List, Material, ReactiveView, ResolvedMaterial, Style, Table, Theme, Titlebar, Typography,
        UiApp, UiAppError, Window,
    };
    pub use aqua_core::{
        DisplayItem, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId, Oklch,
        Point, PointerButton, PointerEvent, Rect, TextRun,
    };
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
        AquaWindowId, FrameReason, ObservedWindowState, WindowEvent, WindowRequest,
    };
    pub use aqua_protocol::{Input, Instant, Output};
    pub use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
    pub use aqua_render::{Scene, SurfaceFacade};
    pub use aqua_runtime::Scheduler;
    pub use aqua_text::{SharedFonts, SharedRuns, TextEditor, TextLayout};
    pub use aqua_ui::element::ui as ui_builders;
}
