use crate::input::pointer::location;
use crate::model::chrome::{centred_buttons, double_click, DoubleClick};
use crate::view::AquaView;
use aqua_protocol::chrome::{hit_test_chrome, ht, HitInput, RectI};
use aqua_protocol::WindowId;
use aqua_shell::Chrome;
use objc2_app_kit::{NSEvent, NSWindowButton, NSWindowStyleMask};
use objc2_foundation::{ns_string, NSUserDefaults};

const BUTTONS: [NSWindowButton; 3] = [
    NSWindowButton::CloseButton,
    NSWindowButton::MiniaturizeButton,
    NSWindowButton::ZoomButton,
];

pub(crate) fn place_buttons(id: WindowId) {
    let Some((native, spec)) =
        crate::state::window(id, |window| (window.native.clone(), window.spec))
    else {
        return;
    };
    if !spec.overlay() || native.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let Some(close) = native.standardWindowButton(NSWindowButton::CloseButton) else {
        return;
    };
    let titlebar = unsafe { close.superview() };
    let container = titlebar
        .as_ref()
        .and_then(|titlebar| unsafe { titlebar.superview() });
    let (Some(titlebar), Some(container)) = (titlebar, container) else {
        return;
    };
    let button_height = close.frame().size.height;
    let strip = centred_buttons(
        spec.titlebar_height,
        button_height,
        native.frame().size.height,
    );
    let mut frame = container.frame();
    frame.origin.y = strip.container_y;
    frame.size.height = strip.container_height;
    container.setFrame(frame);
    let top = match titlebar.isFlipped() {
        true => strip.container_height - button_height,
        false => 0.0,
    };
    for kind in BUTTONS {
        if let Some(button) = native.standardWindowButton(kind) {
            let mut origin = button.frame().origin;
            origin.y = top;
            button.setFrameOrigin(origin);
        }
    }
}

pub(crate) fn configure_buttons(id: WindowId) {
    let Some((native, spec)) =
        crate::state::window(id, |window| (window.native.clone(), window.spec))
    else {
        return;
    };
    let hidden = spec.chrome == Chrome::Custom && !spec.caption_buttons;
    for kind in BUTTONS {
        if let Some(button) = native.standardWindowButton(kind) {
            button.setHidden(hidden);
        }
    }
    if let Some(zoom) = native.standardWindowButton(NSWindowButton::ZoomButton) {
        zoom.setEnabled(spec.maximizable);
    }
    place_buttons(id);
}

fn user_double_click() -> DoubleClick {
    let defaults = NSUserDefaults::standardUserDefaults();
    let action = defaults
        .stringForKey(ns_string!("AppleActionOnDoubleClick"))
        .map(|action| action.to_string());
    let legacy_key = ns_string!("AppleMiniaturizeOnDoubleClick");
    let legacy = defaults
        .objectForKey(legacy_key)
        .map(|_| defaults.boolForKey(legacy_key));
    double_click(action.as_deref(), legacy)
}

pub(crate) fn caption_press(view: &AquaView, event: &NSEvent) -> bool {
    let Some((native, spec, regions)) = crate::state::window(view.id(), |window| {
        (window.native.clone(), window.spec, window.regions.clone())
    }) else {
        return false;
    };
    if spec.chrome != Chrome::Custom {
        return false;
    }
    let scale = native.backingScaleFactor();
    let at = location(view, event);
    let size = view.bounds().size;
    let rects = aqua_shell::regions::drag_rects(&regions, scale);
    let band = match spec.overlay() {
        true => (spec.titlebar_height * scale).round() as u32,
        false => 0,
    };
    let code = hit_test_chrome(&HitInput {
        point: (
            (f64::from(at.x) * scale).floor() as i32,
            (f64::from(at.y) * scale).floor() as i32,
        ),
        window: RectI {
            left: 0,
            top: 0,
            right: (size.width * scale).round() as i32,
            bottom: (size.height * scale).round() as i32,
        },
        client_origin: (0, 0),
        border: 0,
        buttons: None,
        drag_band: band,
        drag: &rects,
        maximized: false,
        resizable: spec.resizable,
        minimizable: spec.minimizable,
    });
    if code != ht::CAPTION {
        return false;
    }
    match (event.clickCount(), user_double_click()) {
        (2, DoubleClick::Zoom) if spec.maximizable => native.zoom(None),
        (2, DoubleClick::Minimize) if spec.minimizable => native.miniaturize(None),
        (2, _) => {}
        _ => native.performWindowDragWithEvent(event),
    }
    true
}
