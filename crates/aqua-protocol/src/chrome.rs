#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragRegions {
    pub draggable: Vec<(f64, f64, f64, f64)>,
    pub no_drag: Vec<(f64, f64, f64, f64)>,
}
