use aqua_core::Oklch;
use aqua_gpu_d3d11::memo::ColorMemo;

fn palette(count: u32) -> Vec<Oklch> {
    (0..count)
        .map(|index| Oklch {
            l: (index % 97) as f32 / 97.0,
            c: (index % 13) as f32 * 0.02,
            h: (index * 37 % 360) as f32,
            alpha: 0.25 + (index % 4) as f32 * 0.25,
        })
        .collect()
}

#[test]
fn memoized_conversions_equal_direct_conversions_through_collisions() {
    let mut memo = ColorMemo::default();
    let colors = palette(3000);
    for pass in 0..3 {
        for color in &colors {
            assert_eq!(memo.premultiplied(*color), color.to_gpu(), "pass {pass}");
            assert_eq!(memo.straight(*color), color.to_srgba(), "pass {pass}");
        }
    }
}

#[test]
fn the_two_tables_do_not_alias_each_other() {
    let mut memo = ColorMemo::default();
    let color = Oklch {
        l: 0.6,
        c: 0.1,
        h: 40.0,
        alpha: 0.5,
    };
    assert_ne!(memo.premultiplied(color), memo.straight(color));
    assert_eq!(memo.premultiplied(color), color.to_gpu());
    assert_eq!(memo.straight(color), color.to_srgba());
}
