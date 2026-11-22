use std::cell::RefCell;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;

use accesskit_macos::{Adapter, NSPoint, QueuedEvents};
use aqua_a11y::accesskit::TreeUpdate;
use aqua_a11y::{A11yBridge, BridgeActionHandler, BridgeActivationHandler, Connection, WakeFn};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use aqua_shell_appkit::{AccessHooks, Window};

fn titled(mut snapshot: TreeUpdate, title: &str) -> TreeUpdate {
    let root = snapshot.tree.as_ref().map(|tree| tree.root);
    for (id, node) in &mut snapshot.nodes {
        if Some(*id) == root && node.label().is_none() && !title.is_empty() {
            node.set_label(title);
        }
    }
    snapshot
}

struct Routed {
    adapter: Adapter,
    activation: BridgeActivationHandler,
}

type Shared = Rc<RefCell<Routed>>;

fn routed<T>(shared: &Shared, call: impl FnOnce(&mut Routed) -> T) -> T {
    let mut routed = shared
        .try_borrow_mut()
        .expect("invariant: AppKit never re-enters the accessibility adapter while it is updating");
    call(&mut routed)
}

fn raise(events: Option<QueuedEvents>) {
    if let Some(events) = events {
        events.raise();
    }
}

struct Hooks {
    shared: Shared,
}

unsafe impl AccessHooks for Hooks {
    fn children(&mut self) -> *mut c_void {
        routed(&self.shared, |routed| {
            routed.adapter.view_children(&mut routed.activation).cast()
        })
    }

    fn focused(&mut self) -> *mut c_void {
        routed(&self.shared, |routed| {
            routed.adapter.focus(&mut routed.activation).cast()
        })
    }

    fn hit_test(&mut self, screen_point: (f64, f64)) -> *mut c_void {
        let point = NSPoint::new(screen_point.0, screen_point.1);
        routed(&self.shared, |routed| {
            routed
                .adapter
                .hit_test(point, &mut routed.activation)
                .cast()
        })
    }
}

pub struct AxAdapter {
    window: Window,
    shared: Shared,
    bridge: Arc<A11yBridge>,
    title: String,
}

impl AxAdapter {
    pub fn attach(window: Window, waker: WakeFn) -> Self {
        let view = window.view_pointer().unwrap_or_else(|| {
            panic!(
                "invariant: window {:?} has a view when accessibility attaches",
                window.id()
            )
        });
        let bridge = A11yBridge::new(waker);
        let adapter = unsafe {
            Adapter::new(
                view.as_ptr(),
                window.is_foreground(),
                BridgeActionHandler::new(bridge.clone()),
            )
        };
        let shared = Rc::new(RefCell::new(Routed {
            adapter,
            activation: BridgeActivationHandler::new(bridge.clone()),
        }));
        window.set_access_hooks(Some(Box::new(Hooks {
            shared: shared.clone(),
        })));
        Self {
            window,
            shared,
            bridge,
            title: String::new(),
        }
    }

    pub fn connection(&self) -> Connection {
        self.bridge.connection()
    }

    pub fn epoch(&self) -> u64 {
        self.bridge.epoch()
    }

    pub fn set_window_title(&mut self, title: &str) {
        if self.title != title {
            self.title = title.to_owned();
        }
    }

    pub fn apply(&mut self, update: &AccessTreeUpdate) {
        if let Some(snapshot) = self.bridge.snapshot(update) {
            let title = self.title.as_str();
            let events = routed(&self.shared, |routed| {
                routed.adapter.update_if_active(|| titled(snapshot, title))
            });
            raise(events);
        }
    }

    pub fn focus(&mut self, focused: bool) {
        let events = routed(&self.shared, |routed| {
            routed.adapter.update_view_focus_state(focused)
        });
        raise(events);
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}

impl Drop for AxAdapter {
    fn drop(&mut self) {
        self.window.set_access_hooks(None);
    }
}
