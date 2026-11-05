use aqua_protocol::DragRegions;
use aqua_protocol::chrome::{DragRects, HitInput, RectI, hit_test_chrome};

pub use aqua_protocol::chrome::ht;

fn to_rect(scale: f64, (left, top, width, height): (f64, f64, f64, f64)) -> RectI {
    RectI {
        left: (left * scale).floor() as i32,
        top: (top * scale).floor() as i32,
        right: ((left + width) * scale).ceil() as i32,
        bottom: ((top + height) * scale).ceil() as i32,
    }
}

pub fn drag_rects(regions: &DragRegions, scale: f64) -> DragRects {
    DragRects {
        draggable: regions
            .draggable
            .iter()
            .map(|r| to_rect(scale, *r))
            .collect(),
        no_drag: regions.no_drag.iter().map(|r| to_rect(scale, *r)).collect(),
        caption_controls: regions
            .caption_controls
            .iter()
            .map(|c| (c.control, to_rect(scale, c.bounds)))
            .collect(),
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Geometry {
    pub point: (i32, i32),
    pub window: RectI,
    pub client_origin: (i32, i32),
    pub border: u32,
    pub maximized: bool,
    pub resizable: bool,
    pub minimizable: bool,
}

pub fn hit_test(geometry: &Geometry, rects: &DragRects) -> i32 {
    hit_test_chrome(&HitInput {
        point: geometry.point,
        window: geometry.window,
        client_origin: geometry.client_origin,
        border: geometry.border,
        buttons: None,
        drag_band: 0,
        drag: rects,
        maximized: geometry.maximized,
        resizable: geometry.resizable,
        minimizable: geometry.minimizable,
    })
}
