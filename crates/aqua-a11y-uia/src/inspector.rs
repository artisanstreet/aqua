use std::collections::HashMap;
use std::fmt::Write;

use accesskit::{Action, Node, NodeId as AkNodeId, Toggled};
use aqua_protocol::AccessTreeUpdate;

use crate::tree::build_tree_update;

const ACTIONS: [(Action, &str); 8] = [
    (Action::Click, "click"),
    (Action::Focus, "focus"),
    (Action::SetValue, "set-value"),
    (Action::ReplaceSelectedText, "replace-selection"),
    (Action::Expand, "expand"),
    (Action::Collapse, "collapse"),
    (Action::Increment, "increment"),
    (Action::Decrement, "decrement"),
];

#[derive(Debug, Clone, PartialEq)]
pub struct InspectorNode {
    pub id: u64,
    pub role: String,
    pub label: Option<String>,
    pub value: Option<String>,
    pub description: Option<String>,
    pub toggled: Option<String>,
    pub expanded: Option<bool>,
    pub selected: Option<bool>,
    pub disabled: bool,
    pub focused: bool,
    pub numeric: Option<f64>,
    pub bounds: Option<[f64; 4]>,
    pub actions: Vec<&'static str>,
    pub children: Vec<InspectorNode>,
}

fn convert(
    id: AkNodeId,
    nodes: &HashMap<AkNodeId, &Node>,
    focus: AkNodeId,
    visited: &mut Vec<AkNodeId>,
) -> Option<InspectorNode> {
    let node = nodes.get(&id)?;
    if visited.contains(&id) {
        return None;
    }
    visited.push(id);
    let children = node
        .children()
        .iter()
        .filter_map(|child| convert(*child, nodes, focus, visited))
        .collect();
    Some(InspectorNode {
        id: id.0,
        role: format!("{:?}", node.role()),
        label: node.label().map(str::to_owned),
        value: node.value().map(str::to_owned),
        description: node.description().map(str::to_owned),
        toggled: node.toggled().map(|toggled| {
            match toggled {
                Toggled::False => "false",
                Toggled::True => "true",
                Toggled::Mixed => "mixed",
            }
            .to_owned()
        }),
        expanded: node.is_expanded(),
        selected: node.is_selected(),
        disabled: node.is_disabled(),
        focused: id == focus,
        numeric: node.numeric_value(),
        bounds: node.bounds().map(|r| [r.x0, r.y0, r.x1, r.y1]),
        actions: ACTIONS
            .iter()
            .filter(|(action, _)| node.supports_action(*action))
            .map(|(_, name)| *name)
            .collect(),
        children,
    })
}

pub fn inspect(update: &AccessTreeUpdate) -> Option<InspectorNode> {
    let built = build_tree_update(update.root, update.focus, &update.nodes);
    let nodes: HashMap<AkNodeId, &Node> =
        built.nodes.iter().map(|(id, node)| (*id, node)).collect();
    let root = built.tree.as_ref()?.root;
    convert(root, &nodes, built.focus, &mut Vec::new())
}

pub fn render_text(root: &InspectorNode) -> String {
    let mut out = String::new();
    write_node(&mut out, root, 0);
    out
}

fn write_node(out: &mut String, node: &InspectorNode, depth: usize) {
    let _ = write!(out, "{:indent$}{}", "", node.role, indent = depth * 2);
    if let Some(label) = &node.label {
        let _ = write!(out, " {label:?}");
    }
    if let Some(value) = &node.value {
        let _ = write!(out, " value={value:?}");
    }
    if let Some(toggled) = &node.toggled {
        let _ = write!(out, " toggled={toggled}");
    }
    if let Some(expanded) = node.expanded {
        let _ = write!(out, " expanded={expanded}");
    }
    if let Some(numeric) = node.numeric {
        let _ = write!(out, " numeric={numeric}");
    }
    if let Some([x0, y0, x1, y1]) = node.bounds {
        let _ = write!(out, " [{x0:.0},{y0:.0},{x1:.0},{y1:.0}]");
    }
    if node.focused {
        out.push_str(" focused");
    }
    if node.disabled {
        out.push_str(" disabled");
    }
    if !node.actions.is_empty() {
        let _ = write!(out, " actions={}", node.actions.join("|"));
    }
    out.push('\n');
    for child in &node.children {
        write_node(out, child, depth + 1);
    }
}
