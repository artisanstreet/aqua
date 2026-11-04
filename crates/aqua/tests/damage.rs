use aqua::render::damage::{DamagePlanner, Plan};
use aqua_core::geometry::Transform2D;
use aqua_core::{DisplayItem, NodeId, Oklch, Rect, Size};
use aqua_scene::Scene;
use aqua_text::SharedRuns;

const SIZE: Size = Size { w: 400.0, h: 300.0 };

fn block(x: f32, y: f32) -> DisplayItem {
    DisplayItem::Rect {
        bounds: Rect {
            x,
            y,
            w: 20.0,
            h: 20.0,
        },
        radius: 0.0,
        fill: Oklch::from_hex("#336699").unwrap(),
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    }
}

struct Rig {
    scene: Scene,
    runs: SharedRuns,
    planner: DamagePlanner,
    blocks: Vec<NodeId>,
}

fn rig() -> Rig {
    let mut scene = Scene::new();
    let blocks = (0..6)
        .map(|i| scene.insert(block(i as f32 * 60.0, 40.0)))
        .collect();
    Rig {
        scene,
        runs: SharedRuns::new(),
        planner: DamagePlanner::new(),
        blocks,
    }
}

fn settle(rig: &mut Rig, scale: f32) -> Plan {
    let plan = rig.planner.plan(&rig.scene, &rig.runs, SIZE, scale);
    rig.planner.commit();
    plan
}

fn covers(plan: &Plan, target: Rect) -> bool {
    match plan {
        Plan::Full => true,
        Plan::Regions(regions) => regions.iter().any(|r| {
            r.x <= target.x
                && r.y <= target.y
                && r.right() >= target.right()
                && r.bottom() >= target.bottom()
        }),
    }
}

#[test]
fn the_first_frame_is_full() {
    let mut rig = rig();
    assert_eq!(settle(&mut rig, 1.0), Plan::Full);
}

#[test]
fn one_changed_item_damages_only_its_old_and_new_footprint() {
    let mut rig = rig();
    settle(&mut rig, 1.0);
    rig.scene.update(rig.blocks[2], block(130.0, 90.0));
    let plan = settle(&mut rig, 1.0);
    let Plan::Regions(regions) = &plan else {
        panic!("expected partial damage, got {plan:?}");
    };
    assert!(regions.len() <= 2);
    assert!(covers(
        &plan,
        Rect {
            x: 120.0,
            y: 40.0,
            w: 20.0,
            h: 20.0
        }
    ));
    assert!(covers(
        &plan,
        Rect {
            x: 130.0,
            y: 90.0,
            w: 20.0,
            h: 20.0
        }
    ));
    let area: f32 = regions.iter().map(|r| r.w * r.h).sum();
    assert!(area < 0.1 * SIZE.w * SIZE.h);
}

#[test]
fn damage_is_in_physical_pixels() {
    let mut rig = rig();
    settle(&mut rig, 2.0);
    rig.scene.update(rig.blocks[0], block(0.0, 100.0));
    let plan = settle(&mut rig, 2.0);
    assert!(covers(
        &plan,
        Rect {
            x: 0.0,
            y: 200.0,
            w: 40.0,
            h: 40.0
        }
    ));
}

#[test]
fn an_unchanged_scene_damages_a_single_pixel() {
    let mut rig = rig();
    settle(&mut rig, 1.0);
    match settle(&mut rig, 1.0) {
        Plan::Regions(regions) => assert_eq!(regions.len(), 1),
        Plan::Full => panic!("unchanged scene should not repaint everything"),
    }
}

#[test]
fn a_viewport_change_repaints_everything() {
    let mut rig = rig();
    settle(&mut rig, 1.0);
    assert_eq!(settle(&mut rig, 1.25), Plan::Full);
}

#[test]
fn an_invalidated_planner_repaints_everything_once() {
    let mut rig = rig();
    settle(&mut rig, 1.0);
    rig.planner.invalidate();
    assert_eq!(settle(&mut rig, 1.0), Plan::Full);
    assert!(matches!(settle(&mut rig, 1.0), Plan::Regions(_)));
}

