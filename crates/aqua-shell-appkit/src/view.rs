use crate::input::{keyboard, pointer, scroll, text};
use crate::model::keys::Interpretation;
use aqua_protocol::WindowId;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject, Sel};
use objc2::{define_class, msg_send, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSDragOperation, NSDraggingDestination, NSDraggingInfo, NSEvent, NSResponder,
    NSTextInputClient, NSView,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSAttributedString, NSAttributedStringKey, NSObject,
    NSObjectProtocol, NSPoint, NSRange, NSRangePointer, NSRect, NSUInteger,
};
use objc2_quartz_core::{CALayer, CAMetalLayer};
use std::cell::{Cell, RefCell};

#[derive(Debug)]
pub(crate) struct ViewIvars {
    pub id: WindowId,
    pub pass: RefCell<Option<Interpretation>>,
    pub marked: RefCell<Option<String>>,
    pub consumed: Cell<Option<u16>>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaView"]
    #[ivars = ViewIvars]
    pub(crate) struct AquaView;

    impl AquaView {
        #[unsafe(method(isFlipped))]
        fn is_flipped(&self) -> bool {
            true
        }

        #[unsafe(method_id(makeBackingLayer))]
        fn make_backing_layer(&self) -> Retained<CALayer> {
            Retained::into_super(CAMetalLayer::new())
        }

        #[unsafe(method(acceptsFirstResponder))]
        fn accepts_first_responder(&self) -> bool {
            true
        }

        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDownCanMoveWindow))]
        fn mouse_down_can_move_window(&self) -> bool {
            false
        }

        #[unsafe(method(viewDidChangeBackingProperties))]
        fn did_change_backing(&self) {
            crate::window::refresh_metrics(self.id());
            crate::looping::callback_done();
        }

        #[unsafe(method(keyDown:))]
        fn key_down(&self, event: &NSEvent) {
            keyboard::key_down(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(keyUp:))]
        fn key_up(&self, event: &NSEvent) {
            keyboard::key_up(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(flagsChanged:))]
        fn flags_changed(&self, event: &NSEvent) {
            keyboard::flags_changed(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(performKeyEquivalent:))]
        fn perform_key_equivalent(&self, event: &NSEvent) -> bool {
            let handled = keyboard::key_equivalent(self, event);
            crate::looping::callback_done();
            handled
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            pointer::pressed(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(mouseUp:))]
        fn mouse_up(&self, event: &NSEvent) {
            pointer::released(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(rightMouseDown:))]
        fn right_mouse_down(&self, event: &NSEvent) {
            pointer::pressed(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(rightMouseUp:))]
        fn right_mouse_up(&self, event: &NSEvent) {
            pointer::released(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(otherMouseDown:))]
        fn other_mouse_down(&self, event: &NSEvent) {
            pointer::pressed(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(otherMouseUp:))]
        fn other_mouse_up(&self, event: &NSEvent) {
            pointer::released(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(mouseMoved:))]
        fn mouse_moved(&self, event: &NSEvent) {
            pointer::moved(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(mouseDragged:))]
        fn mouse_dragged(&self, event: &NSEvent) {
            pointer::moved(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(rightMouseDragged:))]
        fn right_mouse_dragged(&self, event: &NSEvent) {
            pointer::moved(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(otherMouseDragged:))]
        fn other_mouse_dragged(&self, event: &NSEvent) {
            pointer::moved(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(mouseEntered:))]
        fn mouse_entered(&self, event: &NSEvent) {
            pointer::entered(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(mouseExited:))]
        fn mouse_exited(&self, _event: &NSEvent) {
            pointer::exited(self);
            crate::looping::callback_done();
        }

        #[unsafe(method(cursorUpdate:))]
        fn cursor_update(&self, _event: &NSEvent) {
            crate::cursor::apply(self.id());
        }

        #[unsafe(method(scrollWheel:))]
        fn scroll_wheel(&self, event: &NSEvent) {
            scroll::wheel(self, event);
            crate::looping::callback_done();
        }

        #[unsafe(method(magnifyWithEvent:))]
        fn magnify(&self, _event: &NSEvent) {}

        #[unsafe(method_id(accessibilityChildren))]
        fn accessibility_children(&self) -> Option<Retained<AnyObject>> {
            crate::view::access_children(self)
        }

        #[unsafe(method_id(accessibilityFocusedUIElement))]
        fn accessibility_focused(&self) -> Option<Retained<AnyObject>> {
            crate::view::access_focused(self)
        }

        #[unsafe(method_id(accessibilityHitTest:))]
        fn accessibility_hit_test(&self, point: NSPoint) -> Option<Retained<AnyObject>> {
            crate::view::access_hit_test(self, point)
        }
    }

    unsafe impl NSObjectProtocol for AquaView {}

    unsafe impl NSTextInputClient for AquaView {
        #[unsafe(method(insertText:replacementRange:))]
        fn insert_text(&self, string: &AnyObject, replacement: NSRange) {
            text::insert(self, string, replacement);
            crate::looping::callback_done();
        }

        #[unsafe(method(doCommandBySelector:))]
        fn do_command_by_selector(&self, _selector: Sel) {
            text::command(self);
        }

        #[unsafe(method(setMarkedText:selectedRange:replacementRange:))]
        fn set_marked_text(&self, string: &AnyObject, selected: NSRange, replacement: NSRange) {
            text::set_marked(self, string, selected, replacement);
        }

        #[unsafe(method(unmarkText))]
        fn unmark_text(&self) {
            text::unmark(self);
        }

        #[unsafe(method(selectedRange))]
        fn selected_range(&self) -> NSRange {
            text::selected_range(self)
        }

        #[unsafe(method(markedRange))]
        fn marked_range(&self) -> NSRange {
            text::marked_range(self)
        }

        #[unsafe(method(hasMarkedText))]
        fn has_marked_text(&self) -> bool {
            text::has_marked(self)
        }

        #[unsafe(method_id(attributedSubstringForProposedRange:actualRange:))]
        fn attributed_substring(
            &self,
            range: NSRange,
            actual: NSRangePointer,
        ) -> Option<Retained<NSAttributedString>> {
            text::attributed_substring(self, range, actual)
        }

        #[unsafe(method_id(validAttributesForMarkedText))]
        fn valid_attributes(&self) -> Retained<NSArray<NSAttributedStringKey>> {
            NSArray::new()
        }

        #[unsafe(method(firstRectForCharacterRange:actualRange:))]
        fn first_rect(&self, range: NSRange, actual: NSRangePointer) -> NSRect {
            text::first_rect(self, range, actual)
        }

        #[unsafe(method(characterIndexForPoint:))]
        fn character_index(&self, point: NSPoint) -> NSUInteger {
            text::character_index(self, point)
        }
    }

    unsafe impl NSDraggingDestination for AquaView {
        #[unsafe(method(draggingEntered:))]
        fn dragging_entered(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let operation = crate::drop::entered(self.id(), sender);
            crate::looping::callback_done();
            operation
        }

        #[unsafe(method(draggingUpdated:))]
        fn dragging_updated(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> NSDragOperation {
            let operation = crate::drop::updated(self.id(), sender);
            crate::looping::callback_done();
            operation
        }

        #[unsafe(method(draggingExited:))]
        fn dragging_exited(&self, _sender: Option<&ProtocolObject<dyn NSDraggingInfo>>) {
            crate::drop::exited(self.id());
            crate::looping::callback_done();
        }

        #[unsafe(method(performDragOperation:))]
        fn perform_drag(&self, sender: &ProtocolObject<dyn NSDraggingInfo>) -> bool {
            let accepted = crate::drop::perform(self.id(), sender);
            crate::looping::callback_done();
            accepted
        }
    }
);

