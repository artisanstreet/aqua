use aqua_media::reduce::{reduce, reduction_level};

#[test]
fn level_keeps_the_texture_at_least_as_large_as_the_draw() {
    assert_eq!(reduction_level(460, 460, 20.0, 20.0), 4);
    assert_eq!(reduction_level(460, 460, 230.0, 230.0), 1);
    assert_eq!(reduction_level(460, 460, 231.0, 231.0), 0);
    assert_eq!(reduction_level(460, 460, 600.0, 600.0), 0);
}

#[test]
fn the_narrower_axis_limits_the_level() {
    assert_eq!(reduction_level(1000, 100, 10.0, 50.0), 1);
    assert_eq!(reduction_level(1000, 100, 10.0, 12.0), 3);
}

#[test]
fn level_is_capped_and_survives_tiny_or_degenerate_draws() {
    assert_eq!(reduction_level(100_000, 100_000, 0.0, 0.0), 8);
    assert_eq!(reduction_level(1, 1, 1.0, 1.0), 0);
}

#[test]
fn halving_averages_premultiplied_channels() {
    let source = [100, 0, 0, 100, 200, 0, 0, 200, 0, 40, 0, 255, 0, 80, 0, 255];
    let reduced = reduce(&source, 2, 2, 1);
    assert_eq!((reduced.width, reduced.height), (1, 1));
    assert_eq!(reduced.pixels, vec![75, 30, 0, 203]);
}

#[test]
fn odd_sizes_round_down_and_zero_levels_copy() {
    let source = vec![9u8; 5 * 3 * 4];
    let reduced = reduce(&source, 5, 3, 1);
    assert_eq!((reduced.width, reduced.height), (2, 1));
    assert!(reduced.pixels.iter().all(|value| *value == 9));
    assert_eq!(reduce(&source, 5, 3, 0).pixels, source);
}
