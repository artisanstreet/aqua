pub use aqua_core::{
    ColorProfile, GpuiBackground, GpuiGradientColorSpace, GpuiGradientStop, GpuiHsla, GpuiTextColor,
};
pub mod app;
pub mod coalesce;
pub mod config;
pub mod error;
pub mod factory;
pub mod images;
pub mod inbox;
pub mod mailbox;
pub mod mem_probe;
pub mod native;
pub mod native_window;
pub mod paste;
pub mod render;
pub mod runtime;
pub mod ui_app;
pub mod worker;

pub use app::{App, Context, IntoView, ReactiveView};
pub use aqua_protocol::chrome::{CaptionControl, CaptionRegion, DragRegions};
#[cfg(windows)]
pub use aqua_shell_win32::local_time;
pub use aqua_ui::motion::{
    Animate, Easing, Motion, MotionTheme, MotionToken, Presence, SpringParams,
};
pub use config::{
    AquaConfig, Config, ConfigError, Material, ResolvedConfig, ResolvedMaterial, Theme, Titlebar,
    Window,
};
pub use error::Error;
pub use factory::{fr, Column, ColumnBuilder, ColumnWidth, List, MotionKit, Table};
pub use inbox::AquaUserEvent;
pub use runtime::{
    run_headless_externals, AccessSink, AppCtx, Application, Aqua, ExternalFileDrag, ExternalFrame,
    ImeSink,
};
pub use ui_app::{UiApp, UiAppError};

pub use aqua_protocol::{Instant, RuntimeInput, RuntimeOutput};
pub use aqua_runtime::Scheduler;

pub use aqua_ui::element::ui;
pub use aqua_ui::style::{auto, pct, px, Axis, Style, Typography};
pub use aqua_ui::style::{Border, Length, Paint, Shadow, ShadowLayer};
pub use aqua_ui::ActivationSource;

pub mod prelude {
    pub use crate::images::{
        ImageAssetBudget, ImageAssetError, ImageAssets, ImageHandle, ImageStatus,
        ImageSubscription, ManagedUiApp,
    };
    pub use crate::mailbox::{Mailbox, SendError, Sender};
    pub use crate::{
        auto, fr, pct, px, ui, App, AppCtx, Application, Aqua, AquaConfig, AquaUserEvent, Axis,
        CaptionControl, CaptionRegion, Column, ColumnBuilder, ColumnWidth, Config, Context,
        DragRegions, Error, ExternalFrame, IntoView, List, Material, ReactiveView,
        ResolvedMaterial, Style, Table, Theme, Titlebar, Typography, UiApp, UiAppError, Window,
    };
    pub use aqua_core::image::*;
    pub use aqua_core::scene::{
        GpuiSnapshotTransition, GpuiSourceSnapshotTransition, GpuiTextColorMap, SnapshotTransition,
        TextColorMap, TextRasterMode,
    };
    pub use aqua_core::vector::*;
    pub use aqua_core::{
        CursorIcon, DisplayItem, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId,
        Oklch, Point, PointerButton, PointerEvent, Rect, TextRun,
    };
    pub use aqua_protocol::prefs::{PrefState, PreferenceSource, SystemPrefs as OsPreferences};
    pub use aqua_protocol::{
        A11yAction, A11yActionRequest, A11yNode, AquaWindowId, BadgeError, DisplayInfo,
        FrameReason, MaterialCapability, ObservedWindowState, TaskbarBadge, WindowEvent,
        WindowMaterial, WindowOptions, WindowRequest,
    };
    pub use aqua_protocol::{Instant, RuntimeInput, RuntimeOutput};
    pub use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
    pub use aqua_runtime::Scheduler;
    pub use aqua_scene::Scene;
    #[cfg(windows)]
    pub use aqua_shell_win32::notifications::activation::{handle_launch, Activation, Launch};
    #[cfg(windows)]
    pub use aqua_shell_win32::notifications::NotificationCenter;
    pub use aqua_shell_win32::notifications::{
        AppIdentity, Notification, NotificationAction, NotificationError, NotificationResponse,
    };
    pub use aqua_shell_win32::open::{OpenError, OpenTarget};
    pub use aqua_shell_win32::picker::{DialogCapability, DialogRequest, FileFilter};
    pub use aqua_shell_win32::services::{Clipboard, ClipboardBackend};
    pub use aqua_text::document::{
        CopySubstitution, SelectionBinding, SelectionDocument, SelectionDocumentId,
        SelectionSource, SelectionSourceError, SelectionSourceId, SourcePosition, SourceSlice,
    };
    pub use aqua_text::{
        FontFamily, FontSlant, FontStretch, FontWeight, InlineCellError, Selection, SharedFonts,
        SharedRuns, StyledSpan, TextEditor, TextLayout, TextStyle,
    };
    pub use aqua_ui::controls::{Control, ResizeHandle, SelectOption, SliderRange};
    pub use aqua_ui::element::ui as ui_builders;
    pub use aqua_ui::input::{
        CaptureRequest, ClipboardPaste, DragPayload, DragRequest, EditorSnapshot, EventPhase,
        FocusRequest, InputKind, InputResponse, PasteImage, RoutedEvent, RoutedInput, Shortcut,
        TextEdit,
    };
    pub use aqua_ui::popup::{PopupFocus, PopupOptions};
}

pub use images::{
    ImageAssetBudget, ImageAssetError, ImageAssetStats, ImageAssets, ImageHandle, ImageStatus,
    ImageSubscription, ManagedUiApp,
};
