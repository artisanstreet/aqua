use aqua_core::{Point, Rect, Transform2D};

#[derive(Clone, Copy)]
pub(crate) struct EmissionContext {
    pub scrolled: Point,
    pub clip: Option<Rect>,
    pub cull: Option<Rect>,
    pub transform: Transform2D,
}

pub struct SceneFragments {
    pub nodes: Vec<aqua_core::NodeId>,
    pub roots: Vec<(aqua_core::NodeId, std::ops::Range<usize>)>,
    pub items: Vec<(aqua_core::NodeId, aqua_core::DisplayItem)>,
}

#[derive(Default)]
pub(crate) struct TextIndex {
    pub candidates: std::collections::HashSet<aqua_core::NodeId>,
    pub viewport: std::collections::HashSet<aqua_core::NodeId>,
    pub selection: std::collections::HashSet<aqua_core::NodeId>,
    pub selection_changes: std::collections::HashSet<aqua_core::NodeId>,
    pub revision: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LayoutWork {
    pub passes: usize,
    pub measured: usize,
    pub committed: usize,
    pub regions: usize,
}
