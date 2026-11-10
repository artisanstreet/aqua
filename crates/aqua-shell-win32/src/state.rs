use crate::event::{Chrome, Handler, ShellEvent, WindowKind};
use aqua_core::{CursorIcon, Modifiers};
use aqua_protocol::chrome::DragRects;
use aqua_protocol::{CaptionControl, DragRegions, Instant, WindowId};
use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};

pub(crate) enum Queued {
    Window(WindowId, ShellEvent),
    Waitable(usize),
    Wake,
}

pub(crate) struct WindowState {
    pub id: WindowId,
    pub scale: f64,
    pub chrome: Chrome,
    pub kind: WindowKind,
    pub min_logical: Option<(f64, f64)>,
    pub modifiers: Modifiers,
    pub resizable: bool,
    pub minimizable: bool,
    pub regions: DragRegions,
    pub rects: DragRects,
    pub cursor: CursorIcon,
    pub redraw_pending: bool,
    pub resized_since_paint: bool,
    pub tracking_client: bool,
    pub tracking_nc: bool,
    pub hover: Option<CaptionControl>,
    pub buttons_down: u8,
    pub raw_mouse: bool,
    pub minimized: bool,
    pub covered: bool,
    pub text: crate::input::Utf16Assembler,
    pub touch_buttons: HashMap<u32, u8>,
    pub composition: Option<crate::composition::Composition>,
}

#[derive(Default)]
pub(crate) struct Shell {
    pub windows: HashMap<isize, WindowState>,
    pub queue: VecDeque<Queued>,
    pub wake_at: Option<Instant>,
    pub waitables: Vec<isize>,
    pub in_modal: bool,
}

pub(crate) static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

thread_local! {
    pub(crate) static SHELL: RefCell<Shell> = RefCell::new(Shell::default());
    pub(crate) static HANDLER: Cell<Option<*mut dyn Handler>> = const { Cell::new(None) };
    pub(crate) static IN_HANDLER: Cell<bool> = const { Cell::new(false) };
    pub(crate) static UI_THREAD: Cell<bool> = const { Cell::new(false) };
    pub(crate) static LAST_PUMP: Cell<Option<std::time::Instant>> = const { Cell::new(None) };
}

pub(crate) fn with<R>(f: impl FnOnce(&mut Shell) -> R) -> R {
    let (result, wake) = SHELL.with(|s| {
        let mut shell = s.borrow_mut();
        let result = f(&mut shell);
        let wake = crate::looping::message_waiting()
            && crate::looping::message_wait_needs_wake(
                shell.wake_at,
                !shell.queue.is_empty()
                    || !shell.waitables.is_empty()
                    || shell.windows.values().any(|window| window.redraw_pending),
            );
        (result, wake)
    });
    if wake {
        crate::looping::wake_message_wait();
    }
    result
}

pub(crate) fn window<R>(hwnd: isize, f: impl FnOnce(&mut WindowState) -> R) -> Option<R> {
    with(|s| s.windows.get_mut(&hwnd).map(f))
}

pub(crate) fn emit(hwnd: isize, event: ShellEvent) {
    with(|s| {
        if let Some(w) = s.windows.get(&hwnd) {
            let id = w.id;
            s.queue.push_back(Queued::Window(id, event));
        }
    });
}