impl AquaView {
    pub(crate) fn new(mtm: MainThreadMarker, id: WindowId, frame: NSRect) -> Retained<Self> {
        let this = Self::alloc(mtm).set_ivars(ViewIvars {
            id,
            pass: RefCell::new(None),
            marked: RefCell::new(None),
            consumed: Cell::new(None),
        });
        unsafe { msg_send![super(this), initWithFrame: frame] }
    }

    pub(crate) fn id(&self) -> WindowId {
        self.ivars().id
    }
}

fn hooked(
    view: &AquaView,
    call: impl FnOnce(&mut dyn crate::access::AccessHooks) -> *mut std::ffi::c_void,
) -> Option<Retained<AnyObject>> {
    let hooks = crate::state::window(view.id(), |window| window.access.clone()).flatten()?;
    let raw = call(hooks.borrow_mut().as_mut());
    unsafe { Retained::retain(raw.cast::<AnyObject>()) }
}

fn access_children(view: &AquaView) -> Option<Retained<AnyObject>> {
    hooked(view, |hooks| hooks.children())
        .or_else(|| unsafe { msg_send![super(view), accessibilityChildren] })
}

fn access_focused(view: &AquaView) -> Option<Retained<AnyObject>> {
    hooked(view, |hooks| hooks.focused())
        .or_else(|| unsafe { msg_send![super(view), accessibilityFocusedUIElement] })
}

fn access_hit_test(view: &AquaView, point: NSPoint) -> Option<Retained<AnyObject>> {
    hooked(view, |hooks| hooks.hit_test((point.x, point.y)))
        .or_else(|| unsafe { msg_send![super(view), accessibilityHitTest: point] })
}
