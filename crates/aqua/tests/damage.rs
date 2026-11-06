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
    let plan = rig.planner.plan(&rig.scene, &rig.runs, SIZE, scale, true);
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
    let lost = rig.planner.plan(&rig.scene, &rig.runs, SIZE, 1.0, true);
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
    planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    scene.update(
        push,
        DisplayItem::PushTransform(Transform2D::translate(0.0, 100.0)),
    );
    let plan = planner.plan(&scene, &runs, SIZE, 1.0, true);
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

fn blur_at(x: f32, y: f32) -> DisplayItem {
    DisplayItem::BackdropBlur {
        bounds: Rect {
            x,
            y,
            w: 50.0,
            h: 50.0,
        },
        radius: 8.0,
    }
}

#[test]
fn damage_that_reaches_a_backdrop_blur_widens_to_the_whole_blur_zone() {
    let mut rig = rig();
    rig.scene.insert(blur_at(200.0, 200.0));
    settle(&mut rig, 1.0);
    rig.scene.update(rig.blocks[0], block(200.0, 190.0));
    let plan = settle(&mut rig, 1.0);
    assert!(matches!(plan, Plan::Regions(_)), "{plan:?}");
    assert!(covers(
        &plan,
        Rect {
            x: 200.0 - 48.0,
            y: 200.0 - 48.0,
            w: 50.0 + 96.0,
            h: 50.0 + 96.0
        }
    ));
}

#[test]
fn damage_far_from_a_backdrop_blur_stays_partial() {
    let mut rig = rig();
    rig.scene.insert(blur_at(200.0, 200.0));
    settle(&mut rig, 1.0);
    rig.scene.update(rig.blocks[0], block(0.0, 100.0));
    assert!(matches!(settle(&mut rig, 1.0), Plan::Regions(_)));
}

fn pulse() -> DisplayItem {
    DisplayItem::PushGpuiQuadPulse(aqua_core::scene::GpuiQuadPulse {
        start_ms: 0,
        period_ms: 1600,
        min_opacity: 0.4,
    })
}

fn quad(x: f32, y: f32) -> DisplayItem {
    DisplayItem::Quad {
        bounds: Rect {
            x,
            y,
            w: 30.0,
            h: 30.0,
        },
        corners: Default::default(),
        fill: Oklch::from_hex("#ff8800").unwrap(),
        border_widths: Default::default(),
        border_color: Oklch::TRANSPARENT,
        dashed: false,
    }
}

#[test]
fn a_pulsing_quad_damages_only_its_own_footprint_every_frame() {
    let mut rig = rig();
    rig.scene.insert(pulse());
    rig.scene.insert(quad(300.0, 200.0));
    rig.scene.insert(DisplayItem::PopGpuiQuadPulse);
    assert_eq!(settle(&mut rig, 1.0), Plan::Full);
    let plan = settle(&mut rig, 1.0);
    let Plan::Regions(regions) = &plan else {
        panic!("expected regions, got {plan:?}")
    };
    assert!(covers(
        &plan,
        Rect {
            x: 300.0,
            y: 200.0,
            w: 30.0,
            h: 30.0
        }
    ));
    let area: f32 = regions.iter().map(|r| r.w * r.h).sum();
    assert!(area < 40.0 * 40.0, "{area}");
    assert!(regions
        .iter()
        .all(|r| !(r.x < 100.0 && r.right() > 100.0 && r.y < 50.0 && r.bottom() > 50.0)));
}

#[test]
fn a_pulse_under_a_transform_damages_the_transformed_footprint() {
    let mut rig = rig();
    rig.scene
        .insert(DisplayItem::PushTransform(Transform2D::translate(5.0, 7.0)));
    rig.scene.insert(pulse());
    rig.scene.insert(quad(300.0, 200.0));
    rig.scene.insert(DisplayItem::PopGpuiQuadPulse);
    rig.scene.insert(DisplayItem::PopTransform);
    settle(&mut rig, 1.0);
    let plan = settle(&mut rig, 1.0);
    assert!(matches!(plan, Plan::Regions(_)), "{plan:?}");
    assert!(covers(
        &plan,
        Rect {
            x: 305.0,
            y: 207.0,
            w: 30.0,
            h: 30.0
        }
    ));
}

#[test]
fn a_pulse_behind_a_blur_repaints_the_whole_blur_zone_and_nothing_far_from_it() {
    let mut rig = rig();
    rig.scene.insert(blur_at(290.0, 190.0));
    rig.scene.insert(pulse());
    rig.scene.insert(quad(300.0, 200.0));
    rig.scene.insert(DisplayItem::PopGpuiQuadPulse);
    settle(&mut rig, 1.0);
    let plan = settle(&mut rig, 1.0);
    let zone = Rect {
        x: 290.0 - 48.0,
        y: 190.0 - 48.0,
        w: 50.0 + 96.0,
        h: 50.0 + 96.0,
    };
    assert!(covers(&plan, zone));
    let Plan::Regions(regions) = &plan else {
        panic!("expected regions, got {plan:?}")
    };
    assert!(regions
        .iter()
        .all(|r| !(r.x < 100.0 && r.right() > 100.0 && r.y < 50.0 && r.bottom() > 50.0)));
}

