use std::sync::Arc;

use aqua::UiApp;
use aqua::render::lower::Lowering;
use aqua::ui::Text;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshStyle, GpuiMeshVertex};
use aqua_core::scene::{DisplayItem, GpuiShadowLayer, ShadowLayer};
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand, VectorSource};
use aqua_core::{GpuiBackground, GpuiHsla, Oklch, Point, Size, Transform2D};
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::{LowerContext, Lowered};

use crate::support::{self, Frame, Painter};

const SIZE: Size = Size { w: 64.0, h: 48.0 };

fn red() -> Oklch {
    Oklch::from_srgb8(255, 0, 0, 1.0)
}

fn scene_of(items: Vec<DisplayItem>) -> Scene {
    let mut scene = Scene::new();
    items.into_iter().for_each(|item| {
        scene.insert(item);
    });
    scene
}

fn painters() -> Vec<Painter> {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    support::painters(&fonts, &runs)
}

fn paint(painter: &mut Painter, items: Vec<DisplayItem>) -> Frame {
    let frame = painter.paint(&scene_of(items), SIZE, 1.0);
    assert!(
        painter.report.skipped.is_empty(),
        "{}: {:?}",
        painter.label,
        painter.report.skipped
    );
    frame
}

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn quad(bounds: Rect, corners: CornerRadii) -> DisplayItem {
    DisplayItem::Quad {
        bounds,
        corners,
        fill: red(),
        border_widths: Insets::uniform(0.0),
        border_color: Oklch::TRANSPARENT,
        dashed: false,
    }
}

#[test]
fn per_corner_radii_round_only_the_requested_corners() {
    let corners = CornerRadii {
        top_left: 14.0,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![quad(rect(8.0, 8.0, 48.0, 32.0), corners)],
        );
        assert!(
            frame.pixel(9, 9)[3] < 40,
            "{}: rounded corner",
            painter.label
        );
        assert!(
            frame.pixel(54, 9)[3] > 215,
            "{}: square corner",
            painter.label
        );
        assert!(
            frame.pixel(9, 38)[3] > 215,
            "{}: square corner",
            painter.label
        );
        assert!(
            frame.pixel(54, 38)[3] > 215,
            "{}: square corner",
            painter.label
        );
        assert!(
            frame.pixel(32, 24)[3] > 250,
            "{}: seams stay closed",
            painter.label
        );
    }
}

#[test]
fn inset_shadows_darken_the_inside_of_the_edges_only() {
    let layer = ShadowLayer {
        dx: 0.0,
        dy: 0.0,
        blur: 10.0,
        spread: 0.0,
        inset: true,
        color: Oklch::BLACK,
    };
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::Quad {
                    bounds: rect(8.0, 8.0, 48.0, 32.0),
                    corners: CornerRadii::uniform(0.0),
                    fill: Oklch::WHITE,
                    border_widths: Insets::uniform(0.0),
                    border_color: Oklch::TRANSPARENT,
                    dashed: false,
                },
                DisplayItem::Shadow {
                    bounds: rect(8.0, 8.0, 48.0, 32.0),
                    corners: CornerRadii::uniform(0.0),
                    layer,
                },
            ],
        );
        let edge = frame.pixel(9, 24)[0];
        let centre = frame.pixel(32, 24)[0];
        assert!(edge < 200, "{}: edge {edge}", painter.label);
        assert!(
            centre > edge + 40,
            "{}: centre {centre} edge {edge}",
            painter.label
        );
        assert_eq!(
            frame.pixel(4, 24)[3],
            0,
            "{}: nothing outside",
            painter.label
        );
        assert_eq!(
            frame.pixel(60, 24)[3],
            0,
            "{}: nothing outside",
            painter.label
        );
    }
}

#[test]
fn gpui_inset_shadow_matches_the_native_one_in_shape() {
    let layer = GpuiShadowLayer {
        dx: 0.0,
        dy: 0.0,
        blur: 10.0,
        spread: 0.0,
        inset: true,
        color: GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        },
    };
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::GpuiBackground {
                    bounds: rect(8.0, 8.0, 48.0, 32.0),
                    corners: CornerRadii::uniform(6.0),
                    background: GpuiBackground::Solid(GpuiHsla {
                        h: 0.0,
                        s: 0.0,
                        l: 1.0,
                        a: 1.0,
                    }),
                },
                DisplayItem::GpuiShadow {
                    bounds: rect(8.0, 8.0, 48.0, 32.0),
                    corners: CornerRadii::uniform(6.0),
                    layer,
                },
            ],
        );
        assert!(
            frame.pixel(32, 24)[0] > frame.pixel(10, 24)[0] + 60,
            "{}",
            painter.label
        );
        assert_eq!(frame.pixel(3, 3)[3], 0, "{}", painter.label);
    }
}

