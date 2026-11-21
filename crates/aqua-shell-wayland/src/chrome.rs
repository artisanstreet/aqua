use crate::decoration::Frame;
use crate::state::WindowState;
use aqua_core::CursorIcon;
use aqua_protocol::chrome::{hit_test_chrome, ht, HitInput, RectI};
use aqua_protocol::CaptionControl;
use wayland_protocols::xdg::shell::client::xdg_toplevel::ResizeEdge;

pub(crate) const RESIZE_BORDER: f64 = 6.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Hit {
    Client,
    Caption,
    Edge(ResizeEdge),
    Control(CaptionControl),
}

impl Hit {
    pub(crate) fn hover(self) -> Option<CaptionControl> {
        match self {
            Hit::Control(control) => Some(control),
            _ => None,
        }
    }

    pub(crate) fn cursor(self, client: CursorIcon) -> CursorIcon {
        match self {
            Hit::Edge(ResizeEdge::Left | ResizeEdge::Right) => CursorIcon::EwResize,
            Hit::Edge(ResizeEdge::Top | ResizeEdge::Bottom) => CursorIcon::NsResize,
            Hit::Edge(ResizeEdge::TopLeft | ResizeEdge::BottomRight) => CursorIcon::NwseResize,
            Hit::Edge(_) => CursorIcon::NeswResize,
            Hit::Caption | Hit::Control(_) => CursorIcon::Default,
            Hit::Client => client,
        }
    }
}

fn from_code(code: i32) -> Hit {
    match code {
        ht::CAPTION => Hit::Caption,
        ht::MINBUTTON => Hit::Control(CaptionControl::Minimize),
        ht::MAXBUTTON => Hit::Control(CaptionControl::Maximize),
        ht::CLOSE => Hit::Control(CaptionControl::Close),
        ht::LEFT => Hit::Edge(ResizeEdge::Left),
        ht::RIGHT => Hit::Edge(ResizeEdge::Right),
        ht::TOP => Hit::Edge(ResizeEdge::Top),
        ht::BOTTOM => Hit::Edge(ResizeEdge::Bottom),
        ht::TOPLEFT => Hit::Edge(ResizeEdge::TopLeft),
        ht::TOPRIGHT => Hit::Edge(ResizeEdge::TopRight),
        ht::BOTTOMLEFT => Hit::Edge(ResizeEdge::BottomLeft),
        ht::BOTTOMRIGHT => Hit::Edge(ResizeEdge::BottomRight),
        _ => Hit::Client,
    }
}

pub(crate) fn hit(window: &WindowState, at: (f64, f64)) -> Hit {
    let Frame::Client { band } = window.frame else {
        return Hit::Client;
    };
    let scale = window.scale.value;
    let (width, height) = window.physical();
    let constrained = window.states.constrained();
    let border = match window.spec.resizable && !constrained {
        true => (RESIZE_BORDER * scale).round() as u32,
        false => 0,
    };
    from_code(hit_test_chrome(&HitInput {
        point: ((at.0 * scale).floor() as i32, (at.1 * scale).floor() as i32),
        window: RectI {
            left: 0,
            top: 0,
            right: width as i32,
            bottom: height as i32,
        },
        client_origin: (0, 0),
        border,
        buttons: None,
        drag_band: band.map_or(0, |band| (band * scale).round() as u32),
        drag: &window.rects,
        maximized: constrained,
        resizable: window.spec.resizable,
        minimizable: window.spec.minimizable,
    }))
}
