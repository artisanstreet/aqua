//! Windows OS adapter for accessibility (UIA via `accesskit_winit`).
//!
//! `cfg(windows)`-gated: the adapter pulls the OS backend crates. Portable
//! tree/action vocabulary and the wake-contract bridge live in [`crate::a11y`]
//! and compile everywhere.
//!
//! Integration contract for the facade:
//! 1. Create the window **initially invisible**
//!    (`WindowAttributes::with_visible(false)` — the AccessKit adapter
//!    panics on visible windows; [`A11yAdapter::attach`] rejects them with
//!    [`A11yError::WindowAlreadyVisible`](crate::a11y::A11yError::WindowAlreadyVisible)
//!    first).
//! 2. `attach_with_waker(event_loop, window, waker)`, then
//!    `window.set_visible(true)`. The waker is REQUIRED: without it a
//!    screen-reader action arriving while the app idles would sit queued
//!    forever. The facade's waker should wake the event loop (proxy /
//!    `EventLoopProxy::wake_up` equivalent) and route to `drain_actions`.
//!    It is always invoked AFTER the queue lock is released, so re-entering
//!    the queue from the waker cannot deadlock (unit-tested in `a11y`).
//! 3. Forward every `winit::event::WindowEvent` to
//!    [`A11yAdapter::process_window_event`] BEFORE app handling.
//! 4. Push [`crate::a11y::build_tree_update`] output on every semantic change
//!    via [`A11yAdapter::push_snapshot`]; drain
//!    [`A11yAdapter::drain_actions`] into UI behaviors. AT clicks arrive as
//!    action requests — never depend on mouse input for them.

use std::sync::Arc;

use crate::a11y::{
    A11yActionRequest, A11yBridge, A11yError, BridgeActionHandler, BridgeActivationHandler,
    BridgeDeactivationHandler, WakeFn,
};
use accesskit::TreeUpdate;

/// Live per-window accessibility bridge. Construct via [`A11yAdapter::attach`]
/// (event-loop waker) — there is no waker-less production constructor on
/// purpose; see [`A11yBridge::without_waker`] for tests only.
pub struct A11yAdapter {
    adapter: accesskit_winit::Adapter,
    bridge: Arc<A11yBridge>,
}

impl A11yAdapter {
    /// Attach with an explicit event-loop waker (REQUIRED — see module docs).
    pub fn attach_with_waker(
        event_loop: &winit::event_loop::ActiveEventLoop,
        window: &winit::window::Window,
        waker: WakeFn,
    ) -> Result<Self, A11yError> {
        if window.is_visible() == Some(true) {
            return Err(A11yError::WindowAlreadyVisible);
        }
        let bridge = Arc::new(A11yBridge::new(waker));
        let adapter = accesskit_winit::Adapter::with_direct_handlers(
            event_loop,
            window,
            BridgeActivationHandler::new(bridge.clone()),
            BridgeActionHandler::new(bridge.clone()),
            BridgeDeactivationHandler::new(bridge.clone()),
        );
        Ok(Self { adapter, bridge })
    }

    /// Attach with a queue-only bridge. No wakeups are ever emitted: AT
    /// actions may wait indefinitely while idle. Only suitable for tests
    /// and shut-down paths — production code must use `attach_with_waker`.
    pub fn attach_without_waker(
        event_loop: &winit::event_loop::ActiveEventLoop,
        window: &winit::window::Window,
    ) -> Result<Self, A11yError> {
        Self::attach_with_waker(event_loop, window, Arc::new(|| {}))
    }

    /// Stage the tree served synchronously when AT requests the initial
    /// tree. The facade should also `push_snapshot` the same content.
    /// Wakes the loop so a staged-then-idle app still pushes on dispatch.
    pub fn stage_initial(&self, update: TreeUpdate) -> Result<(), A11yError> {
        self.bridge.stage_initial(update);
        Ok(())
    }

    /// Forward every winit window event BEFORE app handling (required for
    /// AT plumbing).
    pub fn process_window_event(
        &mut self,
        window: &winit::window::Window,
        event: &winit::event::WindowEvent,
    ) {
        self.adapter.process_event(window, event);
    }

    /// Push a semantic update when (and only when) AT is active.
    pub fn push_snapshot(&mut self, update: TreeUpdate) {
        self.adapter.update_if_active(|| update);
    }

    /// Drain inbound AT action requests into UI behaviors.
    pub fn drain_actions(&mut self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
