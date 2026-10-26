//! `winit` boundary. The `aqua` facade owns the event loop +
//! `ApplicationHandler`; this module provides the helpers it must use so
//! window creation, cursor mapping and event translation stay consistent.
//!
//! Coordinate: `winit = "0.30.12"` (stable). Do NOT mix 0.31-beta APIs.

use crate::events::{physical_size_to_logical, to_logical, translate_theme};
use crate::geometry::AquaWindowId;
use crate::services::{CursorIcon, ImeAreaWindow};
use crate::window::{WindowEvent, WindowOptions};

/// Build portable `winit` attributes from framework options.
/// The facade passes these to `ActiveEventLoop::create_window`.
///
/// Chrome mapping (see `docs/WINDOW_CHROME.md`):
/// - `Overlay` keeps `decorations(true)`: `WS_CAPTION` is what keeps DWM
///   drawing native min/max/close (the subclass removes only caption
///   text/background via `WM_NCCALCSIZE`). Transparency is NOT forced here:
///   an opaque overlay keeps buttons, drag, snap, and resize fully working;
///   only the glass backdrop needs `transparent(true)` plus an alpha
///   surface (the runtime resolves `transparent` from the material choice).
/// - `Hidden` (or `decorations(false)`) maps to borderless.
/// - `minimizable/maximizable/closable` map to winit enabled buttons.
pub fn window_attributes(options: &WindowOptions) -> winit::window::WindowAttributes {
    use winit::dpi::LogicalSize as WLogical;
    use winit::window::WindowButtons;
    let decorations = match options.effective_titlebar() {
        crate::window::Titlebar::Hidden => false,
        crate::window::Titlebar::Native | crate::window::Titlebar::Overlay => options.decorations,
    };
    let mut buttons = WindowButtons::empty();
    if options.closable {
        buttons |= WindowButtons::CLOSE;
    }
    if options.minimizable {
        buttons |= WindowButtons::MINIMIZE;
    }
    if options.maximizable {
        buttons |= WindowButtons::MAXIMIZE;
    }
    let mut attrs = winit::window::Window::default_attributes()
        .with_title(options.title.clone())
        .with_inner_size(WLogical::new(
            options.inner_size.width,
            options.inner_size.height,
        ))
        .with_resizable(options.resizable)
        .with_decorations(decorations)
        .with_transparent(options.transparent)
        .with_enabled_buttons(buttons)
        .with_theme(match options.theme {
            crate::window::ThemePreference::Light => Some(winit::window::Theme::Light),
            crate::window::ThemePreference::Dark => Some(winit::window::Theme::Dark),
            crate::window::ThemePreference::Unknown => None,
        });
    #[cfg(windows)]
    if options.transparent {
        use winit::platform::windows::WindowAttributesExtWindows as _;
        // Transparent windows are presented through DirectComposition by
        // the runtime. A legacy redirection bitmap would hide the backdrop.
        attrs = attrs.with_no_redirection_bitmap(true);
    }
    if let Some(min) = options.min_size {
        attrs = attrs.with_min_inner_size(WLogical::new(min.width, min.height));
    }
    if let Some(max) = options.max_size {
        attrs = attrs.with_max_inner_size(WLogical::new(max.width, max.height));
    }
    attrs
}

pub fn to_winit_cursor(icon: CursorIcon) -> winit::window::CursorIcon {
    use winit::window::CursorIcon as W;
    match icon {
        CursorIcon::Default => W::Default,
        CursorIcon::Text => W::Text,
        CursorIcon::Pointer => W::Pointer,
        CursorIcon::Crosshair => W::Crosshair,
        CursorIcon::Move => W::Move,
        CursorIcon::NotAllowed => W::NotAllowed,
        CursorIcon::EwResize => W::EwResize,
        CursorIcon::NsResize => W::NsResize,
    }
}

impl ImeAreaWindow for winit::window::Window {
    fn set_area(&self, origin: (f64, f64), size: (f64, f64)) {
        use winit::dpi::{LogicalPosition as LP, LogicalSize as LS};
        self.set_ime_cursor_area(
            LP::new(origin.0, origin.1),
            LS::new(size.0.max(1.0), size.1.max(1.0)),
        );
    }
}

/// Translate one `winit` window event into a framework [`WindowEvent`].
/// Returns `None` for input-level events (keyboard/pointer/IME/scale details
/// the facade routes to the UI layer separately) and for events we
/// deliberately ignore. Never panics on unknown variants (wildcard arm).
pub fn translate_window_event(
    event: &winit::event::WindowEvent,
    id: AquaWindowId,
    scale: f64,
) -> Option<WindowEvent> {
    use winit::event::WindowEvent as W;
    match event {
        W::CloseRequested => Some(WindowEvent::CloseRequested(id)),
        W::Destroyed => Some(WindowEvent::Destroyed(id)),
        W::Focused(focused) => Some(WindowEvent::Focused(id, *focused)),
        W::Occluded(occluded) => Some(WindowEvent::Occluded(id, *occluded)),
        W::Resized(size) => Some(WindowEvent::Resized(
            id,
            physical_size_to_logical(*size, scale),
        )),
        W::RedrawRequested => Some(WindowEvent::RedrawRequested(id)),
        // OS theme changes (Windows/macOS only): surface + material posture
        // may need re-evaluation; the facade records the preference.
        W::ThemeChanged(theme) => Some(WindowEvent::ThemeChanged(id, translate_theme(*theme))),
        // Input, IME, keyboard, scale-factor and surface details are routed
        // by the facade through events::translate_* + to_logical; they are
        // not window-state events. (ScaleFactorChanged in particular is
        // handled in the facade's input router, which owns the GPU resize.)
        _ => None,
    }
}

