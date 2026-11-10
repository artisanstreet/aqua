use aqua_core::{
    geometry::{CornerRadii, Rect},
    scene::{DisplayItem, ShadowLayer},
    Oklch,
};

#[test]
fn shadow_bounds_include_offset_spread_and_gaussian_tail() {
    let bounds = Rect::from_edges(10.0, 20.0, 40.0, 60.0);
    let mut layer = ShadowLayer {
        dx: -3.0,
        dy: 4.0,
        blur: 8.0,
        spread: -2.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let item = DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(3.0),
        layer,
    };
    assert_eq!(
        item.bounds(),
        Some(Rect::from_edges(-3.0, 14.0, 47.0, 74.0))
    );
    layer.inset = true;
    let inset = DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(3.0),
        layer,
    };
    assert_eq!(inset.bounds(), Some(bounds));
}

#[test]
fn native_fill_batches_keep_live_clones_independent_across_threads() {
    use aqua_core::{NodeId, RawId, Srgba8, SrgbaFillBatch, SrgbaFillPatch};
    let patch = SrgbaFillPatch {
        node: NodeId::from_raw(RawId {
            index: 7,
            generation: 3,
        }),
        fill: Srgba8::new(1, 2, 3, 4),
    };
    let mut first = SrgbaFillBatch::default();
    first.push(patch);
    let copy = first.clone();
    let mut next = first.sibling();
    next.push(SrgbaFillPatch {
        fill: Srgba8::new(5, 6, 7, 8),
        ..patch
    });
    let copy = std::thread::spawn(move || {
        let mut local = copy.sibling();
        local.extend_from_slice(&copy);
        local[0].fill = Srgba8::new(9, 10, 11, 12);
        assert_eq!(copy[0], patch);
        local
    })
    .join()
    .unwrap();
    assert_eq!(first[0], patch);
    assert_eq!(next[0].fill, Srgba8::new(5, 6, 7, 8));
    assert_eq!(copy[0].fill, Srgba8::new(9, 10, 11, 12));
    drop(next);
    let reused = first.sibling();
    assert!(reused.is_empty());
    assert_eq!(first[0], patch);
}
