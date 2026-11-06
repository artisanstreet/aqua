mod action;
mod bridge;
mod inspector;
mod tree;

#[cfg(windows)]
mod adapter;
#[cfg(windows)]
pub mod probe;

pub use accesskit;
pub use bridge::{A11yBridge, BridgeActionHandler, BridgeActivationHandler, Connection, WakeFn};
pub use inspector::{inspect, render_json, render_text, InspectorNode};
pub use tree::{build_tree_update, map_role, pack_node_id, unpack_node_id, A11yTree};

#[cfg(windows)]
pub use adapter::{AttachError, UiaAdapter, UiaRouter};