#[test]
fn dashed_borders_leave_gaps_along_straight_edges() {
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![DisplayItem::Quad {
                bounds: rect(4.0, 4.0, 56.0, 40.0),
                corners: CornerRadii::uniform(4.0),
                fill: Oklch::TRANSPARENT,
                border_widths: Insets::uniform(2.0),
                border_color: red(),
                dashed: true,
            }],
        );
        let row: Vec<u8> = (14..50).map(|x| frame.pixel(x, 5)[3]).collect();
        let solid = row.iter().filter(|alpha| **alpha > 200).count();
        let empty = row.iter().filter(|alpha| **alpha < 20).count();
        assert!(solid >= 8 && empty >= 6, "{}: {row:?}", painter.label);
        assert_eq!(
            frame.pixel(32, 24)[3],
            0,
            "{}: interior stays clear",
            painter.label
        );
    }
}

#[test]
fn uneven_borders_draw_each_side_at_its_own_width() {
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![DisplayItem::Quad {
                bounds: rect(4.0, 4.0, 56.0, 40.0),
                corners: CornerRadii::uniform(0.0),
                fill: Oklch::TRANSPARENT,
                border_widths: Insets {
                    left: 6.0,
                    top: 2.0,
                    right: 0.0,
                    bottom: 4.0,
                },
                border_color: red(),
                dashed: false,
            }],
        );
        assert!(frame.pixel(8, 24)[3] > 200, "{}: left", painter.label);
        assert!(
            frame.pixel(12, 24)[3] < 20,
            "{}: left ends at six",
            painter.label
        );
        assert!(frame.pixel(30, 5)[3] > 200, "{}: top", painter.label);
        assert!(
            frame.pixel(30, 8)[3] < 20,
            "{}: top ends at two",
            painter.label
        );
        assert!(frame.pixel(30, 41)[3] > 200, "{}: bottom", painter.label);
        assert!(
            frame.pixel(58, 24)[3] < 20,
            "{}: no right border",
            painter.label
        );
    }
}

#[test]
fn transforms_translate_and_scale_every_primitive_under_them() {
    let transform = Transform2D::scale(2.0, 2.0).then(Transform2D::translate(10.0, 4.0));
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::PushTransform(transform),
                quad(rect(0.0, 0.0, 10.0, 8.0), CornerRadii::uniform(0.0)),
                DisplayItem::PopTransform,
            ],
        );
        assert!(frame.pixel(11, 5)[3] > 250, "{}", painter.label);
        assert!(frame.pixel(29, 19)[3] > 250, "{}", painter.label);
        assert_eq!(frame.pixel(5, 5)[3], 0, "{}", painter.label);
        assert_eq!(frame.pixel(32, 5)[3], 0, "{}", painter.label);
    }
}

#[test]
fn rotated_quads_are_rotated_exactly_and_not_reported_as_approximated() {
    let transform = Transform2D {
        a: 0.8,
        b: 0.6,
        c: -0.6,
        d: 0.8,
        tx: 20.0,
        ty: 10.0,
    };
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::PushTransform(transform),
                quad(rect(0.0, 0.0, 10.0, 8.0), CornerRadii::uniform(0.0)),
                DisplayItem::PopTransform,
            ],
        );
        assert_eq!(painter.report.approximated, 0, "{}", painter.label);
        assert!(frame.pixel(21, 16)[3] > 250, "{}: center", painter.label);
        assert!(frame.pixel(24, 18)[3] > 250, "{}: inside", painter.label);
        assert_eq!(
            frame.pixel(16, 11)[3],
            0,
            "{}: the old bounding box corner",
            painter.label
        );
    }
}