#[test]
fn a_blur_zone_that_swallows_most_of_the_frame_repaints_fully() {
    let mut rig = rig();
    rig.scene.insert(DisplayItem::BackdropBlur {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 400.0,
            h: 280.0,
        },
        radius: 8.0,
    });
    rig.scene.insert(pulse());
    rig.scene.insert(quad(300.0, 200.0));
    rig.scene.insert(DisplayItem::PopGpuiQuadPulse);
    settle(&mut rig, 1.0);
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
        run_with(scale, seed, false);
    }

    fn run_with(scale: f32, seed: u64, blurred: bool) {
        let mut rng = Lcg(seed);
        let mut scene = Scene::new();
        let mut nodes: Vec<NodeId> = (0..10).map(|_| scene.insert(item(&mut rng))).collect();
        if blurred {
            scene.insert(DisplayItem::BackdropBlur {
                bounds: Rect {
                    x: 50.0,
                    y: 30.0,
                    w: 50.0,
                    h: 40.0,
                },
                radius: 3.0,
            });
        }
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
        planner.plan(&scene, &runs, size, scale, true);
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
            let plan = planner.plan(&scene, &runs, size, scale, true);
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
    fn partial_regions_reproduce_the_full_render_around_a_backdrop_blur() {
        [21, 22, 23, 25, 26, 27]
            .into_iter()
            .for_each(|seed| run_with(1.0, seed, true));
    }

    #[test]
    fn partial_regions_reproduce_the_full_render_at_fractional_scale() {
        (11..=14).for_each(|seed| run(1.25, seed));
    }
}

#[test]
fn an_animated_quad_keeps_damaging_across_unchanged_frames() {
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushGpuiQuadPulse(
        aqua_core::scene::GpuiQuadPulse {
            start_ms: 0,
            period_ms: 1000,
            min_opacity: 0.4,
        },
    ));
    scene.insert(DisplayItem::Quad {
        bounds: Rect {
            x: 100.0,
            y: 100.0,
            w: 20.0,
            h: 20.0,
        },
        corners: aqua_core::geometry::CornerRadii::uniform(0.0),
        fill: Oklch::from_hex("#336699").unwrap(),
        border_widths: aqua_core::geometry::Insets::uniform(0.0),
        border_color: Oklch::TRANSPARENT,
        dashed: false,
    });
    scene.insert(DisplayItem::PopGpuiQuadPulse);
    let far = scene.insert(block(300.0, 200.0));
    let runs = SharedRuns::new();
    let mut planner = DamagePlanner::new();
    let animated = Rect {
        x: 100.0,
        y: 100.0,
        w: 20.0,
        h: 20.0,
    };
    assert_eq!(planner.plan(&scene, &runs, SIZE, 1.0, true), Plan::Full);
    planner.commit();
    let first = planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    let second = planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    assert!(covers(&first, animated));
    assert_eq!(first, second);
    scene.update(far, block(310.0, 200.0));
    let moved = planner.plan(&scene, &runs, SIZE, 1.0, true);
    assert!(covers(&moved, animated));
    assert!(covers(
        &moved,
        Rect {
            x: 310.0,
            y: 200.0,
            w: 20.0,
            h: 20.0
        }
    ));
    planner.commit();
    planner.invalidate();
    assert_eq!(planner.plan(&scene, &runs, SIZE, 1.0, true), Plan::Full);
}

#[test]
fn restyling_a_text_scope_damages_what_it_wraps_and_not_the_whole_frame() {
    let tint = |lightness: f32| {
        DisplayItem::PushGpuiTextColor(aqua_core::GpuiHsla {
            h: 0.55,
            s: 0.5,
            l: lightness,
            a: 1.0,
        })
    };
    let mut scene = Scene::new();
    let push = scene.insert(tint(0.4));
    scene.insert(block(20.0, 20.0));
    scene.insert(DisplayItem::PopGpuiTextColor);
    scene.insert(block(300.0, 220.0));
    let runs = SharedRuns::new();
    let mut planner = DamagePlanner::new();
    planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    scene.update(push, tint(0.8));
    let plan = planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    assert!(matches!(plan, Plan::Regions(_)), "{plan:?}");
    assert!(covers(
        &plan,
        Rect {
            x: 20.0,
            y: 20.0,
            w: 20.0,
            h: 20.0
        }
    ));
    let Plan::Regions(regions) = &plan else {
        unreachable!()
    };
    assert!(regions.iter().all(|r| r.right() < 200.0));
    scene.insert(tint(0.2));
    scene.insert(block(60.0, 120.0));
    scene.insert(DisplayItem::PopGpuiTextColor);
    let grown = planner.plan(&scene, &runs, SIZE, 1.0, true);
    assert!(matches!(grown, Plan::Regions(_)), "{grown:?}");
}
