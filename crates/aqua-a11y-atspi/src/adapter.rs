use std::sync::Arc;

use accesskit_unix::Adapter;
use aqua_a11y::accesskit::{Role, TreeUpdate};
use aqua_a11y::{
    A11yBridge, BridgeActionHandler, BridgeActivationHandler, BridgeDeactivationHandler,
    Connection, WakeFn,
};
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};

fn as_window(mut snapshot: TreeUpdate, name: &str) -> TreeUpdate {
    let root = snapshot.tree.as_ref().map(|tree| tree.root);
    for (id, node) in &mut snapshot.nodes {
        if Some(*id) == root {
            node.set_role(Role::Window);
            if node.label().is_none() && !name.is_empty() {
                node.set_label(name);
            }
        }
    }
    snapshot
}

pub struct AtspiAdapter {
    adapter: Adapter,
    bridge: Arc<A11yBridge>,
    name: String,
}

impl AtspiAdapter {
    pub fn attach(waker: WakeFn) -> Self {
        let bridge = A11yBridge::new(waker);
        let adapter = Adapter::new(
            BridgeActivationHandler::new(bridge.clone()),
            BridgeActionHandler::new(bridge.clone()),
            BridgeDeactivationHandler::new(bridge.clone()),
        );
        Self {
            adapter,
            bridge,
            name: String::new(),
        }
    }

    pub fn connection(&self) -> Connection {
        self.bridge.connection()
    }

    pub fn epoch(&self) -> u64 {
        self.bridge.epoch()
    }

    pub fn set_window_name(&mut self, name: &str) {
        if self.name != name {
            self.name = name.to_owned();
        }
    }

    pub fn apply(&mut self, update: &AccessTreeUpdate) {
        if let Some(snapshot) = self.bridge.snapshot(update) {
            let name = self.name.as_str();
            self.adapter.update_if_active(|| as_window(snapshot, name));
        }
    }

    pub fn focus(&mut self, focused: bool) {
        self.adapter.update_window_focus_state(focused);
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.bridge.drain_actions()
    }
}