#[test]
fn an_uncommitted_frame_does_not_advance_the_baseline() {
    let mut rig = rig();
    settle(&mut rig, 1.0);
    rig.scene.update(rig.blocks[1], block(70.0, 140.0));
    let lost = rig.planner.plan(&rig.scene, &rig.runs, SIZE, 1.0);
    assert!(matches!(lost, Plan::Regions(_)));
    rig.planner.invalidate();
    assert_eq!(settle(&mut rig, 1.0), Plan::Full);
}

#[test]
fn moving_a_group_damages_both_extents() {
    let mut scene = Scene::new();
    let push = scene.insert(DisplayItem::PushTransform(Transform2D::translate(0.0, 0.0)));
    scene.insert(block(10.0, 10.0));
    scene.insert(block(60.0, 10.0));
    scene.insert(DisplayItem::PopTransform);
    scene.insert(block(300.0, 200.0));
    let runs = SharedRuns::new();
    let mut planner = DamagePlanner::new();
    planner.plan(&scene, &runs, SIZE, 1.0);
    planner.commit();
    scene.update(
        push,
        DisplayItem::PushTransform(Transform2D::translate(0.0, 100.0)),
    );
    let plan = planner.plan(&scene, &runs, SIZE, 1.0);
    assert!(covers(
        &plan,
        Rect {
            x: 10.0,
            y: 10.0,
            w: 70.0,
            h: 20.0
        }
    ));
    assert!(covers(
        &plan,
        Rect {
            x: 10.0,
            y: 110.0,
            w: 70.0,
            h: 20.0
        }
    ));
    let Plan::Regions(regions) = plan else {
        panic!("group move should stay partial");
    };
    assert!(regions
        .iter()
        .all(|r| !(r.x < 300.0 && r.right() > 300.0 && r.y < 200.0 && r.bottom() > 200.0)));
}

#[test]
fn blurred_scenes_repaint_fully_when_anything_changes() {
    let mut rig = rig();
    rig.scene.insert(DisplayItem::BackdropBlur {
        bounds: Rect {
            x: 200.0,
            y: 200.0,
            w: 50.0,
            h: 50.0,
        },
        radius: 8.0,
    });
    settle(&mut rig, 1.0);
    rig.scene.update(rig.blocks[0], block(0.0, 100.0));
    assert_eq!(settle(&mut rig, 1.0), Plan::Full);
}

mod differential {
    use aqua::render::damage::{DamagePlanner, Plan};
    use aqua_core::geometry::Transform2D;
    use aqua_core::{DisplayItem, NodeId, Oklch, Rect, Size};
    use aqua_gpu_ref::RefBackend;
    use aqua_graph::{plan_paint, Extent};
    use aqua_protocol::{FrameInputs, GpuBackend, GpuReadback, PresentHint, SurfaceTarget};
    use aqua_scene::Scene;
    use aqua_text::SharedRuns;
    use aqua_transport::{GeometryLowering, LowerContext, Lowering};

    const WIDTH: u32 = 160;
    const HEIGHT: u32 = 120;

    struct Lcg(u64);

    impl Lcg {
        fn next(&mut self, bound: u32) -> u32 {
            self.0 = self
                .0
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            ((self.0 >> 33) % u64::from(bound)) as u32
        }
    }

    fn item(rng: &mut Lcg) -> DisplayItem {
        let colors = ["#cc3344", "#33aa55", "#3355cc", "#ddcc22", "#aa22cc"];
        let shadow = match rng.next(3) {
            0 => Some((
                2.0,
                3.0,
                4.0,
                Oklch::from_hex("#000000").unwrap().with_alpha(0.5),
            )),
            _ => None,
        };
        DisplayItem::Rect {
            bounds: Rect {
                x: rng.next(120) as f32,
                y: rng.next(90) as f32,
                w: 8.0 + rng.next(28) as f32,
                h: 8.0 + rng.next(28) as f32,
            },
            radius: rng.next(6) as f32,
            fill: Oklch::from_hex(colors[rng.next(5) as usize]).unwrap(),
            border_width: 0.0,
            border_color: Oklch::TRANSPARENT,
            shadow,
        }
    }

