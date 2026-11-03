use std::cell::RefCell;
use std::fmt;
use std::rc::Rc;

use aqua_core::Point;
use aqua_reactive::{DepTuple, Scope};

use crate::element::Element;
use crate::overlay::{CloseReason, DismissPolicy, Modality, OverlayKind, Placement};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PopupFocus {
    Preserve,
    First,
    Root,
}

#[derive(Clone)]
pub struct PopupOptions {
    pub kind: OverlayKind,
    pub modality: Modality,
    pub dismiss: DismissPolicy,
    pub placement: Placement,
    pub focus: PopupFocus,
    pub at: Option<Point>,
    pub on_close: Option<Rc<dyn Fn(CloseReason)>>,
    pub reactive: Option<ReactivePopup>,
}

impl fmt::Debug for PopupOptions {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PopupOptions")
            .field("kind", &self.kind)
            .field("modality", &self.modality)
            .field("focus", &self.focus)
            .finish()
    }
}

impl PopupOptions {
    pub fn menu() -> Self {
        Self {
            kind: OverlayKind::Menu,
            modality: Modality::NonModal,
            dismiss: DismissPolicy::LIGHT,
            placement: Placement::below(),
            focus: PopupFocus::First,
            at: None,
            on_close: None,
            reactive: None,
        }
    }

    pub fn dialog() -> Self {
        Self {
            kind: OverlayKind::Dialog,
            modality: Modality::Modal,
            dismiss: DismissPolicy::ESCAPE_ONLY,
            placement: Placement::centered_dialog(),
            focus: PopupFocus::First,
            at: None,
            on_close: None,
            reactive: None,
        }
    }

    pub fn tooltip() -> Self {
        Self {
            kind: OverlayKind::Tooltip,
            focus: PopupFocus::Preserve,
            ..Self::menu()
        }
    }

    pub fn at(mut self, at: Point) -> Self {
        self.at = Some(at);
        self.placement.align = crate::overlay::Align::Start;
        self.placement.gap = 0.0;
        self
    }

    pub fn on_close(mut self, callback: impl Fn(CloseReason) + 'static) -> Self {
        self.on_close = Some(Rc::new(callback));
        self
    }
}

#[derive(Clone)]
pub struct Tooltip {
    pub delay: f64,
    pub content: Rc<dyn Fn() -> Element>,
}

impl fmt::Debug for Tooltip {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Tooltip")
            .field("delay", &self.delay)
            .finish()
    }
}

pub type PendingPopup = Rc<RefCell<Option<Element>>>;

type PopupSubscription = Rc<dyn Fn(&Scope, PendingPopup)>;

#[derive(Clone)]
pub struct ReactivePopup {
    initial: Rc<dyn Fn() -> Element>,
    subscribe: PopupSubscription,
}

impl ReactivePopup {
    pub fn new<D: DepTuple>(deps: D, render: impl Fn(D::Values) -> Element + 'static) -> Self {
        let render = Rc::new(render);
        let initial_deps = deps.clone();
        let initial_render = render.clone();
        Self {
            initial: Rc::new(move || initial_render(initial_deps.snapshot())),
            subscribe: Rc::new(move |scope, pending| {
                let render = render.clone();
                scope.effect(deps.clone(), move |values| {
                    *pending.borrow_mut() = Some(render(values))
                });
            }),
        }
    }

    pub fn initial(&self) -> Element {
        (self.initial)()
    }

    pub fn attach(&self, scope: &Scope, pending: PendingPopup) {
        (self.subscribe)(scope, pending);
    }
}
