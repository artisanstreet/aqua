use aqua_protocol::chrome::{ht, DragRects, RectI};
use aqua_protocol::{CaptionControl, CaptionRegion, DragRegions};
use aqua_shell_win32::regions::{drag_rects, hit_test, Geometry};

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

#[test]
fn resize_corners_extend_along_both_edges_at_each_scale() {
    for border in [8, 10, 12, 16] {
        let mut geometry = geometry((0, 0));
        geometry.border = border;
        geometry.window = RectI {
            left: -1200,
            top: -800,
            right: -200,
            bottom: -100,
        };
        geometry.client_origin = (-1200, -800);
        let edge = border as i32 - 1;
        let corner = border as i32 * 2 - 1;
        let rects = DragRects::default();
        for (x, y, expected) in [
            (corner, edge, ht::TOPLEFT),
            (edge, corner, ht::TOPLEFT),
            (999 - corner, edge, ht::TOPRIGHT),
            (999 - edge, corner, ht::TOPRIGHT),
            (corner, 699 - edge, ht::BOTTOMLEFT),
            (edge, 699 - corner, ht::BOTTOMLEFT),
            (999 - corner, 699 - edge, ht::BOTTOMRIGHT),
            (999 - edge, 699 - corner, ht::BOTTOMRIGHT),
            (500, edge, ht::TOP),
            (edge, 350, ht::LEFT),
            (999 - edge, 350, ht::RIGHT),
            (500, 699 - edge, ht::BOTTOM),
            (corner, corner, ht::CLIENT),
        ] {
            geometry.point = (-1200 + x, -800 + y);
            assert_eq!(hit_test(&geometry, &rects), expected);
        }
        geometry.point = (-1200 + edge, -800 + corner);
        geometry.maximized = true;
        assert_eq!(hit_test(&geometry, &rects), ht::CLIENT);
        geometry.maximized = false;
        geometry.resizable = false;
        assert_eq!(hit_test(&geometry, &rects), ht::CLIENT);
    }
}