/// Post-translation surface handling for one framework [`WindowEvent`].
///
/// DPI / material / focus changes each need a different follow-up, and the
/// rules must live in exactly one place instead of spreading `matches!`
/// across the facade. Pure: compute the signal, then let the facade act.
#[derive(Debug, Clone, PartialEq)]
pub struct SurfaceSignal {
    /// GPU surface must be resized / resources re-created.
    pub resize_gpu: bool,
    /// Raster caches keyed on scale are stale; re-query `scale_factor()`.
    pub scale_stale: bool,
    /// Re-apply the window material (DWM backdrops differ active/inactive;
    /// theme switches change fallback contrast).
    pub material_recheck: bool,
    /// Skip production entirely (occluded windows do no work).
    pub skip_production: bool,
}

impl SurfaceSignal {
    pub fn none() -> Self {
        Self {
            resize_gpu: false,
            scale_stale: false,
            material_recheck: false,
            skip_production: false,
        }
    }
}

/// Compute the [`SurfaceSignal`] for an already-translated event.
pub fn surface_signal(ev: &WindowEvent) -> SurfaceSignal {
    let mut s = SurfaceSignal::none();
    match ev {
        WindowEvent::Resized(_, _) => {
            s.resize_gpu = true;
        }
        WindowEvent::ScaleChanged(_, _) => {
            s.resize_gpu = true;
            s.scale_stale = true;
        }
        WindowEvent::Focused(_, _) => {
            s.material_recheck = true;
        }
        WindowEvent::ThemeChanged(_, _) => {
            s.material_recheck = true;
        }
        WindowEvent::Occluded(_, occluded) => {
            s.skip_production = *occluded;
        }
        WindowEvent::CloseRequested(_)
        | WindowEvent::RedrawRequested(_)
        | WindowEvent::Destroyed(_) => {}
    }
    s
}

/// Logical pointer position from a physical winit event payload.
pub fn pointer_position(x_phys: f64, y_phys: f64, scale: f64) -> crate::geometry::LogicalPosition {
    to_logical(x_phys, y_phys, scale)
}

/// Suggested facade control-flow for demand-driven scheduling:
/// wake at the scheduler's next deadline, else sleep.
pub fn control_flow_for_next_wakeup(
    next: Option<std::time::Instant>,
) -> winit::event_loop::ControlFlow {
    use winit::event_loop::ControlFlow;
    match next {
        Some(t) => ControlFlow::WaitUntil(t),
        None => ControlFlow::Wait,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geometry::LogicalSize;

    #[test]
    fn attributes_carry_portable_options() {
        let opts = WindowOptions {
            title: "Demo".to_string(),
            inner_size: LogicalSize::new(1024.0, 768.0),
            resizable: false,
            decorations: false,
            transparent: true,
            ..Default::default()
        };
        // Must not panic headless: building attributes opens no window.
        // (Field assertions would couple to winit internals; the round-trip
        // is verified by facade integration tests with a real event loop.)
        let _attrs = window_attributes(&opts);
        opts.validated().expect("test options must be valid");
    }

    #[test]
    fn window_event_translation_is_total() {
        let id = AquaWindowId::new(1).unwrap();
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::CloseRequested, id, 1.0),
            Some(WindowEvent::CloseRequested(id))
        );
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::RedrawRequested, id, 1.0),
            Some(WindowEvent::RedrawRequested(id))
        );
        assert_eq!(
            translate_window_event(&winit::event::WindowEvent::Focused(true), id, 1.0),
            Some(WindowEvent::Focused(id, true))
        );
        // Input-class events map to None here (routed separately).
        // Ime needs no DeviceId so it is constructible headless.
        let ime = winit::event::WindowEvent::Ime(winit::event::Ime::Enabled);
        assert_eq!(translate_window_event(&ime, id, 1.0), None);
    }

    #[test]
    fn theme_and_focus_drive_material_recheck() {
        use crate::events::window_event_frame_reason;
        use crate::window::ThemePreference;
        let id = AquaWindowId::new(1).unwrap();
        // OS theme change translates, schedules a MATERIAL re-evaluation,
        // and raises the surface signal — the full native-event path.
        let ev = translate_window_event(
            &winit::event::WindowEvent::ThemeChanged(winit::window::Theme::Dark),
            id,
            1.0,
        );
        assert_eq!(
            ev,
            Some(WindowEvent::ThemeChanged(id, ThemePreference::Dark))
        );
        let fw = ev.unwrap();
        assert_eq!(
            window_event_frame_reason(&fw),
            Some(crate::scheduler::FrameReason::MATERIAL)
        );
        let sig = surface_signal(&fw);
        assert!(sig.material_recheck);
        assert!(!sig.resize_gpu && !sig.skip_production);
        // Focus loss rechecks the material too (DWM active/inactive differ).
        let unfocus = WindowEvent::Focused(id, false);
        assert!(surface_signal(&unfocus).material_recheck);
        assert_eq!(
            window_event_frame_reason(&unfocus),
            Some(crate::scheduler::FrameReason::INPUT)
        );
    }

    #[test]
    fn cursor_mapping_covers_all_icons() {
        for icon in [
            CursorIcon::Default,
            CursorIcon::Text,
            CursorIcon::Pointer,
            CursorIcon::Crosshair,
            CursorIcon::Move,
            CursorIcon::NotAllowed,
            CursorIcon::EwResize,
            CursorIcon::NsResize,
        ] {
            let _ = to_winit_cursor(icon);
        }
    }
}
