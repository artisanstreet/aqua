use aqua_protocol::chrome::{hit_test_chrome, ht, DragRects, HitInput, RectI};
use aqua_protocol::CaptionControl;

fn rect(left: i32, top: i32, right: i32, bottom: i32) -> RectI {
    RectI {
        left,
        top,
        right,
        bottom,
    }
}

fn rects() -> DragRects {
    DragRects {
        draggable: vec![rect(0, 0, 800, 40)],
        no_drag: vec![rect(100, 0, 150, 40)],
        caption_controls: vec![
            (CaptionControl::Minimize, rect(662, 0, 708, 40)),
            (CaptionControl::Maximize, rect(708, 0, 754, 40)),
            (CaptionControl::Close, rect(754, 0, 800, 40)),
        ],
    }
}

fn hit(point: (i32, i32), maximized: bool, resizable: bool, drag: &DragRects) -> i32 {
    hit_test_chrome(&HitInput {
        point,
        window: rect(0, 0, 800, 600),
        client_origin: (0, 0),
        border: 6,
        buttons: None,
        drag_band: 0,
        drag,
        maximized,
        resizable,
        minimizable: true,
    })
}

#[test]
fn frame_corners_and_edges_resolve_before_caption_content() {
    let drag = rects();
    assert_eq!(hit((2, 2), false, true, &drag), ht::TOPLEFT);
    assert_eq!(hit((797, 2), false, true, &drag), ht::TOPRIGHT);
    assert_eq!(hit((2, 597), false, true, &drag), ht::BOTTOMLEFT);
    assert_eq!(hit((797, 597), false, true, &drag), ht::BOTTOMRIGHT);
    assert_eq!(hit((2, 300), false, true, &drag), ht::LEFT);
    assert_eq!(hit((400, 597), false, true, &drag), ht::BOTTOM);
    assert_eq!(hit((300, 2), false, true, &drag), ht::TOP);
}

#[test]
fn caption_controls_holes_and_drag_regions() {
    let drag = rects();
    assert_eq!(hit((680, 20), false, true, &drag), ht::MINBUTTON);
    assert_eq!(hit((730, 20), false, true, &drag), ht::MAXBUTTON);
    assert_eq!(hit((780, 20), false, true, &drag), ht::CLOSE);
    assert_eq!(hit((120, 20), false, true, &drag), ht::CLIENT);
    assert_eq!(hit((300, 20), false, true, &drag), ht::CAPTION);
    assert_eq!(hit((300, 300), false, true, &drag), ht::CLIENT);
}

#[test]
fn maximized_and_fixed_windows_have_no_resize_frame() {
    let drag = rects();
    assert_eq!(hit((2, 300), true, true, &drag), ht::CLIENT);
    assert_eq!(hit((2, 300), false, false, &drag), ht::CLIENT);
    assert_eq!(hit((730, 20), false, false, &drag), ht::CAPTION);
}

#[test]
fn drag_band_owns_the_top_strip_when_there_are_no_caption_controls() {
    let drag = DragRects::default();
    let band = |point| {
        hit_test_chrome(&HitInput {
            point,
            window: rect(0, 0, 800, 600),
            client_origin: (0, 0),
            border: 6,
            buttons: None,
            drag_band: 32,
            drag: &drag,
            maximized: false,
            resizable: true,
            minimizable: true,
        })
    };
    assert_eq!(band((300, 3)), ht::CAPTION);
    assert_eq!(band((300, 20)), ht::CAPTION);
    assert_eq!(band((300, 100)), ht::CLIENT);
}
