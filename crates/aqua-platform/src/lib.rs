//! `aqua-platform`: Windows-first platform adapter for Aqua.
//!
//! One portable API over `winit 0.30` plus native Windows (DWM) and
//! cross-platform OS-service integrations (clipboard via `arboard`, file
//! dialogs via `rfd`, accessibility via AccessKit).
//!
//! Canonical vocabulary lives in `aqua-core` (`geometry`, `event`,
//! `semantic`); this crate translates native occurrences into it and owns
//! the windowing-boundary remainder (window identity, `f64` DIP sizes,
//! scheduler, materials, OS services). Pure translation + scheduling logic
//! is headless-testable (no display/GPU); `winit` handles appear only at
//! the [`host`] boundary and in constructors that need a live window.
//!
//! # Coordination contract (for facade + core owners)
//! - `winit = "0.30.12"` (stable, NOT 0.31 beta). Facade owns
//!   `ApplicationHandler` + event loop; call
//!   [`host::window_attributes`] to build windows and
//!   [`host::translate_window_event`] to convert events.
//! - Frame pacing: [`scheduler::Scheduler::set_fps_target`] takes
//!   `Option<NonZeroU32>` (`None` = unpaced/demand-only); any nonzero
//!   accepted, interval clamped >= 1us. App-facing guide: `docs/SCHEDULER.md`.
//! - Materials: [`material::WindowMaterial`] + [`material::material_capability`];
//!   non-Windows always reports fallback explicitly. On Windows use
//!   [`adapters`] to apply to a live window. Materials REQUIRE a
//!   transparent window + alpha surface (see [`material::material_needs_transparent_window`]).
//! - New input code uses `events::translate_*_core` (canonical core types
//!   out); the provisional `Aqua*` enums stay for existing facade call sites.
//! - Accessibility: [`a11y::A11yAdapter::attach`] needs an initially
//!   INVISIBLE window; events must be forwarded before app handling.

pub mod a11y;
/// Windows UIA adapter (gated: pulls the OS a11y backend crates).
#[cfg(windows)]
pub mod a11y_winit;
pub mod adapters;
/// Native overlay chrome: pure hit-test + stub guard (common).
pub mod chrome;
/// Windows HWND subclass overlay guard (gated: native window APIs).
#[cfg(windows)]
pub mod chrome_winit;
pub mod dialogs;
pub mod events;
pub mod geometry;
pub mod host;
pub mod material;
pub mod prefs;
pub mod scheduler;
pub mod services;
pub mod window;

pub use geometry::{AquaWindowId, LogicalPosition, LogicalSize, PhysicalSize};
pub use material::{MaterialCapability, WindowMaterial};
pub use scheduler::{FrameReason, Scheduler};
pub use window::{
    ObservedWindowState, ThemePreference, Titlebar, WindowEvent, WindowOptions, WindowRequest,
};
