use aqua_core::{CursorIcon, InputEvent, Size};

use crate::chrome::{CaptionControl, DragRegions};
use crate::time::Instant;
use crate::window::{FrameReason, WindowId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct RequestId(pub u64);

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PixelSize {
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ColorScheme {
    Light,
    Dark,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowMaterial {
    #[default]
    Solid,
    Mica,
    Acrylic,
    Tabbed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum TitlebarMode {
    #[default]
    Native,
    Overlay,
    Hidden,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WindowKind {
    #[default]
    Normal,
    Panel,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WindowSpec {
    pub title: String,
    pub size: Size,
    pub min_size: Option<Size>,
    pub resizable: bool,
    pub kind: WindowKind,
    pub titlebar: TitlebarMode,
    pub material: WindowMaterial,
    pub scheme: ColorScheme,
    pub topmost: bool,
    pub visible: bool,
}

impl Default for WindowSpec {
    fn default() -> Self {
        Self {
            title: String::from("Aqua"),
            size: Size { w: 960.0, h: 640.0 },
            min_size: None,
            resizable: true,
            kind: WindowKind::Normal,
            titlebar: TitlebarMode::Native,
            material: WindowMaterial::Solid,
            scheme: ColorScheme::Unknown,
            topmost: false,
            visible: true,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct OsPreferences {
    pub scheme: ColorScheme,
    pub reduced_motion: bool,
    pub high_contrast: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FileDropEvent {
    Hover { paths: Vec<String> },
    Drop { paths: Vec<String> },
    Cancel,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogKind {
    OpenFile { multiple: bool },
    SaveFile { suggested_name: String },
    PickFolder,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogFilter {
    pub label: String,
    pub extensions: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DialogSpec {
    pub kind: DialogKind,
    pub title: String,
    pub filters: Vec<DialogFilter>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum DialogOutcome {
    Chosen(Vec<String>),
    Dismissed,
    Failed(String),
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShellEvent {
    Input {
        window: WindowId,
        event: InputEvent,
    },
    Resized {
        window: WindowId,
        size: PixelSize,
        scale: f32,
    },
    Frame {
        window: WindowId,
        presentation: Instant,
    },
    Visibility {
        window: WindowId,
        visible: bool,
    },
    Maximized {
        window: WindowId,
        maximized: bool,
    },
    CaptionHover {
        window: WindowId,
        control: Option<CaptionControl>,
    },
    CloseRequested(WindowId),
    Destroyed(WindowId),
    Preferences(OsPreferences),
    Clipboard {
        request: RequestId,
        text: Option<String>,
    },
    FileDrop {
        window: WindowId,
        event: FileDropEvent,
    },
    Dialog {
        request: RequestId,
        outcome: DialogOutcome,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowOp {
    Minimize,
    Maximize,
    Restore,
    Show,
    Hide,
    Focus,
}

#[derive(Clone, Debug, PartialEq)]
pub enum ShellCommand {
    CreateWindow {
        window: WindowId,
        spec: WindowSpec,
    },
    CloseWindow(WindowId),
    SetTitle {
        window: WindowId,
        title: String,
    },
    SetCursor {
        window: WindowId,
        icon: CursorIcon,
    },
    SetDragRegions {
        window: WindowId,
        regions: DragRegions,
    },
    SetMaterial {
        window: WindowId,
        material: WindowMaterial,
    },
    SetScheme {
        window: WindowId,
        scheme: ColorScheme,
    },
    Operate {
        window: WindowId,
        op: WindowOp,
    },
    RequestFrame {
        window: WindowId,
        reason: FrameReason,
    },
    ClipboardWrite {
        text: String,
    },
    ClipboardRead {
        request: RequestId,
    },
    Dialog {
        request: RequestId,
        spec: DialogSpec,
    },
    Quit,
}
