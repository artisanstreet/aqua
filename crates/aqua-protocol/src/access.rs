use aqua_core::{NodeId, Rect, SemanticNode};

#[derive(Debug, Clone)]
pub struct A11yNode {
    pub text: Option<A11yText>,
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
    SetValue,
    ReplaceSelectedText,
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
    TextByteSelection {
        anchor_node: NodeId,
        anchor_byte: usize,
        focus_node: NodeId,
        focus_byte: usize,
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

#[derive(Debug, Clone)]
pub struct A11yText {
    pub runs: Vec<A11yTextRun>,
    pub selection: Option<(usize, usize)>,
    pub selection_enabled: bool,
}

#[derive(Debug, Clone)]
pub struct A11yTextRun {
    pub text: String,
    pub byte_start: usize,
    pub bounds: Rect,
    pub character_lengths: Vec<u8>,
    pub word_starts: Vec<u8>,
    pub character_positions: Vec<f32>,
    pub character_widths: Vec<f32>,
    pub rtl: bool,
}
