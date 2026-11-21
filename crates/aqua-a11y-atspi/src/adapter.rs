use std::sync::Arc;

use accesskit_unix::Adapter;
use aqua_a11y::accesskit::{Role, TreeUpdate};
use aqua_a11y::{
    A11yBridge, BridgeActionHandler, BridgeActivationHandler, BridgeDeactivationHandler,
    Connection, WakeFn,
};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};

fn as_window(mut snapshot: TreeUpdate) -> TreeUpdate {
    let root = snapshot.tree.as_ref().map(|tree| tree.root);
    for (id, node) in &mut snapshot.nodes {
        if Some(*id) == root {
            node.set_role(Role::Window);
        }
    }
    snapshot
}

pub struct AtspiAdapter {
    adapter: Adapter,
    bridge: Arc<A11yBridge>,
}

impl AtspiAdapter {
    pub fn attach(waker: WakeFn) -> Self {
        let bridge = A11yBridge::new(waker);
        let adapter = Adapter::new(
            BridgeActivationHandler::new(bridge.clone()),
            BridgeActionHandler::new(bridge.clone()),
            BridgeDeactivationHandler::new(bridge.clone()),
        );
        Self { adapter, bridge }
    }

    pub fn connection(&self) -> Connection {
        self.bridge.connection()
    }

    pub fn epoch(&self) -> u64 {
        self.bridge.epoch()
    }

    pub fn apply(&mut self, update: &AccessTreeUpdate) {
        if let Some(snapshot) = self.bridge.snapshot(update) {
            self.adapter.update_if_active(|| as_window(snapshot));
        }
    }

    pub fn focus(&mut self, focused: bool) {
        self.adapter.update_window_focus_state(focused);
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
