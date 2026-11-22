use crate::access::AccessHooks;
use crate::lease::{self, LayerGeometry};
use crate::looping::emit;
use crate::model::drop::DropTarget;
use crate::model::presence::{Change, Presence};
use crate::text_input::TextInputClient;
use crate::view::AquaView;
use crate::window_class::{AquaWindow, WindowDelegate};
use aqua_core::CursorIcon;
use aqua_protocol::{DragRegions, WindowId};
use aqua_shell::{Chrome, ShellEvent, WindowKind};
use objc2::rc::Retained;
use objc2_quartz_core::CAMetalLayer;
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::rc::Rc;
use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct Spec {
    pub chrome: Chrome,
    pub kind: WindowKind,
    pub titlebar_height: f64,
    pub caption_buttons: bool,
    pub resizable: bool,
    pub minimizable: bool,
    pub maximizable: bool,
    pub alpha_surface: bool,
}

impl Spec {
    pub(crate) fn overlay(self) -> bool {
        self.chrome == Chrome::Custom && self.caption_buttons
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct Reported {
    pub scale: Option<f64>,
    pub physical: Option<(u32, u32)>,
    pub position: Option<(i32, i32)>,
    pub maximized: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) struct Pointer {
    pub inside: bool,
    pub buttons: u8,
}

pub(crate) type SharedClient = Rc<RefCell<Box<dyn TextInputClient>>>;
pub(crate) type SharedAccess = Rc<RefCell<Box<dyn AccessHooks>>>;

pub(crate) struct WindowState {
    pub native: Retained<AquaWindow>,
    pub view: Retained<AquaView>,
    pub _delegate: Retained<WindowDelegate>,
    pub layer: Retained<CAMetalLayer>,
    pub spec: Spec,
    pub presence: Presence,
    pub reported: Reported,
    pub regions: DragRegions,
    pub cursor: CursorIcon,
    pub pointer: Pointer,
    pub raw_mouse: bool,
    pub redraw_pending: bool,
    pub geometry: Arc<Mutex<LayerGeometry>>,
    pub drop: DropTarget,
    pub text_client: Option<SharedClient>,
    pub access: Option<SharedAccess>,
}

struct Registry {
    windows: BTreeMap<WindowId, WindowState>,
    next: u64,
}

thread_local! {
    static REGISTRY: RefCell<Registry> = const { RefCell::new(Registry { windows: BTreeMap::new(), next: 1 }) };
}

pub(crate) fn next_id() -> WindowId {
    REGISTRY.with_borrow_mut(|registry| {
        let id =
            WindowId::new(registry.next).expect("invariant: window ids start at one and only grow");
        registry.next += 1;
        id
    })
}

pub(crate) fn insert(id: WindowId, window: WindowState) {
    REGISTRY.with_borrow_mut(|registry| {
        let previous = registry.windows.insert(id, window);
        assert!(
            previous.is_none(),
            "invariant: window {id:?} is inserted once"
        );
    });
}

pub(crate) fn remove(id: WindowId) -> Option<(WindowState, usize)> {
    REGISTRY.with_borrow_mut(|registry| {
        registry
            .windows
            .remove(&id)
            .map(|window| (window, registry.windows.len()))
    })
}

pub(crate) fn window<T>(id: WindowId, read: impl FnOnce(&mut WindowState) -> T) -> Option<T> {
    REGISTRY.with_borrow_mut(|registry| registry.windows.get_mut(&id).map(read))
}

pub(crate) fn native(id: WindowId) -> Option<Retained<AquaWindow>> {
    window(id, |window| window.native.clone())
}

pub(crate) fn window_ids() -> Vec<WindowId> {
    REGISTRY.with_borrow(|registry| registry.windows.keys().copied().collect())
}

pub(crate) fn redraw_pending() -> bool {
    REGISTRY.with_borrow(|registry| {
        registry
            .windows
            .values()
            .any(|window| window.redraw_pending)
    })
}

pub(crate) fn take_redraws() -> Vec<WindowId> {
    REGISTRY.with_borrow_mut(|registry| {
        registry
            .windows
            .iter_mut()
            .filter_map(|(id, window)| std::mem::take(&mut window.redraw_pending).then_some(*id))
            .collect()
    })
}

pub(crate) fn change_presence(id: WindowId, change: Change) {
    let reports = window(id, |window| {
        let (next, reports) = window.presence.apply(change);
        window.presence = next;
        let covered = next.covered();
        lease::update(&window.geometry, |geometry| geometry.covered = covered);
        reports
    });
    let Some(reports) = reports else {
        return;
    };
    if let Some(minimized) = reports.minimized {
        emit(id, ShellEvent::Minimized(minimized));
    }
    if let Some(covered) = reports.covered {
        emit(id, ShellEvent::Covered(covered));
    }
}

pub(crate) fn screens_changed() {
    for id in window_ids() {
        crate::window::refresh_metrics(id);
    }
}
