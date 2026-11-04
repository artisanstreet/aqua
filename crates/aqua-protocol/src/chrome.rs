#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptionControl {
    Minimize,
    Maximize,
    Close,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CaptionRegion {
    pub control: CaptionControl,
    pub bounds: (f64, f64, f64, f64),
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DragRegions {
    pub draggable: Vec<(f64, f64, f64, f64)>,
    pub no_drag: Vec<(f64, f64, f64, f64)>,
    pub caption_controls: Vec<CaptionRegion>,
}