    fn render(backend: &mut RefBackend, scene: &Scene, scale: f32) -> Vec<u8> {
        let size = Size {
            w: WIDTH as f32 / scale,
            h: HEIGHT as f32 / scale,
        };
        let lowered = GeometryLowering.lower(
            scene,
            &LowerContext {
                size,
                scale,
                now: aqua_protocol::Instant::ZERO,
            },
        );
        let extent = Extent {
            width: WIDTH,
            height: HEIGHT,
        };
        let plan = plan_paint(
            extent,
            Oklch::from_hex("#101418").unwrap(),
            &lowered.commands,
        )
        .unwrap();
        let surface = backend
            .create_surface(SurfaceTarget::Offscreen(extent))
            .unwrap();
        backend.execute(&plan, &FrameInputs::new(surface)).unwrap();
        backend.present(surface, PresentHint::Vsync).unwrap();
        backend.read_surface(surface).unwrap().rgba8
    }

    fn splice(base: &mut [u8], fresh: &[u8], regions: &[Rect]) {
        for region in regions {
            let x0 = region.x.max(0.0) as usize;
            let y0 = region.y.max(0.0) as usize;
            let x1 = (region.right().ceil() as usize).min(WIDTH as usize);
            let y1 = (region.bottom().ceil() as usize).min(HEIGHT as usize);
            for y in y0..y1 {
                let from = (y * WIDTH as usize + x0) * 4;
                let to = (y * WIDTH as usize + x1) * 4;
                base[from..to].copy_from_slice(&fresh[from..to]);
            }
        }
    }

    fn run(scale: f32, seed: u64) {
        let mut rng = Lcg(seed);
        let mut scene = Scene::new();
        let mut nodes: Vec<NodeId> = (0..10).map(|_| scene.insert(item(&mut rng))).collect();
        let push = scene.insert(DisplayItem::PushTransform(Transform2D::translate(0.0, 0.0)));
        let grouped: Vec<NodeId> = (0..3).map(|_| scene.insert(item(&mut rng))).collect();
        scene.insert(DisplayItem::PopTransform);
        let runs = SharedRuns::new();
        let mut planner = DamagePlanner::new();
        let size = Size {
            w: WIDTH as f32 / scale,
            h: HEIGHT as f32 / scale,
        };
        let mut backend = RefBackend::new();
        let mut shown = render(&mut backend, &scene, scale);
        planner.plan(&scene, &runs, size, scale);
        planner.commit();
        let mut partial = 0;
        for step in 0..30 {
            match rng.next(5) {
                0 => {
                    let at = rng.next(nodes.len() as u32) as usize;
                    scene.update(nodes[at], item(&mut rng));
                }
                1 => {
                    let at = rng.next(nodes.len() as u32) as usize;
                    scene.remove(nodes.remove(at));
                    nodes.push(scene.insert(item(&mut rng)));
                }
                2 => {
                    scene.update(
                        push,
                        DisplayItem::PushTransform(Transform2D::translate(
                            rng.next(40) as f32,
                            rng.next(30) as f32,
                        )),
                    );
                }
                3 => {
                    let at = rng.next(grouped.len() as u32) as usize;
                    scene.update(grouped[at], item(&mut rng));
                }
                _ => {
                    nodes.push(scene.insert(item(&mut rng)));
                }
            }
            let plan = planner.plan(&scene, &runs, size, scale);
            planner.commit();
            let fresh = render(&mut backend, &scene, scale);
            match &plan {
                Plan::Full => shown = fresh,
                Plan::Regions(regions) => {
                    partial += 1;
                    splice(&mut shown, &fresh, regions);
                    assert_eq!(
                        shown, fresh,
                        "seed {seed} scale {scale} step {step}: damage {regions:?} missed a change"
                    );
                }
            }
        }
        assert!(partial > 10, "seed {seed}: only {partial} partial frames");
    }

    #[test]
    fn partial_regions_reproduce_the_full_render() {
        (1..=6).for_each(|seed| run(1.0, seed));
    }

    #[test]
    fn partial_regions_reproduce_the_full_render_at_fractional_scale() {
        (11..=14).for_each(|seed| run(1.25, seed));
    }
}
