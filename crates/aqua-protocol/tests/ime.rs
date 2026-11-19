use aqua_core::{Point, Rect};
use aqua_protocol::{ImeGeometry, ImeHit, ImeLine, ImeStop, Utf16Range};

fn line(top: f32, stops: &[(u32, f32)]) -> ImeLine {
    ImeLine {
        top,
        bottom: top + 20.0,
        stops: stops
            .iter()
            .map(|(offset, x)| ImeStop {
                offset: *offset,
                x: *x,
            })
            .collect(),
    }
}

fn wrapped() -> ImeGeometry {
    ImeGeometry {
        clip: Rect {
            x: 0.0,
            y: 0.0,
            w: 100.0,
            h: 30.0,
        },
        lines: vec![
            line(0.0, &[(0, 0.0), (1, 10.0), (2, 20.0), (3, 30.0)]),
            line(20.0, &[(3, 0.0), (4, 10.0), (5, 20.0)]),
        ],
    }
}

#[test]
fn a_range_spanning_a_soft_wrap_covers_both_lines() {
    let bounds = wrapped().range_bounds(Utf16Range::new(2, 4)).unwrap();
    assert_eq!(bounds.rect, Rect::from_edges(0.0, 0.0, 30.0, 40.0));
    assert!(bounds.clipped);
}

#[test]
fn a_range_ending_at_a_wrap_stays_on_its_line() {
    let bounds = wrapped().range_bounds(Utf16Range::new(1, 3)).unwrap();
    assert_eq!(bounds.rect, Rect::from_edges(10.0, 0.0, 30.0, 20.0));
    assert!(!bounds.clipped);
}

#[test]
fn a_caret_is_a_zero_width_rect_at_its_offset() {
    let bounds = wrapped().range_bounds(Utf16Range::new(4, 4)).unwrap();
    assert_eq!(bounds.rect, Rect::from_edges(10.0, 20.0, 10.0, 40.0));
}

#[test]
fn right_to_left_characters_span_their_visual_extent() {
    let geometry = ImeGeometry {
        lines: vec![line(0.0, &[(0, 30.0), (1, 20.0), (2, 10.0), (3, 0.0)])],
        ..wrapped()
    };
    let bounds = geometry.range_bounds(Utf16Range::new(0, 1)).unwrap();
    assert_eq!(bounds.rect, Rect::from_edges(20.0, 0.0, 30.0, 20.0));
    assert_eq!(
        geometry.hit(Point::new(24.0, 5.0)),
        Some(ImeHit {
            nearest: 1,
            inside: Some(0)
        })
    );
}

#[test]
fn offsets_outside_the_laid_out_window_have_no_geometry() {
    assert_eq!(wrapped().range_bounds(Utf16Range::new(7, 9)), None);
    assert_eq!(ImeGeometry::UNAVAILABLE.hit(Point::new(1.0, 1.0)), None);
}

#[test]
fn points_beside_the_text_hit_only_the_nearest_boundary() {
    let geometry = wrapped();
    assert_eq!(
        geometry.hit(Point::new(14.0, 25.0)),
        Some(ImeHit {
            nearest: 4,
            inside: Some(4)
        })
    );
    assert_eq!(
        geometry.hit(Point::new(80.0, 5.0)),
        Some(ImeHit {
            nearest: 3,
            inside: None
        })
    );
    assert_eq!(geometry.hit(Point::new(5.0, 90.0)).unwrap().inside, None);
}
