use std::sync::Arc;

use aqua::render::vector::{gradient_axis, rotated_svg, rotation_steps, VectorCache};
use aqua_core::geometry::Rect;
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
use aqua_core::{GpuiBackground, GpuiHsla, Point, Transform2D};

#[test]
fn rotation_quantizes_to_whole_steps_and_wraps() {
    assert_eq!(rotation_steps(0.0), 0);
    assert_eq!(rotation_steps(std::f32::consts::TAU), 0);
    assert_eq!(rotation_steps(std::f32::consts::FRAC_PI_2), 30);
    assert_eq!(rotation_steps(-std::f32::consts::FRAC_PI_2), 90);
    assert_eq!(rotation_steps(f32::NAN), 0);
}

#[test]
fn rotated_svg_wraps_the_original_around_its_view_box_centre() {
    let source = "<?xml version=\"1.0\"?><svg viewBox=\"0 0 16 8\" xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>";
    let wrapped = rotated_svg(source, 30);
    assert!(wrapped.contains("rotate(90 8 4)"));
    assert!(wrapped.contains("viewBox=\"0 0 16 8\""));
    assert!(wrapped.contains("<rect"));
    assert!(!wrapped.contains("<?xml"));
}

#[test]
fn rotated_svg_falls_back_to_width_and_height() {
    let wrapped = rotated_svg("<svg width=\"20px\" height=\"10\"></svg>", 60);
    assert!(wrapped.contains("rotate(180 10 5)"));
}

#[test]
fn gradient_axes_follow_css_angles() {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 50.0,
    };
    let (start, end) = gradient_axis(bounds, 0.0);
    assert!((start.x - 50.0).abs() < 1e-3 && (start.y - 50.0).abs() < 1e-3);
    assert!((end.x - 50.0).abs() < 1e-3 && end.y.abs() < 1e-3);
    let (start, end) = gradient_axis(bounds, 90.0);
    assert!(start.x.abs() < 1e-3 && (end.x - 100.0).abs() < 1e-3);
}

fn triangle(origin: Point) -> GpuiPath {
    GpuiPath {
        commands_local: vec![
            PathCommand::Move(Point::new(0.0, 0.0)),
            PathCommand::Line(Point::new(10.0, 0.0)),
            PathCommand::Line(Point::new(5.0, 8.0)),
            PathCommand::Close,
        ],
        origin_window_logical: origin,
        element_transform: Transform2D::IDENTITY,
        style: GpuiPathStyle::Fill {
            rule: FillRule::NonZero,
        },
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
    }
}

fn drawing_of(raster: &aqua::render::vector::PathRaster) -> Arc<aqua_core::vector::Drawing> {
    match &raster.source {
        aqua_core::vector::VectorSource::Drawing(drawing) => drawing.clone(),
        aqua_core::vector::VectorSource::Svg(_) => panic!("path rasters are drawings"),
    }
}

#[test]
fn translating_a_path_by_whole_pixels_reuses_the_same_drawing() {
    let mut cache = VectorCache::default();
    let device = Transform2D::scale(2.0, 2.0);
    let first = cache
        .path(&triangle(Point::new(10.0, 10.0)), device)
        .unwrap();
    let moved = cache
        .path(&triangle(Point::new(40.0, 25.0)), device)
        .unwrap();
    assert!(Arc::ptr_eq(&drawing_of(&first), &drawing_of(&moved)));
    assert_eq!(moved.placement.x - first.placement.x, 60.0);
    assert_eq!(moved.placement.y - first.placement.y, 30.0);
}

#[test]
fn subpixel_translation_gets_its_own_drawing() {
    let mut cache = VectorCache::default();
    let device = Transform2D::IDENTITY;
    let first = cache
        .path(&triangle(Point::new(10.0, 10.0)), device)
        .unwrap();
    let shifted = cache
        .path(&triangle(Point::new(10.5, 10.0)), device)
        .unwrap();
    assert!(!Arc::ptr_eq(&drawing_of(&first), &drawing_of(&shifted)));
}

#[test]
fn mixed_path_bounds_include_controls_and_scaled_arc_reach() {
    let mut path = triangle(Point::new(3.25, 7.5));
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(-2.0, 3.0)),
        PathCommand::Quadratic {
            control: Point::new(-8.0, -4.0),
            to: Point::new(6.0, 2.0),
        },
        PathCommand::Cubic {
            first: Point::new(20.0, -6.0),
            second: Point::new(9.0, 18.0),
            to: Point::new(5.0, 5.0),
        },
        PathCommand::Arc {
            radii: Point::new(2.0, 4.0),
            rotation_radians: 0.0,
            large: false,
            sweep: true,
            to: Point::new(12.0, 10.0),
        },
        PathCommand::Close,
    ];
    let mut cache = VectorCache::default();
    let raster = cache.path(&path, Transform2D::scale(2.0, 2.0)).unwrap();
    assert_eq!(
        raster.placement,
        Rect {
            x: -19.0,
            y: -6.0,
            w: 75.0,
            h: 66.0
        }
    );
    assert_eq!((raster.width, raster.height), (75, 66));
    let drawing = drawing_of(&raster);
    assert_eq!(
        drawing.paths[0].commands[0],
        PathCommand::Move(Point::new(25.5, 21.0))
    );
    assert_eq!(
        drawing.paths[0].commands[4],
        PathCommand::Arc {
            radii: Point::new(4.0, 8.0),
            rotation_radians: 0.0,
            large: false,
            sweep: true,
            to: Point::new(49.5, 41.0),
        }
    );
}

#[test]
fn svg_sources_are_shared_per_tint_and_rotation() {
    let mut cache = VectorCache::default();
    let svg: Arc<str> = "<svg viewBox=\"0 0 4 4\"></svg>".into();
    let plain = cache.svg(&svg, None, 0);
    assert_eq!(plain, cache.svg(&svg, None, 0));
    assert_ne!(plain, cache.svg(&svg, None, 5));
}
