use aqua_platform::chrome::{RectI, ht};
use aqua_protocol::{CaptionControl, CaptionRegion, DragRegions};
use aqua_shell_win32::regions::{Geometry, drag_rects, hit_test};

fn regions() -> DragRegions {
    DragRegions {
        draggable: vec![(0.0, 0.0, 800.0, 40.0)],
        no_drag: vec![(100.0, 0.0, 50.0, 40.0)],
        caption_controls: vec![
            CaptionRegion {
                control: CaptionControl::Minimize,
                bounds: (662.0, 0.0, 46.0, 40.0),
            },
            CaptionRegion {
                control: CaptionControl::Maximize,
                bounds: (708.0, 0.0, 46.0, 40.0),
            },
            CaptionRegion {
                control: CaptionControl::Close,
                bounds: (754.0, 0.0, 46.0, 40.0),
            },
        ],
    }
}

fn geometry(point: (i32, i32)) -> Geometry {
    Geometry {
        point,
        window: RectI {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 700,
        },
        client_origin: (0, 0),
        border: 8,
        maximized: false,
        resizable: true,
        minimizable: true,
    }
}

#[test]
fn logical_regions_scale_outward_to_physical_pixels() {
    let rects = drag_rects(&regions(), 1.25);
    assert_eq!(
        rects.draggable[0],
        RectI {
            left: 0,
            top: 0,
            right: 1000,
            bottom: 50
        }
    );
    assert_eq!(
        rects.no_drag[0],
        RectI {
            left: 125,
            top: 0,
            right: 188,
            bottom: 50
        }
    );
    assert_eq!(rects.caption_controls[2].0, CaptionControl::Close);
}

#[test]
fn hit_test_resolves_caption_controls_drag_and_edges() {
    let rects = drag_rects(&regions(), 1.25);
    assert_eq!(hit_test(&geometry((900, 20)), &rects), ht::MAXBUTTON);
    assert_eq!(hit_test(&geometry((960, 20)), &rects), ht::CLOSE);
    assert_eq!(hit_test(&geometry((500, 20)), &rects), ht::CAPTION);
    assert_eq!(hit_test(&geometry((150, 20)), &rects), ht::CLIENT);
    assert_eq!(hit_test(&geometry((500, 400)), &rects), ht::CLIENT);
    assert_eq!(hit_test(&geometry((2, 400)), &rects), ht::LEFT);
    assert_eq!(hit_test(&geometry((998, 698)), &rects), ht::BOTTOMRIGHT);
}
