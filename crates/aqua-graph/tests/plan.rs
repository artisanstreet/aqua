use aqua_core::{Oklch, Rect};
use aqua_graph::{
    plan_paint, BackdropBlur, Extent, PaintCommand, Pass, PassKind, TextureRef, TransientId,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

#[test]
fn repaint_closes_over_chained_backdrop_sources() {
    let extent = Extent {
        width: 800,
        height: 600,
    };
    let commands: Vec<_> = [
        rect(300.0, 200.0, 200.0, 150.0),
        rect(200.0, 100.0, 200.0, 150.0),
    ]
    .into_iter()
    .map(|bounds| {
        PaintCommand::BackdropBlur(BackdropBlur {
            bounds,
            radius: 12.0,
            corner_radius: 0.0,
            mask: None,
        })
    })
    .collect();
    let plan = plan_paint(extent, Oklch::BLACK, &commands).unwrap();
    let damage = rect(210.0, 110.0, 10.0, 10.0);
    let expanded = plan.repaint_bounds(damage);
    for pass in &plan.passes {
        if let PassKind::Filter { params, .. } = &pass.kind {
            if pass.reads.contains(&TextureRef::Surface) {
                assert_eq!(expanded.union(params.region), expanded);
            }
        }
    }
    assert!(expanded.right() >= 500.0);
    assert!(expanded.bottom() >= 350.0);
    assert_eq!(plan.repaint_bounds(expanded), expanded);
    let unrelated = rect(700.0, 500.0, 10.0, 10.0);
    assert_eq!(plan.repaint_bounds(unrelated), unrelated);
}

#[test]
fn rounded_layer_snapshot_is_reconstructed_in_full() {
    let extent = Extent {
        width: 800,
        height: 600,
    };
    let mut plan = plan_paint(extent, Oklch::BLACK, &[]).unwrap();
    let mut copy = Pass::clear(TextureRef::Transient(TransientId(0)), Oklch::TRANSPARENT);
    copy.kind = PassKind::Copy {
        source_origin: [200, 150],
        dest_origin: [0, 0],
        size: Extent {
            width: 300,
            height: 200,
        },
    };
    copy.reads.push(TextureRef::Surface);
    plan.passes.push(copy);
    assert_eq!(
        plan.repaint_bounds(rect(250.0, 200.0, 20.0, 20.0)),
        rect(200.0, 150.0, 300.0, 200.0)
    );
    assert_eq!(
        plan.repaint_bounds(rect(10.0, 10.0, 20.0, 20.0)),
        rect(10.0, 10.0, 20.0, 20.0)
    );
}
