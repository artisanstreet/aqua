use aqua_core::image::{ImageFit, ImageFitError, ImagePlacement};
use aqua_core::{Point, Rect, Size};

#[test]
fn fitting_crops_preserves_aspect_and_resolves_alignment() {
    let bounds = Rect {
        x: 10.0,
        y: 20.0,
        w: 100.0,
        h: 100.0,
    };
    let source = Rect {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
    };
    let centered = Point::new(0.5, 0.5);
    let contained = ImagePlacement::resolve(
        bounds,
        Size::new(200.0, 100.0),
        ImageFit::Contain,
        centered,
        source,
    )
    .unwrap();
    assert_eq!(
        contained.destination,
        Rect {
            x: 10.0,
            y: 45.0,
            w: 100.0,
            h: 50.0
        }
    );
    let covered = ImagePlacement::resolve(
        bounds,
        Size::new(200.0, 100.0),
        ImageFit::Cover,
        Point::new(1.0, 0.0),
        source,
    )
    .unwrap();
    assert_eq!(
        covered.destination,
        Rect {
            x: -90.0,
            y: 20.0,
            w: 200.0,
            h: 100.0
        }
    );
    let cropped = Rect {
        x: 0.5,
        y: 0.0,
        w: 0.5,
        h: 1.0,
    };
    let placement = ImagePlacement::resolve(
        bounds,
        Size::new(200.0, 100.0),
        ImageFit::Contain,
        centered,
        cropped,
    )
    .unwrap();
    assert_eq!(placement.destination, bounds);
    assert_eq!(placement.source, cropped);
    assert_eq!(
        ImagePlacement::resolve(
            bounds,
            Size::new(10.0, 10.0),
            ImageFit::ScaleDown,
            centered,
            source
        )
        .unwrap()
        .destination
        .w,
        10.0
    );
    assert_eq!(
        ImagePlacement::resolve(
            bounds,
            Size::new(10.0, 10.0),
            ImageFit::Fill,
            centered,
            source
        )
        .unwrap()
        .destination,
        bounds
    );
    assert_eq!(
        ImagePlacement::resolve(
            bounds,
            Size::new(10.0, 10.0),
            ImageFit::Cover,
            centered,
            Rect {
                x: 0.5,
                w: 1.0,
                ..source
            }
        ),
        Err(ImageFitError::InvalidSource)
    );
}
