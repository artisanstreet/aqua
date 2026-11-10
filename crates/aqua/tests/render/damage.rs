use aqua::render::damage::DamagePlanner;
use aqua_core::{DisplayItem, Rect, Size};
use aqua_scene::Scene;
use aqua_text::SharedRuns;

const SIZE: Size = Size { w: 400.0, h: 300.0 };

#[test]
fn retained_buffer_repaint_expands_culling_across_blur_zones() {
    let mut scene = Scene::new();
    let runs = SharedRuns::new();
    let mut planner = DamagePlanner::new();
    scene.insert(DisplayItem::BackdropBlur {
        bounds: Rect {
            x: 140.0,
            y: 100.0,
            w: 80.0,
            h: 100.0,
        },
        radius: 4.0,
    });
    planner.plan(&scene, &runs, SIZE, 1.5, true);
    planner.commit();
    planner.plan(&scene, &runs, SIZE, 1.5, true);
    planner.commit();
    let retained = Rect {
        x: 30.0,
        y: 30.0,
        w: 510.0,
        h: 180.0,
    };
    let expanded = planner.repaint_bounds(retained);
    assert_eq!(
        expanded,
        Rect {
            x: 30.0,
            y: 30.0,
            w: 510.0,
            h: 318.0
        }
    );
    assert_eq!(planner.repaint_bounds(expanded), expanded);
    let unrelated = Rect {
        x: 10.0,
        y: 10.0,
        w: 20.0,
        h: 20.0,
    };
    assert_eq!(planner.repaint_bounds(unrelated), unrelated);
}

#[test]
fn backdrop_inside_a_rounded_layer_expands_the_snapshot_cull() {
    let mut scene = Scene::new();
    let runs = SharedRuns::new();
    let mut planner = DamagePlanner::new();
    let clip = Rect {
        x: 40.0,
        y: 40.0,
        w: 300.0,
        h: 220.0,
    };
    scene.insert(DisplayItem::PushClip {
        bounds: clip,
        radius: 12.0,
    });
    scene.insert(DisplayItem::BackdropBlur {
        bounds: Rect {
            x: 160.0,
            y: 120.0,
            w: 20.0,
            h: 20.0,
        },
        radius: 2.0,
    });
    scene.insert(DisplayItem::PopClip);
    planner.plan(&scene, &runs, SIZE, 1.0, true);
    planner.commit();
    let region = Rect {
        x: 150.0,
        y: 120.0,
        w: 5.0,
        h: 5.0,
    };
    assert_eq!(planner.repaint_bounds(region), clip);
}
