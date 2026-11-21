use crate::window::Window;
use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{define_class, msg_send, AnyThread, DefinedClass, MainThreadOnly};
use objc2_app_kit::{
    NSBackingStoreType, NSDragOperation, NSDraggingContext, NSDraggingItem, NSDraggingSession,
    NSDraggingSource, NSEvent, NSPasteboard, NSPasteboardWriting, NSResponder, NSView, NSWindow,
    NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSArray, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize, NSString, NSURL,
};
use std::cell::Cell;
use std::path::{Path, PathBuf};

fn mtm() -> MainThreadMarker {
    MainThreadMarker::new().expect("tests drive AppKit from the main thread")
}

fn urls(paths: &[PathBuf]) -> Vec<Retained<NSURL>> {
    paths.iter().map(|path| file_url(path)).collect()
}

fn file_url(path: &Path) -> Retained<NSURL> {
    NSURL::fileURLWithPath(&NSString::from_str(&path.to_string_lossy()))
}

#[derive(Debug)]
pub struct InfoIvars {
    board: Retained<NSPasteboard>,
    location: Cell<NSPoint>,
}

define_class!(
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaTestDraggingInfo"]
    #[ivars = InfoIvars]
    pub struct TestDraggingInfo;

    impl TestDraggingInfo {
        #[unsafe(method_id(draggingPasteboard))]
        fn dragging_pasteboard(&self) -> Retained<NSPasteboard> {
            self.ivars().board.clone()
        }

        #[unsafe(method(draggingLocation))]
        fn dragging_location(&self) -> NSPoint {
            self.ivars().location.get()
        }
    }
);

pub struct FakeDrag {
    window: Window,
    info: Retained<TestDraggingInfo>,
}

impl FakeDrag {
    pub fn carrying(window: Window, paths: &[PathBuf]) -> FakeDrag {
        let board = NSPasteboard::pasteboardWithUniqueName();
        board.clearContents();
        let writers: Vec<Retained<ProtocolObject<dyn NSPasteboardWriting>>> = urls(paths)
            .into_iter()
            .map(ProtocolObject::from_retained)
            .collect();
        if !writers.is_empty() {
            assert!(
                board.writeObjects(&NSArray::from_retained_slice(&writers)),
                "the private drag pasteboard takes the file URLs"
            );
        }
        let info = TestDraggingInfo::alloc(mtm()).set_ivars(InfoIvars {
            board,
            location: Cell::new(NSPoint::new(0.0, 0.0)),
        });
        let info: Retained<TestDraggingInfo> = unsafe { msg_send![super(info), init] };
        FakeDrag { window, info }
    }

    fn view(&self) -> Retained<crate::view::AquaView> {
        crate::state::window(self.window.id(), |state| state.view.clone())
            .expect("fake drags target a live window")
    }

    fn at(&self, (x, y): (f64, f64)) -> Retained<crate::view::AquaView> {
        let view = self.view();
        self.info
            .ivars()
            .location
            .set(view.convertPoint_toView(NSPoint::new(x, y), None));
        view
    }

    fn sender(&self) -> &AnyObject {
        &self.info
    }

    pub fn enter(&self, at: (f64, f64)) -> NSDragOperation {
        let view = self.at(at);
        unsafe { msg_send![&*view, draggingEntered: self.sender()] }
    }

    pub fn update(&self, at: (f64, f64)) -> NSDragOperation {
        let view = self.at(at);
        unsafe { msg_send![&*view, draggingUpdated: self.sender()] }
    }

    pub fn exit(&self) {
        let view = self.view();
        unsafe { msg_send![&*view, draggingExited: Some(self.sender())] }
    }

    pub fn perform(&self, at: (f64, f64)) -> bool {
        let view = self.at(at);
        unsafe { msg_send![&*view, performDragOperation: self.sender()] }
    }
}

thread_local! {
    static SOURCE_ENDED: Cell<bool> = const { Cell::new(false) };
}

#[derive(Debug)]
pub struct SourceIvars {
    paths: Vec<PathBuf>,
}

define_class!(
    #[unsafe(super(NSView, NSResponder, NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "AquaTestDragSource"]
    #[ivars = SourceIvars]
    pub struct DragSourceView;

    impl DragSourceView {
        #[unsafe(method(acceptsFirstMouse:))]
        fn accepts_first_mouse(&self, _event: Option<&NSEvent>) -> bool {
            true
        }

        #[unsafe(method(mouseDown:))]
        fn mouse_down(&self, event: &NSEvent) {
            let bounds = self.bounds();
            let items: Vec<Retained<NSDraggingItem>> = urls(&self.ivars().paths)
                .into_iter()
                .map(|url| {
                    let item = NSDraggingItem::initWithPasteboardWriter(
                        NSDraggingItem::alloc(),
                        ProtocolObject::from_ref(&*url),
                    );
                    unsafe { item.setDraggingFrame_contents(bounds, None) };
                    item
                })
                .collect();
            println!("pressed");
            self.beginDraggingSessionWithItems_event_source(
                &NSArray::from_retained_slice(&items),
                event,
                ProtocolObject::from_ref(self),
            );
            println!("began");
        }
    }

    unsafe impl NSObjectProtocol for DragSourceView {}

    unsafe impl NSDraggingSource for DragSourceView {
        #[unsafe(method(draggingSession:sourceOperationMaskForDraggingContext:))]
        fn operation_mask(
            &self,
            _session: &NSDraggingSession,
            _context: NSDraggingContext,
        ) -> NSDragOperation {
            NSDragOperation::Copy
        }

        #[unsafe(method(draggingSession:movedToPoint:))]
        fn moved(&self, _session: &NSDraggingSession, point: NSPoint) {
            println!("moved {} {}", point.x, point.y);
        }

        #[unsafe(method(draggingSession:endedAtPoint:operation:))]
        fn ended(&self, _session: &NSDraggingSession, _point: NSPoint, _operation: NSDragOperation) {
            SOURCE_ENDED.set(true);
        }
    }
);

pub struct DragSource {
    _window: Retained<NSWindow>,
    pub centre: (f64, f64),
}

pub fn source_window(top_left: (f64, f64), size: (f64, f64), paths: &[PathBuf]) -> DragSource {
    let mtm = mtm();
    crate::app::ensure(mtm);
    let primary = crate::output::primary_height(mtm);
    let frame = NSRect::new(
        NSPoint::new(top_left.0, primary - top_left.1 - size.1),
        NSSize::new(size.0, size.1),
    );
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            frame,
            NSWindowStyleMask::Borderless,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    let view = DragSourceView::alloc(mtm).set_ivars(SourceIvars {
        paths: paths.to_vec(),
    });
    let view: Retained<DragSourceView> = unsafe {
        msg_send![super(view), initWithFrame: NSRect::new(NSPoint::new(0.0, 0.0), frame.size)]
    };
    window.setContentView(Some(&view));
    window.orderFrontRegardless();
    DragSource {
        _window: window,
        centre: (top_left.0 + size.0 / 2.0, top_left.1 + size.1 / 2.0),
    }
}

pub fn source_ended() -> bool {
    SOURCE_ENDED.get()
}

pub fn drop_state(window: Window) -> String {
    crate::state::window(window.id(), |state| format!("{:?}", state.drop))
        .unwrap_or_else(|| "no window".into())
}
