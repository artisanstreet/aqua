#[cfg(windows)]
mod adapter;
#[cfg(windows)]
pub mod probe;

pub use aqua_a11y::{
    accesskit, build_tree_update, inspect, map_role, pack_node_id, render_json, render_text,
    unpack_node_id, A11yBridge, A11yTree, BridgeActionHandler, BridgeActivationHandler, Connection,
    InspectorNode, WakeFn,
};

#[cfg(windows)]
pub use adapter::{AttachError, UiaAdapter, UiaRouter};
