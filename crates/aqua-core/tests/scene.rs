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
