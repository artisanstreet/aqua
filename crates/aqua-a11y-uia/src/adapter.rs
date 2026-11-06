use std::cell::RefCell;
use std::rc::{Rc, Weak};
use std::sync::Arc;

use accesskit_windows::{Adapter, SubclassingAdapter, HWND, LPARAM, WPARAM};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use windows::Win32::UI::WindowsAndMessaging::{
    IsWindowVisible, WM_ENTERMENULOOP, WM_ENTERSIZEMOVE, WM_EXITMENULOOP, WM_EXITSIZEMOVE,
    WM_GETOBJECT, WM_KILLFOCUS, WM_SETFOCUS,
};

use crate::bridge::{A11yBridge, BridgeActionHandler, BridgeActivationHandler, Connection, WakeFn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    WindowAlreadyVisible,
}

struct Routed {
    adapter: Adapter,
    activation: BridgeActivationHandler,
}

enum Backend {
    Subclassed(SubclassingAdapter),
    Routed(Rc<RefCell<Routed>>),
}

pub struct UiaRouter {
    routed: Weak<RefCell<Routed>>,
}

impl UiaRouter {
    pub fn retired(&self) -> bool {
        self.routed.strong_count() == 0
    }

    pub fn handle(&self, message: u32, wparam: usize, lparam: isize) -> Option<isize> {
        let routed = self.routed.upgrade()?;
        match message {
            WM_GETOBJECT => {
                let mut state = routed.borrow_mut();
                let state = &mut *state;
                let result = state.adapter.handle_wm_getobject(
                    WPARAM(wparam),
                    LPARAM(lparam),
                    &mut state.activation,
                );
                result.map(|result| {
                    let result: accesskit_windows::LRESULT = result.into();
                    result.0
                })
            }
            WM_SETFOCUS | WM_EXITMENULOOP | WM_EXITSIZEMOVE => {
                Self::focus(&routed, true);
                None
            }
            WM_KILLFOCUS | WM_ENTERMENULOOP | WM_ENTERSIZEMOVE => {
                Self::focus(&routed, false);
                None
            }
            _ => None,
        }
    }

    fn focus(routed: &Rc<RefCell<Routed>>, focused: bool) {
        let events = routed
            .borrow_mut()
            .adapter
            .update_window_focus_state(focused);
        if let Some(events) = events {
            events.raise();
        }
    }
}

pub struct UiaAdapter {
    backend: Backend,
    bridge: Arc<A11yBridge>,
}

impl UiaAdapter {
    fn checked(hwnd: isize) -> Result<HWND, AttachError> {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        match unsafe { IsWindowVisible(hwnd) }.as_bool() {
            true => Err(AttachError::WindowAlreadyVisible),
            false => Ok(hwnd),
        }
    }

    pub fn attach(hwnd: isize, waker: WakeFn) -> Result<Self, AttachError> {
        let hwnd = Self::checked(hwnd)?;
        let bridge = A11yBridge::new(waker);
        let adapter = SubclassingAdapter::new(
            hwnd,
            BridgeActivationHandler::new(bridge.clone()),
            BridgeActionHandler::new(bridge.clone()),
        );
        Ok(Self {
            backend: Backend::Subclassed(adapter),
            bridge,
        })
    }

    pub fn attach_routed(hwnd: isize, waker: WakeFn) -> Result<(Self, UiaRouter), AttachError> {
        let hwnd = Self::checked(hwnd)?;
        let bridge = A11yBridge::new(waker);
        let routed = Rc::new(RefCell::new(Routed {
            adapter: Adapter::new(hwnd, false, BridgeActionHandler::new(bridge.clone())),
            activation: BridgeActivationHandler::new(bridge.clone()),
        }));
        let router = UiaRouter {
            routed: Rc::downgrade(&routed),
        };
        Ok((
            Self {
                backend: Backend::Routed(routed),
                bridge,
            },
            router,
        ))
    }

    pub fn connection(&self) -> Connection {
        self.bridge.connection()
    }

    pub fn apply(&mut self, update: &AccessTreeUpdate) {
        let Some(snapshot) = self.bridge.snapshot(update) else {
            return;
        };
        let events = match &mut self.backend {
            Backend::Subclassed(adapter) => adapter.update_if_active(|| snapshot),
            Backend::Routed(routed) => routed.borrow_mut().adapter.update_if_active(|| snapshot),
        };
        if let Some(events) = events {
            events.raise();
        }
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
