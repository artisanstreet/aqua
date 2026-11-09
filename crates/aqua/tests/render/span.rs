use aqua::render::lower::Lowering;
use aqua_core::geometry::{CornerRadii, Insets, Rect};
use aqua_core::scene::{DisplayItem, ShadowLayer};
use aqua_core::NodeId;
use aqua_core::{Oklch, Size, Transform2D};
use aqua_graph::PaintCommand;
use aqua_protocol::Instant;
use aqua_scene::Scene;
use aqua_text::{SharedFonts, SharedRuns};

const VIEW: Size = Size { w: 200.0, h: 480.0 };
const TOP: f32 = 300.0;

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn lowering() -> Lowering {
    Lowering::new(SharedFonts::new(), SharedRuns::new())
}

fn quad(bounds: Rect, shade: u8) -> DisplayItem {
    DisplayItem::Quad {
        bounds,
        corners: CornerRadii::uniform(3.0),
        fill: Oklch::from_srgb8(shade, 40, 90, 1.0),
        border_widths: Insets::uniform(1.0),
        border_color: Oklch::from_srgb8(200, 200, 200, 1.0),
        dashed: false,
    }
}

fn shadow(bounds: Rect) -> DisplayItem {
    DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(4.0),
        layer: ShadowLayer {
            dx: 0.0,
            dy: 3.0,
            blur: 8.0,
            spread: 0.0,
            inset: false,
            color: Oklch::from_srgb8(0, 0, 0, 0.5),
        },
    }
}

struct Rig {
    scene: Scene,
    glide: NodeId,
    rows: Vec<NodeId>,
}

fn rig(rows: usize) -> Rig {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: rect(0.0, TOP, VIEW.w, 160.0),
        radius: 0.0,
    });
    let glide = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    let ids = (0..rows)
        .map(|row| {
            let y = TOP + row as f32 * 12.0;
            if row % 7 == 0 {
                scene.insert(shadow(rect(10.0, y, 120.0, 10.0)));
            }
            scene.insert(quad(rect(10.0, y, 120.0, 10.0), (row % 200) as u8))
        })
        .collect();
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    Rig {
        scene,
        glide,
        rows: ids,
    }
}

fn frame(lowering: &mut Lowering, scene: &Scene, scale: f32) -> Vec<PaintCommand> {
    let (commands, _, _) = lowering.commands(scene, VIEW, scale, Instant::ZERO);
    lowering.retain_frame(commands.clone());
    commands
}

fn fresh(scene: &Scene, scale: f32) -> Vec<PaintCommand> {
    frame(&mut lowering(), scene, scale)
}

fn same(label: &str, left: &[PaintCommand], right: &[PaintCommand]) {
    let at = left.iter().zip(right).position(|(a, b)| a != b);
    assert!(
        at.is_none() && left.len() == right.len(),
        "{label}: lengths {} vs {}, first difference at {at:?}: {:?} vs {:?}",
        left.len(),
        right.len(),
        at.map(|i| &left[i]),
        at.map(|i| &right[i]),
    );
}

fn scroll(rig: &mut Rig, y: f32) {
    rig.scene.update(
        rig.glide,
        DisplayItem::PushTransform(Transform2D::translate(0.0, -y)),
    );
}

#[test]
fn replayed_glide_frames_equal_fresh_lowering() {
    for scale in [1.0, 1.25, 2.0] {
        let mut rig = rig(120);
        let mut cached = lowering();
        scroll(&mut rig, 0.0);
        frame(&mut cached, &rig.scene, scale);
        for step in 1..40 {
            scroll(&mut rig, step as f32 * 2.6);
            let replayed = frame(&mut cached, &rig.scene, scale);
            same(
                &format!("scale {scale} step {step}"),
                &replayed,
                &fresh(&rig.scene, scale),
            );
        }
        assert!(cached.span_replays() > 20, "scale {scale}");
    }
}

