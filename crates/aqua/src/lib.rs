pub mod app;
pub mod config;
pub mod error;
pub mod factory;
pub mod images;
pub mod mailbox;
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
pub use aqua_ui::style::{auto, pct, px, Axis, Style, Typography};
pub use aqua_ui::style::{Border, Length, Paint, Shadow};

pub mod prelude {
    pub use crate::images::{
        ImageAssetBudget, ImageAssetError, ImageAssets, ImageHandle, ImageStatus,
        ImageSubscription, ManagedUiApp,
    };
    pub use crate::mailbox::{Mailbox, SendError, Sender};
    pub use crate::{
        auto, fr, pct, px, ui, App, AppCtx, Application, Aqua, AquaConfig, AquaUserEvent, Axis,
        Column, ColumnBuilder, ColumnWidth, Config, Context, DragRegions, Error, ExternalFrame,
        IntoView, List, Material, ReactiveView, ResolvedMaterial, Style, Table, Theme, Titlebar,
        Typography, UiApp, UiAppError, Window,
    };
    pub use aqua_core::image::*;
    pub use aqua_core::scene::{SnapshotTransition, TextColorMap, TextRasterMode};
    pub use aqua_core::vector::*;
    pub use aqua_core::{
        CursorIcon, DisplayItem, ImeEvent, InputEvent, Key, KeyEvent, Modifiers, NamedKey, NodeId,
        Oklch, Point, PointerButton, PointerEvent, Rect, TextRun,
    };
    pub use aqua_platform::events::AquaInputEvent;
    pub use aqua_platform::window::WindowOptions;
    pub use aqua_platform::{
        a11y::{a11y_capability, A11yAction, A11yActionRequest, A11yCapability, A11yNode},
        badge::{BadgeError, TaskbarBadge},
        dialogs::{DialogCapability, DialogRequest, FileFilter},
        material::{MaterialCapability, WindowMaterial},
        monitor::DisplayInfo,
        notifications::{
            AppIdentity, Notification, NotificationAction, NotificationCenter, NotificationError,
            NotificationResponse,
        },
        prefs::{OsPreferences, PrefState, PreferenceSource},
        services::{Clipboard, ClipboardBackend},
        shell::{OpenTarget, ShellError},
    };
    pub use aqua_platform::{
        AquaWindowId, FrameReason, ObservedWindowState, WindowEvent, WindowRequest,
    };
    pub use aqua_protocol::{Input, Instant, Output};
    pub use aqua_reactive::{FlushReport, Runtime as ReactiveRuntime};
    pub use aqua_render::media::DecodedImage;
    pub use aqua_render::{Scene, SurfaceFacade};
    pub use aqua_runtime::Scheduler;
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
