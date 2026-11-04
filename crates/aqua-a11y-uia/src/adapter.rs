use std::sync::Arc;

use accesskit_windows::{HWND, SubclassingAdapter};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};
use windows::Win32::UI::WindowsAndMessaging::IsWindowVisible;

use crate::bridge::{A11yBridge, BridgeActionHandler, BridgeActivationHandler, Connection, WakeFn};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AttachError {
    WindowAlreadyVisible,
}

pub struct UiaAdapter {
    adapter: SubclassingAdapter,
    bridge: Arc<A11yBridge>,
}

impl UiaAdapter {
    pub fn attach(hwnd: isize, waker: WakeFn) -> Result<Self, AttachError> {
        let hwnd = HWND(hwnd as *mut core::ffi::c_void);
        if unsafe { IsWindowVisible(hwnd) }.as_bool() {
            return Err(AttachError::WindowAlreadyVisible);
        }
        let bridge = A11yBridge::new(waker);
        let adapter = SubclassingAdapter::new(
            hwnd,
            BridgeActivationHandler::new(bridge.clone()),
            BridgeActionHandler::new(bridge.clone()),
        );
        Ok(Self { adapter, bridge })
    }

    pub fn connection(&self) -> Connection {
        self.bridge.connection()
    }

    pub fn apply(&mut self, update: &AccessTreeUpdate) {
        let Some(snapshot) = self.bridge.snapshot(update) else {
            return;
        };
        if let Some(events) = self.adapter.update_if_active(|| snapshot) {
            events.raise();
        }
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
