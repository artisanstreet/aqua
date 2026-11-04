#![forbid(unsafe_code)]

pub mod access;
pub mod chrome;
pub mod gpu;
pub mod ime;
pub mod machine;
pub mod material;
pub mod prefs;
pub mod render;
pub mod shell;
pub mod time;
pub mod window;

pub use access::{
    A11yAction, A11yActionData, A11yActionRequest, A11yNode, A11yText, A11yTextRun, AccessAction,
    AccessTreeUpdate,
};
pub use aqua_motion::{MotionCommand, MotionEvent};
pub use chrome::{CaptionControl, CaptionRegion, DragRegions};
pub use gpu::{
    DeviceStatus, FeatureTier, Features, FrameInputs, FrameReport, GpuBackend, GpuCapabilities,
    GpuError, GpuReadback, NativeSurface, PresentHint, PresentTimes, RecoveryReport, SurfaceId,
    SurfacePixels, SurfaceTarget, TextureOp, TextureRegion, TextureUpload, UploadBatch,
};
pub use ime::{ImeBackend, ImeCommand, ImeDocument, ImeEvent, Utf16Range};
pub use machine::{
    first_divergence, replay, Input as RuntimeInput, Machine, Output as RuntimeOutput, Recorded,
    Recorder,
};
pub use render::{
    Feedback, FrameId, GpuEvent, RenderCommand, RenderEvent, RenderInput, RenderOutput, RenderWake,
    SceneDiff,
};
pub use shell::{
    ColorScheme, DialogFilter, DialogKind, DialogOutcome, DialogSpec, FileDropEvent, OsPreferences,
    PixelSize, RequestId, ShellCommand, ShellEvent, TitlebarMode, WindowKind, WindowMaterial,
    WindowOp, WindowSpec,
};
pub use time::Instant;
pub use window::{FrameReason, WindowId};
