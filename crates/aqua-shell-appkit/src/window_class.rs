use crate::looping::{callback_done, emit};
use crate::model::presence::{Change, Occlusion};
use crate::wake_trace::{self, Source};
use aqua_protocol::WindowId;
use aqua_shell::ShellEvent;
use objc2::rc::Retained;
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSResponder, NSWindow, NSWindowDelegate, NSWindowOcclusionState,
    NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSNotification, NSObject, NSObjectProtocol, NSRect};

define_class!(
    #[unsafe(super(NSWindow, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaWindow"]
    pub(crate) struct AquaWindow;

    impl AquaWindow {
        #[unsafe(method(canBecomeKeyWindow))]
        fn can_become_key(&self) -> bool {
            true
        }

        #[unsafe(method(canBecomeMainWindow))]
        fn can_become_main(&self) -> bool {
            true
        }
    }
);

impl AquaWindow {
    pub(crate) fn new(
        mtm: MainThreadMarker,
        frame: NSRect,
        style: NSWindowStyleMask,
    ) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(());
        let window: Retained<Self> = unsafe {
            msg_send![
                super(this),
                initWithContentRect: frame,
                styleMask: style,
                backing: NSBackingStoreType::Buffered,
                defer: false
            ]
        };
        unsafe { window.setReleasedWhenClosed(false) };
        window
    }
}

#[derive(Debug)]
pub(crate) struct DelegateIvars {
    id: WindowId,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaWindowDelegate"]
    #[ivars = DelegateIvars]
    pub(crate) struct WindowDelegate;

    unsafe impl NSObjectProtocol for WindowDelegate {}

    unsafe impl NSWindowDelegate for WindowDelegate {
        #[unsafe(method(windowShouldClose:))]
        fn should_close(&self, _sender: &NSWindow) -> bool {
            emit(self.id(), ShellEvent::CloseRequested);
            callback_done();
            false
        }

        #[unsafe(method(windowDidResize:))]
        fn did_resize(&self, _notification: &NSNotification) {
            crate::window::refresh_metrics(self.id());
            crate::chrome::place_buttons(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidMove:))]
        fn did_move(&self, _notification: &NSNotification) {
            crate::window::report_position(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidChangeBackingProperties:))]
        fn did_change_backing(&self, _notification: &NSNotification) {
            crate::window::refresh_metrics(self.id());
            crate::chrome::place_buttons(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidChangeScreen:))]
        fn did_change_screen(&self, _notification: &NSNotification) {
            crate::window::refresh_metrics(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidBecomeKey:))]
        fn did_become_key(&self, _notification: &NSNotification) {
            emit(self.id(), ShellEvent::Focused(true));
            crate::chrome::place_buttons(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidResignKey:))]
        fn did_resign_key(&self, _notification: &NSNotification) {
            emit(self.id(), ShellEvent::Focused(false));
            crate::chrome::place_buttons(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidMiniaturize:))]
        fn did_miniaturize(&self, _notification: &NSNotification) {
            crate::state::change_presence(self.id(), Change::Miniaturized);
            callback_done();
        }

        #[unsafe(method(windowDidDeminiaturize:))]
        fn did_deminiaturize(&self, _notification: &NSNotification) {
            let occlusion = crate::window::occlusion(self.id());
            crate::state::change_presence(self.id(), Change::Deminiaturized(occlusion));
            callback_done();
        }

        #[unsafe(method(windowDidChangeOcclusionState:))]
        fn did_change_occlusion(&self, _notification: &NSNotification) {
            let occlusion = crate::window::occlusion(self.id());
            crate::state::change_presence(self.id(), Change::Occluded(occlusion));
            callback_done();
        }

        #[unsafe(method(windowDidEnterFullScreen:))]
        fn did_enter_full_screen(&self, _notification: &NSNotification) {
            crate::window::report_maximized(self.id());
            callback_done();
        }

        #[unsafe(method(windowDidExitFullScreen:))]
        fn did_exit_full_screen(&self, _notification: &NSNotification) {
            crate::window::report_maximized(self.id());
            crate::chrome::place_buttons(self.id());
            callback_done();
        }

        #[unsafe(method(windowWillStartLiveResize:))]
        fn will_start_live_resize(&self, _notification: &NSNotification) {
            wake_trace::record(Source::LiveResize);
        }

        #[unsafe(method(windowDidEndLiveResize:))]
        fn did_end_live_resize(&self, _notification: &NSNotification) {
            wake_trace::record(Source::LiveResize);
            callback_done();
        }
    }
);

impl WindowDelegate {
    pub(crate) fn new(mtm: MainThreadMarker, id: WindowId) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(DelegateIvars { id });
        unsafe { msg_send![super(this), init] }
    }

    fn id(&self) -> WindowId {
        self.ivars().id
    }
}

pub(crate) fn occlusion_of(window: &NSWindow) -> Occlusion {
    match window
        .occlusionState()
        .contains(NSWindowOcclusionState::Visible)
    {
        true => Occlusion::Exposed,
        false => Occlusion::Covered,
    }
}
