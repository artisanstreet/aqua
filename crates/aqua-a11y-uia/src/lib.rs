mod action;
mod bridge;
mod tree;

#[cfg(windows)]
mod adapter;

pub use bridge::{A11yBridge, BridgeActionHandler, BridgeActivationHandler, Connection, WakeFn};
pub use tree::{A11yTree, build_tree_update, map_role, pack_node_id, unpack_node_id};

#[cfg(windows)]
pub use adapter::{AttachError, UiaAdapter};