#[test]
fn drifting_past_the_slack_records_again() {
    let mut rig = rig(200);
    let mut cached = lowering();
    scroll(&mut rig, 0.0);
    frame(&mut cached, &rig.scene, 1.0);
    for target in [90.0, 400.0, 401.5, 900.0, 3.0] {
        scroll(&mut rig, target);
        let replayed = frame(&mut cached, &rig.scene, 1.0);
        same(
            &format!("offset {target}"),
            &replayed,
            &fresh(&rig.scene, 1.0),
        );
    }
}

#[test]
fn editing_an_item_inside_the_span_is_never_replayed_stale() {
    let mut rig = rig(60);
    let mut cached = lowering();
    scroll(&mut rig, 5.0);
    frame(&mut cached, &rig.scene, 1.25);
    scroll(&mut rig, 9.0);
    frame(&mut cached, &rig.scene, 1.25);
    rig.scene
        .update(rig.rows[4], quad(rect(30.0, TOP + 48.0, 90.0, 10.0), 250));
    scroll(&mut rig, 11.0);
    let replayed = frame(&mut cached, &rig.scene, 1.25);
    same("edited", &replayed, &fresh(&rig.scene, 1.25));
    scroll(&mut rig, 12.0);
    same(
        "after edit",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
}

#[test]
fn structural_changes_drop_the_cached_span() {
    let mut rig = rig(60);
    let mut cached = lowering();
    scroll(&mut rig, 5.0);
    frame(&mut cached, &rig.scene, 1.0);
    rig.scene.remove(rig.rows[0]);
    scroll(&mut rig, 7.0);
    same(
        "structure",
        &frame(&mut cached, &rig.scene, 1.0),
        &fresh(&rig.scene, 1.0),
    );
}

#[test]
fn structural_changes_outside_the_span_keep_it_replaying() {
    let mut rig = rig(60);
    let mut cached = lowering();
    scroll(&mut rig, 5.0);
    frame(&mut cached, &rig.scene, 1.0);
    scroll(&mut rig, 8.0);
    frame(&mut cached, &rig.scene, 1.0);
    let replays = cached.span_replays();
    rig.scene.insert(quad(rect(0.0, 0.0, 40.0, 10.0), 9));
    scroll(&mut rig, 11.0);
    let replayed = frame(&mut cached, &rig.scene, 1.0);
    same("outside", &replayed, &fresh(&rig.scene, 1.0));
    assert!(cached.span_replays() > replays);
}

#[test]
fn flat_root_segments_replay_and_follow_edits() {
    let mut rig = rig(60);
    let flat: Vec<NodeId> = (0..60)
        .map(|index| {
            rig.scene.insert(quad(
                rect(300.0, index as f32 * 6.0, 20.0, 4.0),
                (index * 3) as u8,
            ))
        })
        .collect();
    let mut cached = lowering();
    for step in 0..4 {
        scroll(&mut rig, 5.0 + step as f32 * 3.0);
        same(
            "warm",
            &frame(&mut cached, &rig.scene, 1.25),
            &fresh(&rig.scene, 1.25),
        );
    }
    let before = cached.span_replays();
    scroll(&mut rig, 30.0);
    same(
        "steady",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
    assert!(cached.span_replays() >= before + 2);
    rig.scene
        .update(flat[5], quad(rect(310.0, 30.0, 25.0, 4.0), 250));
    scroll(&mut rig, 33.0);
    let before_edit = cached.span_replays();
    same(
        "edited",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
    assert!(cached.span_replays() >= before_edit + 2);
    rig.scene
        .update(flat[0], quad(rect(300.0, 0.0, 33.0, 4.0), 200));
    scroll(&mut rig, 34.5);
    same(
        "root edited",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
    rig.scene.remove(flat[7]);
    scroll(&mut rig, 36.0);
    same(
        "removed",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
    scroll(&mut rig, 39.0);
    same(
        "after",
        &frame(&mut cached, &rig.scene, 1.25),
        &fresh(&rig.scene, 1.25),
    );
}

#[test]
fn fractional_scroll_offsets_land_on_whole_device_pixels() {
    let mut rig = rig(10);
    scroll(&mut rig, 3.3);
    let commands = fresh(&rig.scene, 1.25);
    let top = commands
        .iter()
        .find_map(|command| match command {
            PaintCommand::Quad(quad) => Some(quad.bounds.y),
            _ => None,
        })
        .expect("a quad");
    assert_eq!(top.fract(), 0.0);
}

#[test]
fn nested_translations_land_on_whole_device_pixels() {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(Transform2D::translate(0.0, 0.7)));
    scene.insert(DisplayItem::PushTransform(Transform2D::translate(
        0.0, 13.3,
    )));
    scene.insert(quad(rect(0.0, 0.0, 20.0, 8.0), 10));
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopTransform);
    let commands = fresh(&scene, 1.25);
    let top = commands
        .iter()
        .find_map(|command| match command {
            PaintCommand::Quad(quad) => Some(quad.bounds.y),
            _ => None,
        })
        .expect("a quad");
    assert_eq!(top.fract(), 0.0);
}

struct Nested {
    scene: Scene,
    offset: NodeId,
    glide: NodeId,
}

fn nested(rows: usize) -> Nested {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: rect(0.0, TOP, VIEW.w, 160.0),
        radius: 0.0,
    });
    let offset = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    let glide = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    for row in 0..rows {
        let y = TOP + row as f32 * 12.0;
        scene.insert(quad(rect(10.0, y, 120.0, 10.0), (row % 200) as u8));
    }
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    Nested {
        scene,
        offset,
        glide,
    }
}

#[test]
fn nested_glide_wrapper_replays_while_the_outer_offset_changes_rarely() {
    let mut rig = nested(150);
    let mut cached = lowering();
    for step in 0..60 {
        let base = (step / 20) as f32 * 40.0;
        let wrapper = (step % 20) as f32 * 0.8;
        rig.scene.update(
            rig.offset,
            DisplayItem::PushTransform(Transform2D::translate(0.0, -base)),
        );
        rig.scene.update(
            rig.glide,
            DisplayItem::PushTransform(Transform2D::translate(0.0, -wrapper)),
        );
        let replayed = frame(&mut cached, &rig.scene, 1.25);
        let expected = fresh(&rig.scene, 1.25);
        same(&format!("step {step}"), &replayed, &expected);
    }
    assert!(cached.span_replays() > 30);
}

#[test]
fn leaves_that_change_every_frame_are_patched_into_the_replay() {
    let mut rig = rig(150);
    let mut cached = lowering();
    for step in 0..50 {
        scroll(&mut rig, step as f32 * 1.9);
        let sticky = TOP + 20.0 + step as f32 * 0.7;
        rig.scene.update(
            rig.rows[3],
            quad(rect(30.0, sticky, 90.0, 10.0), 10 + step as u8),
        );
        rig.scene.update(
            rig.rows[9],
            shadow(rect(12.0, TOP + 100.0 - step as f32, 100.0, 12.0)),
        );
        let replayed = frame(&mut cached, &rig.scene, 1.25);
        same(&format!("step {step}"), &replayed, &fresh(&rig.scene, 1.25));
    }
    assert!(cached.span_replays() > 40);
}

#[test]
fn a_changing_scope_item_falls_back_without_going_stale() {
    let mut rig = rig(150);
    let mut cached = lowering();
    for step in 0..30 {
        scroll(&mut rig, step as f32 * 1.9);
        if step % 3 == 0 {
            rig.scene.update(
                NodeIdOf::first(&rig.scene),
                DisplayItem::PushClip {
                    bounds: rect(0.0, TOP + step as f32, VIEW.w, 150.0),
                    radius: 0.0,
                },
            );
        }
        let replayed = frame(&mut cached, &rig.scene, 1.0);
        same(&format!("step {step}"), &replayed, &fresh(&rig.scene, 1.0));
    }
}

struct NodeIdOf;

impl NodeIdOf {
    fn first(scene: &Scene) -> NodeId {
        scene.ordered().next().map(|(id, _)| id).unwrap()
    }
}

struct Canceled {
    scene: Scene,
    offset: NodeId,
    cancels: Vec<(NodeId, NodeId, NodeId)>,
}

fn canceled(rows: usize, canvases: &[usize]) -> Canceled {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: rect(0.0, TOP, VIEW.w, 160.0),
        radius: 0.0,
    });
    let offset = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
    let mut cancels = Vec::new();
    for row in 0..rows {
        let y = TOP + row as f32 * 12.0;
        scene.insert(quad(rect(10.0, y, 120.0, 10.0), (row % 200) as u8));
        if canvases.contains(&row) {
            let open = scene.insert(DisplayItem::PushTransform(Transform2D::IDENTITY));
            let first = scene.insert(quad(rect(140.0, y, 20.0, 10.0), 200));
            let second = scene.insert(quad(rect(140.0, y + 2.0, 20.0, 6.0), 100));
            scene.insert(DisplayItem::PopTransform);
            cancels.push((open, first, second));
        }
    }
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    Canceled {
        scene,
        offset,
        cancels,
    }
}

