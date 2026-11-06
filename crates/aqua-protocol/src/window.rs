use std::num::NonZeroU64;

use crate::badge::TaskbarBadge;
use crate::geometry::{AquaWindowId, LogicalSize, PhysicalPosition};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct WindowId(pub NonZeroU64);

impl WindowId {
    pub fn new(raw: u64) -> Option<Self> {
        NonZeroU64::new(raw).map(WindowId)
    }

    pub fn raw(self) -> u64 {
        self.0.get()
    }
}

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    pub struct FrameReason: u32 {
        const INPUT      = 0b0000_0001;
        const DAMAGE     = 0b0000_0010;
        const ANIMATION  = 0b0000_0100;
        const MEDIA      = 0b0000_1000;
        const EXPOSE     = 0b0000_0001_0000;
        const RESIZE     = 0b0000_0010_0000;
        const MATERIAL   = 0b0000_0100_0000;
        const EXPLICIT   = 0b0000_1000_0000;
        const GPUI_PULSE = 0b0001_0000_0000;
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct WindowOptions {
    pub title: String,
    pub inner_size: LogicalSize,
    pub min_size: Option<LogicalSize>,
    pub max_size: Option<LogicalSize>,
    pub resizable: bool,
    pub decorations: bool,
    pub transparent: bool,
    pub theme: ThemePreference,
    pub titlebar: Titlebar,
    pub titlebar_height: f64,
    pub minimizable: bool,
    pub maximizable: bool,
    pub closable: bool,
}

impl Default for WindowOptions {
    fn default() -> Self {
        Self {
            title: "Aqua".to_string(),
            inner_size: LogicalSize::new(800.0, 600.0),
            min_size: None,
            max_size: None,
            resizable: true,
            decorations: true,
            transparent: false,
            theme: ThemePreference::Unknown,
            titlebar: Titlebar::Native,
            titlebar_height: 40.0,
            minimizable: true,
            maximizable: true,
            closable: true,
        }
    }
}

impl WindowOptions {
    pub fn validated(&self) -> Result<(), WindowError> {
        for size in std::iter::once(self.inner_size)
            .chain(self.min_size)
            .chain(self.max_size)
        {
            if size.is_empty() || !size.width.is_finite() || !size.height.is_finite() {
                return Err(WindowError::EmptySize);
            }
        }
        if let (Some(min), Some(max)) = (self.min_size, self.max_size) {
            if min.width > max.width || min.height > max.height {
                return Err(WindowError::InvalidSizeConstraints);
            }
        }
        if self.title.contains('\0') {
            return Err(WindowError::InvalidTitle);
        }
        if !self.titlebar_height.is_finite() || self.titlebar_height < 0.0 {
            return Err(WindowError::InvalidChromeHeight);
        }
        Ok(())
    }

    pub fn effective_titlebar(&self) -> Titlebar {
        if !self.decorations {
            Titlebar::Hidden
        } else {
            self.titlebar
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WindowError {
    EmptySize,
    InvalidSizeConstraints,
    InvalidTitle,
    UnknownWindow(AquaWindowId),
    Native(String),
    InvalidChromeHeight,
    Unsupported(&'static str),
}

impl std::fmt::Display for WindowError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, formatter)
    }
}

impl std::error::Error for WindowError {}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowRequest {
    Create(WindowOptions),
    SetTitle(AquaWindowId, String),
    SetInnerSize(AquaWindowId, LogicalSize),
    SetVisible(AquaWindowId, bool),
    RequestRedraw(AquaWindowId),
    SetMinimized(AquaWindowId, bool),
    SetMaximized(AquaWindowId, bool),
    Close(AquaWindowId),
    Activate(AquaWindowId),
    SetBadge(AquaWindowId, Option<TaskbarBadge>),
    SetMaxFrameRate(AquaWindowId, Option<std::num::NonZeroU32>),
    SetVsync(AquaWindowId, bool),
    SetGlideCadence(AquaWindowId, bool),
    SetOuterPosition(AquaWindowId, PhysicalPosition),
    Center(AquaWindowId),
    BeginSnapshotTransition(AquaWindowId, aqua_core::scene::SnapshotTransition),
    BeginGpuiSnapshotTransition(AquaWindowId, aqua_core::scene::GpuiSnapshotTransition),
    BeginGpuiSourceSnapshotTransition(AquaWindowId, aqua_core::scene::GpuiSourceSnapshotTransition),
    SetTextRasterMode(AquaWindowId, aqua_core::scene::TextRasterMode),
}

#[derive(Debug, Clone, PartialEq)]
pub struct ObservedWindowState {
    pub id: AquaWindowId,
    pub title: String,
    pub inner_size: LogicalSize,
    pub scale_factor: f64,
    pub focused: bool,
    pub visible: bool,
    pub occluded: bool,
    pub maximized: bool,
}

impl ObservedWindowState {
    pub fn new(id: AquaWindowId, title: String, inner_size: LogicalSize) -> Self {
        Self {
            id,
            title,
            inner_size,
            scale_factor: 1.0,
            focused: false,
            visible: true,
            occluded: false,
            maximized: false,
        }
    }
}

pub type ThemePreference = crate::shell::ColorScheme;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Titlebar {
    #[default]
    Native,
    Overlay,
    Hidden,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowEvent {
    CloseRequested(AquaWindowId),
    Resized(AquaWindowId, LogicalSize),
    ScaleChanged(AquaWindowId, f64),
    Focused(AquaWindowId, bool),
    Occluded(AquaWindowId, bool),
    ThemeChanged(AquaWindowId, ThemePreference),
    RedrawRequested(AquaWindowId),
    Destroyed(AquaWindowId),
}