#[test]
fn svg_vectors_paths_and_meshes_reach_the_surface() {
    let svg = "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 8 8\"><rect width=\"8\" height=\"8\" fill=\"#ff0000\"/></svg>";
    let path = GpuiPath {
        commands_local: vec![
            PathCommand::Move(Point::new(0.0, 0.0)),
            PathCommand::Line(Point::new(16.0, 0.0)),
            PathCommand::Line(Point::new(16.0, 16.0)),
            PathCommand::Line(Point::new(0.0, 16.0)),
            PathCommand::Close,
        ],
        origin_window_logical: Point::new(40.0, 4.0),
        element_transform: Transform2D::IDENTITY,
        style: GpuiPathStyle::Fill {
            rule: FillRule::NonZero,
        },
        background: GpuiBackground::Solid(GpuiHsla {
            h: 2.0 / 3.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
    };
    let corner =
        |x: f32, y: f32| GpuiMeshVertex::new([x, y, 0.0], [0.0, 0.0, 1.0], Some([0.0, 1.0, 0.0]));
    let mesh = Arc::new(GpuiMesh::new(vec![
        corner(-1.0, -1.0),
        corner(1.0, -1.0),
        corner(1.0, 1.0),
    ]));
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let mut projection = identity;
    projection[2][2] = -0.5;
    projection[3][2] = 0.5;
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::Vector {
                    bounds: rect(2.0, 2.0, 16.0, 16.0),
                    source: VectorSource::svg(svg),
                    opacity: 1.0,
                },
                DisplayItem::GpuiSvg {
                    bounds: rect(2.0, 24.0, 16.0, 16.0),
                    source: svg.into(),
                    tint: Some(GpuiHsla {
                        h: 1.0 / 3.0,
                        s: 1.0,
                        l: 0.5,
                        a: 1.0,
                    }),
                    opacity: 1.0,
                },
                DisplayItem::GpuiPath {
                    transformed_window_logical_bounds: rect(40.0, 4.0, 16.0, 16.0),
                    path: Arc::new(path.clone()),
                },
                DisplayItem::GpuiMesh {
                    bounds: rect(24.0, 24.0, 24.0, 20.0),
                    corners: CornerRadii::uniform(0.0),
                    mesh: mesh.clone(),
                    style: GpuiMeshStyle {
                        view: identity,
                        projection,
                        base_color: [0.5, 0.5, 0.5],
                        edge_color: [0.0, 0.0, 0.0, 0.0],
                    },
                },
            ],
        );
        let red_ok = |p: [u8; 4]| p[3] > 200 && p[0] > 200 && p[1] < 60 && p[2] < 60;
        let green_ok = |p: [u8; 4]| p[3] > 200 && p[1] > 150 && p[0] < 90 && p[2] < 90;
        assert!(
            red_ok(frame.pixel(10, 10)),
            "{}: vector {:?}",
            painter.label,
            frame.pixel(10, 10)
        );
        assert!(
            green_ok(frame.pixel(10, 32)),
            "{}: tinted svg {:?}",
            painter.label,
            frame.pixel(10, 32)
        );
        let blue = frame.pixel(48, 12);
        assert!(
            blue[3] > 200 && blue[2] > 200 && blue[0] < 60,
            "{}: path {blue:?}",
            painter.label
        );
        assert!(frame.pixel(40, 40)[3] > 200, "{}: mesh", painter.label);
    }
}

#[test]
fn rounded_clips_cut_the_corners_of_what_they_contain() {
    for mut painter in painters() {
        let frame = paint(
            &mut painter,
            vec![
                DisplayItem::PushClip {
                    bounds: rect(8.0, 8.0, 40.0, 32.0),
                    radius: 12.0,
                },
                quad(rect(0.0, 0.0, 64.0, 48.0), CornerRadii::uniform(0.0)),
                DisplayItem::PopClip,
            ],
        );
        assert!(frame.pixel(9, 9)[3] < 60, "{}: corner cut", painter.label);
        assert!(frame.pixel(28, 24)[3] > 250, "{}: inside", painter.label);
        assert_eq!(frame.pixel(56, 24)[3], 0, "{}: outside", painter.label);
    }
}

fn text_app() -> (UiApp, SharedFonts, SharedRuns) {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone());
    app.mount(|_| Text::new("Recover me").build()).unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    (app, fonts, runs)
}

fn lowered(lowering: &mut Lowering, scene: &Scene) -> Lowered {
    aqua_transport::Lowering::lower(lowering, scene, &context())
}

fn context() -> LowerContext {
    LowerContext {
        size: Size { w: 160.0, h: 64.0 },
        scale: 1.0,
        now: aqua_protocol::Instant::ZERO,
    }
}

