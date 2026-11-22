use std::cell::RefCell;
use std::ffi::c_void;
use std::sync::Arc;
use std::time::Duration;

use aqua_core::{ClickSettings, Point};
use aqua_protocol::prefs::{PrefState, PreferenceSource, SystemPrefs, TextScale};
use aqua_protocol::shell::ColorScheme;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2::{define_class, msg_send, sel, MainThreadOnly};
use objc2_app_kit::{
    NSAppearanceNameAqua, NSAppearanceNameDarkAqua, NSEvent, NSWorkspace,
    NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSArray, NSKeyValueObservingOptions, NSNotification, NSObject,
    NSObjectNSKeyValueObserverRegistration, NSString, NSUserDefaults,
};

pub use aqua_protocol::prefs::{prefers_opaque_frame, should_reduce_motion};

pub type Wake = Arc<dyn Fn() + Send + Sync>;

const DOUBLE_CLICK_DISTANCE: f32 = 5.0;

fn state(on: bool) -> PrefState {
    match on {
        true => PrefState::On,
        false => PrefState::Off,
    }
}

fn appearance_theme(mtm: MainThreadMarker) -> ColorScheme {
    let candidates = unsafe { [NSAppearanceNameAqua, NSAppearanceNameDarkAqua] };
    let names = NSArray::from_slice(&candidates);
    let best = crate::app::shared(mtm)
        .effectiveAppearance()
        .bestMatchFromAppearancesWithNames(&names);
    match best {
        Some(name) if &*name == unsafe { NSAppearanceNameDarkAqua } => ColorScheme::Dark,
        Some(_) => ColorScheme::Light,
        None => ColorScheme::Unknown,
    }
}

fn defaults_theme() -> ColorScheme {
    let style =
        NSUserDefaults::standardUserDefaults().stringForKey(ns_string!("AppleInterfaceStyle"));
    match style {
        Some(style) if &*style == ns_string!("Dark") => ColorScheme::Dark,
        _ => ColorScheme::Light,
    }
}

pub fn system(mtm: Option<MainThreadMarker>) -> SystemPrefs {
    let workspace = NSWorkspace::sharedWorkspace();
    SystemPrefs {
        reduce_motion: state(workspace.accessibilityDisplayShouldReduceMotion()),
        reduce_transparency: state(workspace.accessibilityDisplayShouldReduceTransparency()),
        high_contrast: state(workspace.accessibilityDisplayShouldIncreaseContrast()),
        text_scale: TextScale::NORMAL,
        theme: mtm.map_or_else(defaults_theme, appearance_theme),
        source: PreferenceSource::AppKit,
    }
}

pub fn query() -> SystemPrefs {
    system(MainThreadMarker::new()).with_overrides(|name| std::env::var(name).ok())
}

pub fn click_settings() -> ClickSettings {
    let interval = Duration::from_secs_f64(NSEvent::doubleClickInterval().max(0.0));
    ClickSettings::new(
        interval,
        Point::new(DOUBLE_CLICK_DISTANCE, DOUBLE_CLICK_DISTANCE),
    )
    .unwrap_or_default()
}

struct Following {
    wake: Wake,
    seen: SystemPrefs,
}

thread_local! {
    static FOLLOWING: RefCell<Option<Following>> = const { RefCell::new(None) };
}

fn changed(mtm: MainThreadMarker) {
    let wake = FOLLOWING.with_borrow_mut(|following| {
        let following = following
            .as_mut()
            .expect("invariant: the preference observer exists only while changes are followed");
        let now = system(Some(mtm));
        let moved = now != following.seen;
        following.seen = now;
        moved.then(|| following.wake.clone())
    });
    if let Some(wake) = wake {
        wake();
    }
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaPreferenceObserver"]
    struct Observer;

    impl Observer {
        #[unsafe(method(observeValueForKeyPath:ofObject:change:context:))]
        fn observe_value(
            &self,
            _key_path: Option<&NSString>,
            _object: Option<&AnyObject>,
            _change: *mut AnyObject,
            _context: *mut c_void,
        ) {
            changed(MainThreadMarker::from(self));
        }

        #[unsafe(method(displayOptionsChanged:))]
        fn display_options_changed(&self, _notification: &NSNotification) {
            changed(MainThreadMarker::from(self));
        }
    }
);

fn observe(mtm: MainThreadMarker) {
    let observer: Retained<Observer> = unsafe { msg_send![Observer::alloc(mtm), init] };
    let app = crate::app::shared(mtm);
    unsafe {
        app.addObserver_forKeyPath_options_context(
            &observer,
            ns_string!("effectiveAppearance"),
            NSKeyValueObservingOptions::New,
            std::ptr::null_mut(),
        );
        NSWorkspace::sharedWorkspace()
            .notificationCenter()
            .addObserver_selector_name_object(
                &observer,
                sel!(displayOptionsChanged:),
                Some(NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification),
                None,
            );
    }
    let _immortal = Retained::into_raw(observer);
}

pub fn follow_changes(wake: Wake) {
    let mtm = MainThreadMarker::new()
        .expect("invariant: preference changes are followed from the main thread");
    let first = FOLLOWING.with_borrow_mut(|following| {
        let first = following.is_none();
        *following = Some(Following {
            wake,
            seen: system(Some(mtm)),
        });
        first
    });
    if first {
        observe(mtm);
    }
}
