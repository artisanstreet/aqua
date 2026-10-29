use aqua_core::{NodeId, Rect, SemanticNode};

#[derive(Debug, Clone)]
pub struct A11yNode {
    pub semantic: SemanticNode,
    pub bounds: Option<Rect>,
    pub children: Vec<NodeId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct A11yActionRequest {
    pub node: NodeId,
    pub action: A11yAction,
    pub data: A11yActionData,
}

#[derive(Debug, Clone, PartialEq)]
pub enum A11yAction {
    Press,
    Focus,
    Expand,
    Collapse,
    Increment,
    Decrement,
    Other(&'static str),
}

#[derive(Debug, Clone, PartialEq)]
pub enum A11yActionData {
    None,
    Value(String),
    Numeric(f64),
    TextSelection {
        anchor_node: NodeId,
        anchor_index: usize,
        focus_node: NodeId,
        focus_index: usize,
    },
    Point {
        x: f64,
        y: f64,
    },
    ScrollOffset {
        x: f64,
        y: f64,
    },
    Custom(i32),
}