#[test]
fn atlas_uploads_are_sent_once_and_again_after_a_device_that_lost_its_textures() {
    let (app, fonts, runs) = text_app();
    let scene = app.scene();
    let mut lowering = Lowering::new(fonts, runs);
    let first = lowered(&mut lowering, scene);
    assert!(first.glyphs > 0 && !first.uploads.textures.is_empty());
    let steady = lowered(&mut lowering, scene);
    assert!(steady.uploads.textures.is_empty());
    aqua_transport::Lowering::device_recovered(&mut lowering);
    let rebuilt = lowered(&mut lowering, scene);
    assert!(!rebuilt.uploads.textures.is_empty());
    assert_eq!(rebuilt.glyphs, first.glyphs);
}

#[test]
fn a_device_that_replayed_its_textures_keeps_the_atlas() {
    let (app, fonts, runs) = text_app();
    let scene = app.scene();
    let signal = aqua::render::lower::RecoverySignal::default();
    let _listener = signal.listener();
    let mut lowering = Lowering::new(fonts, runs).with_recovery(signal);
    lowered(&mut lowering, scene);
    aqua_transport::Lowering::device_recovered(&mut lowering);
    assert!(lowered(&mut lowering, scene).uploads.textures.is_empty());
}

#[test]
fn a_device_that_needs_reuploads_clears_the_atlas_through_the_listener() {
    let (app, fonts, runs) = text_app();
    let scene = app.scene();
    let signal = aqua::render::lower::RecoverySignal::default();
    let listener = signal.listener();
    let mut lowering = Lowering::new(fonts, runs).with_recovery(signal);
    lowered(&mut lowering, scene);
    listener.textures_lost();
    aqua_transport::Lowering::device_recovered(&mut lowering);
    assert!(!lowered(&mut lowering, scene).uploads.textures.is_empty());
}

#[test]
fn trimming_everything_releases_pages_that_the_last_frame_did_not_use() {
    let (app, fonts, runs) = text_app();
    let scene = app.scene();
    let mut lowering = Lowering::new(fonts, runs);
    lowered(&mut lowering, scene);
    let empty = Scene::new();
    lowered(&mut lowering, &empty);
    let freed =
        aqua_transport::Lowering::trim(&mut lowering, aqua_transport::TrimRequest::Everything);
    assert!(freed > 0);
    let after = lowered(&mut lowering, &empty);
    assert!(
        after
            .uploads
            .textures
            .iter()
            .any(|upload| matches!(upload.op, aqua_protocol::TextureOp::Release))
    );
}

fn resident_images(lowering: &Lowering) -> Vec<aqua_media::atlas::ImageKey> {
    lowering
        .residency()
        .page_keys()
        .1
        .into_iter()
        .flatten()
        .filter_map(|key| match key {
            aqua_media::residency::ColorKey::Image(key) => Some(key),
            _ => None,
        })
        .collect()
}

fn image_item(bounds: Rect) -> DisplayItem {
    DisplayItem::Image {
        bounds,
        layer: 7,
        fit: aqua_core::image::ImageFit::Fill,
        position: Point { x: 0.5, y: 0.5 },
        source: rect(0.0, 0.0, 1.0, 1.0),
        opacity: 1.0,
        rounded_clip: 0.0,
    }
}

#[test]
fn images_drawn_small_upload_a_reduced_copy_and_release_every_level() {
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    lowering.register_image(7, 128, 128, &vec![200; 128 * 128 * 4]);
    lowered(
        &mut lowering,
        &scene_of(vec![image_item(rect(0.0, 0.0, 16.0, 16.0))]),
    );
    let small = resident_images(&lowering);
    assert_eq!(small.len(), 1);
    assert_eq!(
        (small[0].frame, small[0].width, small[0].height),
        (3, 16, 16)
    );
    lowered(
        &mut lowering,
        &scene_of(vec![image_item(rect(0.0, 0.0, 128.0, 128.0))]),
    );
    let both = resident_images(&lowering);
    assert!(both.iter().any(|key| key.frame == 0 && key.width == 128));
    assert!(both.iter().any(|key| key.frame == 3));
    lowering.release_image(7);
    lowered(&mut lowering, &Scene::new());
    assert!(resident_images(&lowering).is_empty());
}