#[test]
fn screen_space_canvases_that_cancel_the_scroll_are_patched_as_units() {
    let canvases = [2, 5, 9, 30, 60];
    let mut rig = canceled(100, &canvases);
    let mut cached = lowering();
    for step in 0..45 {
        let scrolled = step as f32 * 2.3;
        rig.scene.update(
            rig.offset,
            DisplayItem::PushTransform(Transform2D::translate(0.0, -scrolled)),
        );
        for (index, (open, first, second)) in rig.cancels.iter().enumerate() {
            let y = TOP + canvases[index] as f32 * 12.0 - scrolled;
            rig.scene.update(
                *open,
                DisplayItem::PushTransform(Transform2D::translate(0.0, scrolled)),
            );
            rig.scene
                .update(*first, quad(rect(140.0, y, 20.0, 10.0), 200));
            rig.scene
                .update(*second, quad(rect(140.0, y + 2.0, 20.0, 6.0), 100));
        }
        let replayed = frame(&mut cached, &rig.scene, 1.25);
        same(&format!("step {step}"), &replayed, &fresh(&rig.scene, 1.25));
    }
    assert!(cached.span_replays() > 30);
}

#[test]
fn retiring_changing_paths_keeps_unrelated_scene_spans_reusable() {
    use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
    use aqua_core::{GpuiBackground, GpuiHsla, Point};
    use std::sync::Arc;
    let mut rig = rig(120);
    let path = |width| DisplayItem::GpuiPath {
        transformed_window_logical_bounds: rect(0.0, 0.0, width, 20.0),
        path: Arc::new(GpuiPath {
            commands_local: vec![
                PathCommand::Move(Point::new(0.0, 0.0)),
                PathCommand::Line(Point::new(width, 0.0)),
                PathCommand::Line(Point::new(width, 20.0)),
                PathCommand::Line(Point::new(0.0, 20.0)),
                PathCommand::Close,
            ],
            origin_window_logical: Point::ZERO,
            element_transform: Transform2D::IDENTITY,
            style: GpuiPathStyle::Fill {
                rule: FillRule::NonZero,
            },
            background: GpuiBackground::Solid(GpuiHsla {
                h: 0.0,
                s: 0.0,
                l: 0.5,
                a: 1.0,
            }),
        }),
    };
    let changing = rig.scene.insert(path(100.0));
    let mut cached = lowering();
    frame(&mut cached, &rig.scene, 1.0);
    for step in 1..20 {
        rig.scene.update(changing, path(100.0 + step as f32));
        scroll(&mut rig, step as f32 * 2.6);
        frame(&mut cached, &rig.scene, 1.0);
    }
    assert!(
        cached.span_replays() >= 18,
        "{} span replays",
        cached.span_replays()
    );
}
