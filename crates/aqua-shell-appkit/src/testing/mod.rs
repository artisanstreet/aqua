pub mod capture;
pub mod cases;
pub mod inject;
pub mod pasteboard;
pub mod post;
pub mod screen;
pub mod script;

use crate::window::Window;
use aqua_protocol::WindowId;
use objc2::rc::Retained;
use objc2_app_kit::{NSButton, NSWindowButton, NSWindowDidChangeBackingPropertiesNotification};
use objc2_core_graphics::{
    CGPreflightPostEventAccess, CGPreflightScreenCaptureAccess, CGSessionCopyCurrentDictionary,
};
use objc2_foundation::{MainThreadMarker, NSNotificationCenter, NSPoint, NSRect};
use std::cell::RefCell;
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Facility {
    WindowServer,
    PostEvent,
    ScreenCapture,
}

impl Facility {
    fn flag(self) -> &'static str {
        match self {
            Facility::WindowServer => "AQUA_REQUIRE_APPKIT",
            Facility::PostEvent => "AQUA_REQUIRE_POST_EVENT",
            Facility::ScreenCapture => "AQUA_REQUIRE_SCREEN_CAPTURE",
        }
    }

    fn skip(self) -> &'static str {
        match self {
            Facility::WindowServer => "SKIP-NO-WINDOW-SERVER",
            Facility::PostEvent => "SKIP-NO-POST-EVENT",
            Facility::ScreenCapture => "SKIP-NO-SCREEN-CAPTURE",
        }
    }

    fn present(self) -> bool {
        let session = CGSessionCopyCurrentDictionary().is_some();
        match self {
            Facility::WindowServer => session,
            Facility::PostEvent => session && CGPreflightPostEventAccess(),
            Facility::ScreenCapture => session && CGPreflightScreenCaptureAccess(),
        }
    }
}

pub fn available(facility: Facility) -> bool {
    let present = facility.present();
    let required = std::env::var(facility.flag()).is_ok_and(|value| value == "1");
    match (present, required) {
        (true, _) => true,
        (false, true) => panic!(
            "{}=1 but this process has no {:?}",
            facility.flag(),
            facility
        ),
        (false, false) => {
            println!("{}: this process has no {facility:?}", facility.skip());
            false
        }
    }
}

#[allow(deprecated)]
pub fn activate() {
    let mtm = MainThreadMarker::new().expect("tests drive AppKit from the main thread");
    crate::app::ensure(mtm);
    crate::app::shared(mtm).activateIgnoringOtherApps(true);
}

pub fn is_active() -> bool {
    let mtm = MainThreadMarker::new().expect("tests drive AppKit from the main thread");
    crate::app::shared(mtm).isActive()
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    pub fn centre(self) -> (f64, f64) {
        (self.x + self.width / 2.0, self.y + self.height / 2.0)
    }
}

fn rect_of(rect: NSRect) -> Rect {
    Rect {
        x: rect.origin.x,
        y: rect.origin.y,
        width: rect.size.width,
        height: rect.size.height,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Caption {
    Close,
    Minimize,
    Zoom,
}

fn button(window: Window, which: Caption) -> Option<Retained<NSButton>> {
    let kind = match which {
        Caption::Close => NSWindowButton::CloseButton,
        Caption::Minimize => NSWindowButton::MiniaturizeButton,
        Caption::Zoom => NSWindowButton::ZoomButton,
    };
    crate::state::native(window.id())?.standardWindowButton(kind)
}

pub fn caption_button(window: Window, which: Caption) -> Option<(Rect, bool)> {
    let button = button(window, which)?;
    let view = crate::state::window(window.id(), |state| state.view.clone())?;
    let in_window = button.convertRect_toView(button.bounds(), None);
    let local = view.convertRect_fromView(in_window, None);
    let shown = !button.isHidden() && button.window().is_some();
    Some((rect_of(local), shown))
}

pub fn click_caption_button(window: Window, which: Caption) {
    if let Some(button) = button(window, which) {
        unsafe { button.performClick(None) };
    }
}

pub fn frame(window: Window) -> Option<Rect> {
    crate::state::native(window.id()).map(|native| rect_of(native.frame()))
}

pub fn window_number(window: Window) -> Option<isize> {
    crate::state::native(window.id()).map(|native| native.windowNumber())
}

pub fn screen_point(window: Window, (x, y): (f64, f64)) -> Option<(f64, f64)> {
    let mtm = MainThreadMarker::new()?;
    let (native, view) = crate::state::window(window.id(), |state| {
        (state.native.clone(), state.view.clone())
    })?;
    let in_window = view.convertPoint_toView(NSPoint::new(x, y), None);
    let on_screen = native.convertPointToScreen(in_window);
    Some((
        on_screen.x,
        crate::output::primary_height(mtm) - on_screen.y,
    ))
}

pub fn set_frame_size(window: Window, width: f64, height: f64) {
    if let Some(native) = crate::state::native(window.id()) {
        let mut frame = native.frame();
        frame.size.width = width;
        frame.size.height = height;
        native.setFrame_display(frame, true);
    }
}

pub fn exposed(window: Window) -> bool {
    crate::window::occlusion(window.id()) == crate::model::presence::Occlusion::Exposed
}

pub fn set_shadow(window: Window, shadow: bool) {
    if let Some(native) = crate::state::native(window.id()) {
        native.setHasShadow(shadow);
    }
}

pub fn current_cursor() -> Option<aqua_core::CursorIcon> {
    crate::cursor::current()
}

pub fn track_events_for(duration: std::time::Duration) {
    let mtm = MainThreadMarker::new().expect("tests drive AppKit from the main thread");
    let app = crate::app::shared(mtm);
    let started = std::time::Instant::now();
    while let Some(left) = duration.checked_sub(started.elapsed()) {
        let until = objc2_foundation::NSDate::dateWithTimeIntervalSinceNow(left.as_secs_f64());
        let event = app.nextEventMatchingMask_untilDate_inMode_dequeue(
            objc2_app_kit::NSEventMask::Any,
            Some(&until),
            unsafe { objc2_app_kit::NSEventTrackingRunLoopMode },
            true,
        );
        if let Some(event) = event {
            app.sendEvent(&event);
        }
    }
}

thread_local! {
    static SCALES: RefCell<BTreeMap<WindowId, f64>> = const { RefCell::new(BTreeMap::new()) };
}

pub(crate) fn scale_override(id: WindowId) -> Option<f64> {
    SCALES.with_borrow(|scales| scales.get(&id).copied())
}

pub fn report_backing_scale(window: Window, scale: Option<f64>) {
    SCALES.with_borrow_mut(|scales| match scale {
        Some(scale) => scales.insert(window.id(), scale),
        None => scales.remove(&window.id()),
    });
    if let Some(native) = crate::state::native(window.id()) {
        unsafe {
            NSNotificationCenter::defaultCenter().postNotificationName_object(
                NSWindowDidChangeBackingPropertiesNotification,
                Some(&native),
            )
        };
    }
}

pub fn contents_scale(window: Window) -> Option<f64> {
    crate::state::window(window.id(), |state| state.layer.contentsScale())
}
