use aqua_shell::ShellEvent;
use objc2::rc::Retained;
use objc2::runtime::{ProtocolObject, Sel};
use objc2::{define_class, msg_send, sel, ClassType, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate,
    NSApplicationTerminateReply, NSEvent, NSEventModifierFlags, NSEventType, NSMenu, NSMenuItem,
};
use objc2_foundation::{
    ns_string, MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSPoint,
    NSProcessInfo, NSString,
};
use std::cell::RefCell;

define_class!(
    #[unsafe(super(NSApplication, objc2_app_kit::NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaApplication"]
    pub(crate) struct AquaApplication;

    impl AquaApplication {
        #[unsafe(method(sendEvent:))]
        fn send_event(&self, event: &NSEvent) {
            let command_up = event.r#type() == NSEventType::KeyUp
                && event.modifierFlags().contains(NSEventModifierFlags::Command);
            let target = event.window(MainThreadMarker::from(self)).or_else(|| self.keyWindow());
            match (command_up, target) {
                (true, Some(window)) => window.sendEvent(event),
                _ => unsafe { msg_send![super(self), sendEvent: event] },
            }
        }
    }
);

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaApplicationDelegate"]
    struct AppDelegate;

    unsafe impl NSObjectProtocol for AppDelegate {}

    unsafe impl NSApplicationDelegate for AppDelegate {
        #[unsafe(method(applicationShouldTerminate:))]
        fn should_terminate(&self, _sender: &NSApplication) -> NSApplicationTerminateReply {
            for id in crate::state::window_ids() {
                crate::looping::emit(id, ShellEvent::CloseRequested);
            }
            crate::looping::callback_done();
            NSApplicationTerminateReply::TerminateCancel
        }

        #[unsafe(method(applicationDidFinishLaunching:))]
        #[allow(deprecated)]
        fn did_finish_launching(&self, _notification: &NSNotification) {
            shared(MainThreadMarker::from(self)).activateIgnoringOtherApps(true);
        }

        #[unsafe(method(applicationShouldTerminateAfterLastWindowClosed:))]
        fn after_last_window(&self, _sender: &NSApplication) -> bool {
            false
        }

        #[unsafe(method(applicationDidChangeScreenParameters:))]
        fn screens_changed(&self, _notification: &NSNotification) {
            crate::state::screens_changed();
            crate::looping::callback_done();
        }
    }
);

impl AppDelegate {
    fn new(mtm: MainThreadMarker) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        unsafe { msg_send![super(this), init] }
    }
}

thread_local! {
    static DELEGATE: RefCell<Option<Retained<AppDelegate>>> = const { RefCell::new(None) };
}

fn item(
    mtm: MainThreadMarker,
    title: &NSString,
    action: Option<Sel>,
    key: &NSString,
    modifiers: NSEventModifierFlags,
) -> Retained<NSMenuItem> {
    let item = unsafe {
        NSMenuItem::initWithTitle_action_keyEquivalent(NSMenuItem::alloc(mtm), title, action, key)
    };
    item.setKeyEquivalentModifierMask(modifiers);
    item
}

fn submenu(
    mtm: MainThreadMarker,
    bar: &NSMenu,
    title: &NSString,
    items: &[Retained<NSMenuItem>],
) -> Retained<NSMenu> {
    let menu = NSMenu::initWithTitle(NSMenu::alloc(mtm), title);
    for entry in items {
        menu.addItem(entry);
    }
    let holder = NSMenuItem::new(mtm);
    holder.setSubmenu(Some(&menu));
    bar.addItem(&holder);
    menu
}

fn install_menu(mtm: MainThreadMarker, app: &NSApplication) {
    let name = NSProcessInfo::processInfo().processName();
    let command = NSEventModifierFlags::Command;
    let separator = || NSMenuItem::separatorItem(mtm);
    let bar = NSMenu::new(mtm);
    let about = NSString::from_str(&format!("About {name}"));
    let hide = NSString::from_str(&format!("Hide {name}"));
    let quit = NSString::from_str(&format!("Quit {name}"));
    submenu(
        mtm,
        &bar,
        &name,
        &[
            item(
                mtm,
                &about,
                Some(sel!(orderFrontStandardAboutPanel:)),
                ns_string!(""),
                command,
            ),
            separator(),
            item(mtm, &hide, Some(sel!(hide:)), ns_string!("h"), command),
            item(
                mtm,
                ns_string!("Hide Others"),
                Some(sel!(hideOtherApplications:)),
                ns_string!("h"),
                command | NSEventModifierFlags::Option,
            ),
            item(
                mtm,
                ns_string!("Show All"),
                Some(sel!(unhideAllApplications:)),
                ns_string!(""),
                command,
            ),
            separator(),
            item(mtm, &quit, Some(sel!(terminate:)), ns_string!("q"), command),
        ],
    );
    submenu(
        mtm,
        &bar,
        ns_string!("Edit"),
        &[
            item(
                mtm,
                ns_string!("Undo"),
                Some(sel!(undo:)),
                ns_string!("z"),
                command,
            ),
            item(
                mtm,
                ns_string!("Redo"),
                Some(sel!(redo:)),
                ns_string!("z"),
                command | NSEventModifierFlags::Shift,
            ),
            separator(),
            item(
                mtm,
                ns_string!("Cut"),
                Some(sel!(cut:)),
                ns_string!("x"),
                command,
            ),
            item(
                mtm,
                ns_string!("Copy"),
                Some(sel!(copy:)),
                ns_string!("c"),
                command,
            ),
            item(
                mtm,
                ns_string!("Paste"),
                Some(sel!(paste:)),
                ns_string!("v"),
                command,
            ),
            item(
                mtm,
                ns_string!("Select All"),
                Some(sel!(selectAll:)),
                ns_string!("a"),
                command,
            ),
        ],
    );
    let windows = submenu(
        mtm,
        &bar,
        ns_string!("Window"),
        &[
            item(
                mtm,
                ns_string!("Minimize"),
                Some(sel!(performMiniaturize:)),
                ns_string!("m"),
                command,
            ),
            item(
                mtm,
                ns_string!("Zoom"),
                Some(sel!(performZoom:)),
                ns_string!(""),
                command,
            ),
        ],
    );
    app.setMainMenu(Some(&bar));
    app.setWindowsMenu(Some(&windows));
}

pub(crate) fn shared(_mtm: MainThreadMarker) -> Retained<NSApplication> {
    unsafe { msg_send![AquaApplication::class(), sharedApplication] }
}

pub(crate) fn ensure(mtm: MainThreadMarker) {
    if DELEGATE.with_borrow(Option::is_some) {
        return;
    }
    let app = shared(mtm);
    app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    let delegate = AppDelegate::new(mtm);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    install_menu(mtm, &app);
    DELEGATE.set(Some(delegate));
    crate::looping::install(mtm);
}

pub(crate) fn run(mtm: MainThreadMarker) {
    shared(mtm).run();
}

pub(crate) fn stop(mtm: MainThreadMarker) {
    let app = shared(mtm);
    app.stop(None);
    let wake = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(
            NSEventType::ApplicationDefined,
            NSPoint::new(0.0, 0.0),
            NSEventModifierFlags::empty(),
            0.0,
            0,
            None,
            0,
            0,
            0,
        )
    .expect("invariant: AppKit builds an application-defined event");
    app.postEvent_atStart(&wake, true);
}
