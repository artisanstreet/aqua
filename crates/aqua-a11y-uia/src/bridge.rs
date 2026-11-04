use std::collections::VecDeque;
use std::sync::{Arc, Mutex, MutexGuard};

use accesskit::TreeUpdate;
use aqua_protocol::{A11yActionRequest, AccessTreeUpdate};

use crate::tree::A11yTree;

pub type WakeFn = Arc<dyn Fn() + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Connection {
    #[default]
    Dormant,
    Connected,
}

#[derive(Default)]
struct Shared {
    connection: Connection,
    actions: VecDeque<A11yActionRequest>,
    tree: A11yTree,
}

pub struct A11yBridge {
    shared: Mutex<Shared>,
    waker: WakeFn,
}

impl A11yBridge {
    pub fn new(waker: WakeFn) -> Arc<Self> {
        Arc::new(Self {
            shared: Mutex::new(Shared::default()),
            waker,
        })
    }

    fn shared(&self) -> MutexGuard<'_, Shared> {
        self.shared
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn connection(&self) -> Connection {
        self.shared().connection
    }

    pub fn connect(&self) {
        self.shared().connection = Connection::Connected;
        (self.waker)();
    }

    pub fn snapshot(&self, update: &AccessTreeUpdate) -> Option<TreeUpdate> {
        let mut shared = self.shared();
        match shared.connection {
            Connection::Dormant => None,
            Connection::Connected => {
                Some(shared.tree.build(update.root, update.focus, &update.nodes))
            }
        }
    }

    pub fn native_action(&self, request: &accesskit::ActionRequest) {
        let translated = self.shared().tree.translate(request);
        if let Some(translated) = translated {
            self.push_action(translated);
        }
    }

    pub fn push_action(&self, request: A11yActionRequest) {
        self.shared().actions.push_back(request);
        (self.waker)();
    }

    pub fn drain_actions(&self) -> Vec<A11yActionRequest> {
        self.shared().actions.drain(..).collect()
    }
}

pub struct BridgeActivationHandler {
    bridge: Arc<A11yBridge>,
}

impl BridgeActivationHandler {
    pub fn new(bridge: Arc<A11yBridge>) -> Self {
        Self { bridge }
    }
}

impl accesskit::ActivationHandler for BridgeActivationHandler {
    fn request_initial_tree(&mut self) -> Option<TreeUpdate> {
        self.bridge.connect();
        None
    }
}

pub struct BridgeActionHandler {
    bridge: Arc<A11yBridge>,
}

impl BridgeActionHandler {
    pub fn new(bridge: Arc<A11yBridge>) -> Self {
        Self { bridge }
    }
}

impl accesskit::ActionHandler for BridgeActionHandler {
    fn do_action(&mut self, request: accesskit::ActionRequest) {
        self.bridge.native_action(&request);
    }
}
