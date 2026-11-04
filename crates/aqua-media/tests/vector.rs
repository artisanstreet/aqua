use std::sync::Arc;

use aqua_core::vector::{
    Brush, Drawing, FillRule, GradientStop, PaintedPath, PathCommand, VectorSource,
};
use aqua_core::{Oklch, Point, Rect, Transform2D};
use aqua_media::vector::{rasterize, VectorBudget, VectorError};

fn rectangle(x: f32, y: f32, width: f32, height: f32) -> Vec<PathCommand> {
    vec![
        PathCommand::Move(Point::new(x, y)),
        PathCommand::Line(Point::new(x + width, y)),
        PathCommand::Line(Point::new(x + width, y + height)),
        PathCommand::Line(Point::new(x, y + height)),
        PathCommand::Close,
    ]
}

#[test]
fn gradients_holes_and_affine_paths_preserve_linear_premultiplied_color() {
    let red = Oklch::from_hex("#ff0000").unwrap();
    let blue = Oklch::from_hex("#0000ff").unwrap();
    let mut commands = rectangle(0.0, 0.0, 64.0, 64.0);
    commands.extend(rectangle(24.0, 24.0, 16.0, 16.0));
    let mut path = PaintedPath::fill(
        commands,
        Brush::Linear {
            start: Point::new(0.0, 0.0),
            end: Point::new(64.0, 0.0),
            stops: Arc::from([
                GradientStop {
                    position: 0.0,
                    color: red.with_alpha(0.5),
                },
                GradientStop {
                    position: 1.0,
                    color: blue.with_alpha(0.5),
                },
            ]),
        },
    );
    path.fill_rule = FillRule::EvenOdd;
    path.transform = Transform2D::translate(8.0, 0.0);
    let source = VectorSource::Drawing(Arc::new(Drawing {
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            w: 80.0,
            h: 64.0,
        },
        paths: vec![path],
    }));
    let raster = rasterize(&source, 80, 64, VectorBudget::default()).unwrap();
    let pixel = |x: usize, y: usize| &raster.pixels[(y * 80 + x) * 4..][..4];
    assert_eq!(pixel(2, 16), &[0, 0, 0, 0]);
    assert_eq!(pixel(40, 32), &[0, 0, 0, 0]);
    let middle = pixel(40, 16);
    assert!(
        (middle[0] as i16 - 64).abs() <= 3 && (middle[2] as i16 - 64).abs() <= 3,
        "linear gradient: {middle:?}"
    );
    assert!((middle[3] as i16 - 128).abs() <= 1);
}

#[test]
fn svg_masks_full_color_and_external_resources_have_explicit_contracts() {
    let svg: Arc<str> = Arc::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="16"><path fill="#ff0000" d="M0 0H16V16H0Z"/><path fill="#00ff00" d="M16 0H32V16H16Z"/></svg>"##,
    );
    let raster = rasterize(
        &VectorSource::svg(svg.clone()),
        32,
        16,
        VectorBudget::default(),
    )
    .unwrap();
    assert_eq!(&raster.pixels[(8 * 32 + 8) * 4..][..4], &[255, 0, 0, 255]);
    assert_eq!(&raster.pixels[(8 * 32 + 24) * 4..][..4], &[0, 255, 0, 255]);
    let tint = Oklch::from_hex("#0000ff").unwrap().with_alpha(0.5);
    let raster = rasterize(
        &VectorSource::tinted_svg(svg, tint),
        32,
        16,
        VectorBudget::default(),
    )
    .unwrap();
    let tinted = &raster.pixels[(8 * 32 + 8) * 4..][..4];
    assert_eq!(&tinted[..2], &[0, 0]);
    assert!((tinted[2] as i16 - 128).abs() <= 1);
    assert_eq!(tinted[3], 128);
    let source = VectorSource::svg(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="20" height="20"><image href="/etc/passwd" width="20" height="20"/></svg>"##,
    );
    assert!(matches!(
        rasterize(&source, 20, 20, VectorBudget::default()),
        Err(VectorError::ExternalResource)
    ));
    assert!(matches!(
        rasterize(
            &VectorSource::svg("invalid"),
            20,
            20,
            VectorBudget::default()
        ),
        Err(VectorError::InvalidSvg(_))
    ));
    assert!(matches!(
        rasterize(&source, 8193, 10, VectorBudget::default()),
        Err(VectorError::TooLarge)
    ));
}

#[test]
fn arcs_cubic_ribbons_stroke_dashes_and_radial_fills_rasterize_geometry() {
    use aqua_core::vector::{LineCap, Stroke};
    let white = Brush::Solid(Oklch::WHITE);
    let circle = PaintedPath::fill(
        vec![
            PathCommand::Move(Point::new(48.0, 32.0)),
            PathCommand::Arc {
                radii: Point::new(16.0, 16.0),
                rotation_radians: 0.0,
                large: false,
                sweep: true,
                to: Point::new(16.0, 32.0),
            },
            PathCommand::Arc {
                radii: Point::new(16.0, 16.0),
                rotation_radians: 0.0,
                large: false,
                sweep: true,
                to: Point::new(48.0, 32.0),
            },
            PathCommand::Close,
        ],
        Brush::Radial {
            center: Point::new(32.0, 32.0),
            focal: Point::new(32.0, 32.0),
            radius: 16.0,
            stops: Arc::from([
                GradientStop {
                    position: 0.0,
                    color: Oklch::WHITE,
                },
                GradientStop {
                    position: 1.0,
                    color: Oklch::WHITE.with_alpha(0.0),
                },
            ]),
        },
    );
    let mut stroke = Stroke::new(white.clone(), 4.0);
    stroke.cap = LineCap::Butt;
    stroke.dash = vec![4.0, 4.0];
    let dashed = PaintedPath::stroke(
        vec![
            PathCommand::Move(Point::new(4.0, 56.0)),
            PathCommand::Line(Point::new(60.0, 56.0)),
        ],
        stroke,
    );
    let ribbon = PaintedPath::fill(
        vec![
            PathCommand::Move(Point::new(4.0, 4.0)),
            PathCommand::Cubic {
                first: Point::new(4.0, 12.0),
                second: Point::new(60.0, 4.0),
                to: Point::new(60.0, 12.0),
            },
            PathCommand::Line(Point::new(60.0, 16.0)),
            PathCommand::Cubic {
                first: Point::new(60.0, 8.0),
                second: Point::new(4.0, 16.0),
                to: Point::new(4.0, 8.0),
            },
            PathCommand::Close,
        ],
        white,
    );
    let source = VectorSource::Drawing(Arc::new(Drawing {
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        },
        paths: vec![circle, dashed, ribbon],
    }));
    let result = rasterize(&source, 64, 64, VectorBudget::default()).unwrap();
    let alpha = |x: usize, y: usize| result.pixels[(y * 64 + x) * 4 + 3];
    assert!(alpha(32, 32) > 230);
    assert!(alpha(44, 32) > 20 && alpha(44, 32) < 90);
    assert_eq!(alpha(50, 32), 0);
    assert_eq!(alpha(5, 56), 255);
    assert_eq!(alpha(9, 56), 0);
    assert!(alpha(32, 10) > 200);
}
