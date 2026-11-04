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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RectI {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl RectI {
    pub fn contains(self, x: i32, y: i32) -> bool {
        x >= self.left && x < self.right && y >= self.top && y < self.bottom
    }

    pub fn width(self) -> i32 {
        (self.right - self.left).max(0)
    }
}

#[derive(Debug, Clone, Default)]
pub struct DragRects {
    pub draggable: Vec<RectI>,
    pub no_drag: Vec<RectI>,
    pub caption_controls: Vec<(CaptionControl, RectI)>,
}

#[derive(Debug, Clone)]
pub struct HitInput<'a> {
    pub point: (i32, i32),
    pub window: RectI,
    pub client_origin: (i32, i32),
    pub border: u32,
    pub buttons: Option<RectI>,
    pub drag_band: u32,
    pub drag: &'a DragRects,
    pub maximized: bool,
    pub resizable: bool,
    pub minimizable: bool,
}

pub mod ht {
    pub const NOWHERE: i32 = 0;
    pub const CLIENT: i32 = 1;
    pub const CAPTION: i32 = 2;
    pub const MINBUTTON: i32 = 8;
    pub const MAXBUTTON: i32 = 9;
    pub const LEFT: i32 = 10;
    pub const RIGHT: i32 = 11;
    pub const TOP: i32 = 12;
    pub const TOPLEFT: i32 = 13;
    pub const TOPRIGHT: i32 = 14;
    pub const BOTTOM: i32 = 15;
    pub const BOTTOMLEFT: i32 = 16;
    pub const BOTTOMRIGHT: i32 = 17;
    pub const CLOSE: i32 = 20;
}

fn resize_edge(input: &HitInput<'_>, lx: i32, ly: i32, width: i32, height: i32) -> Option<i32> {
    let border = input.border as i32;
    let active = !input.maximized && input.resizable && border > 0;
    let left = lx < border;
    let right = lx >= width - border;
    let top = ly < border;
    let bottom = ly >= height - border;
    let top_owned_by_frame = input.drag_band == 0 || !input.drag.caption_controls.is_empty();
    active
        .then_some(match (top, bottom, left, right) {
            (true, _, true, _) => Some(ht::TOPLEFT),
            (true, _, _, true) => Some(ht::TOPRIGHT),
            (_, true, true, _) => Some(ht::BOTTOMLEFT),
            (_, true, _, true) => Some(ht::BOTTOMRIGHT),
            (_, _, true, _) => Some(ht::LEFT),
            (_, _, _, true) => Some(ht::RIGHT),
            (_, true, _, _) => Some(ht::BOTTOM),
            (true, ..) if top_owned_by_frame => Some(ht::TOP),
            _ => None,
        })
        .flatten()
}

pub fn hit_test_chrome(input: &HitInput<'_>) -> i32 {
    let (x, y) = input.point;
    let window = input.window;
    let lx = x - window.left;
    let ly = y - window.top;
    let cx = x - input.client_origin.0;
    let cy = y - input.client_origin.1;
    let width = window.width();
    let height = (window.bottom - window.top).max(0);

    if let Some(edge) = resize_edge(input, lx, ly, width, height) {
        return edge;
    }

    if let Some((control, _)) = input
        .drag
        .caption_controls
        .iter()
        .find(|(_, rect)| rect.contains(cx, cy))
    {
        return match control {
            CaptionControl::Minimize if input.minimizable => ht::MINBUTTON,
            CaptionControl::Maximize if input.resizable => ht::MAXBUTTON,
            CaptionControl::Close => ht::CLOSE,
            CaptionControl::Minimize | CaptionControl::Maximize => ht::CAPTION,
        };
    }

    let in_button_strip = input.buttons.is_some_and(|strip| strip.contains(x, y));
    let in_hole = input.drag.no_drag.iter().any(|r| r.contains(cx, cy));
    let in_band =
        input.drag_band > 0 && ly >= 0 && ly < input.drag_band as i32 && lx >= 0 && lx < width;
    let in_draggable = input.drag.draggable.iter().any(|r| r.contains(cx, cy));
    match (in_button_strip || in_hole, in_band || in_draggable) {
        (true, _) => ht::CLIENT,
        (false, true) => ht::CAPTION,
        (false, false) => ht::CLIENT,
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ClientChromeHit {
    Drag,
    Caption(CaptionControl),
    Client,
}

pub fn client_chrome_hit(regions: &DragRegions, x: f64, y: f64) -> ClientChromeHit {
    let contains = |&(left, top, width, height): &(f64, f64, f64, f64)| {
        x >= left && x < left + width && y >= top && y < top + height
    };
    regions
        .caption_controls
        .iter()
        .find(|region| contains(&region.bounds))
        .map(|region| ClientChromeHit::Caption(region.control))
        .or_else(|| {
            (!regions.no_drag.iter().any(contains) && regions.draggable.iter().any(contains))
                .then_some(ClientChromeHit::Drag)
        })
        .unwrap_or(ClientChromeHit::Client)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChromeError {
    Unsupported(&'static str),
    NullHandle,
    InvalidOptions(&'static str),
    WrongThread,
}

impl std::fmt::Display for ChromeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChromeError::Unsupported(reason) => write!(f, "window overlay unsupported: {reason}"),
            ChromeError::NullHandle => write!(f, "null window handle"),
            ChromeError::InvalidOptions(reason) => write!(f, "invalid chrome options: {reason}"),
            ChromeError::WrongThread => {
                write!(f, "chrome must be installed on the window's GUI thread")
            }
        }
    }
}

impl std::error::Error for ChromeError {}
