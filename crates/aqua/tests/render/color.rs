use aqua::render::color::{legacy_map, shimmer, source_array, text_color};
use aqua_core::GpuiTextColor;

#[test]
fn shimmer_leaves_pixels_outside_the_band_untouched() {
    let base = [0.2, 0.2, 0.2, 1.0];
    let target = [1.0, 1.0, 1.0, 1.0];
    assert_eq!(shimmer(base, target, f32::NAN, 0.0, 30.0), base);
    assert_eq!(shimmer(base, target, 0.5, 0.0, 0.0), base);
    assert_eq!(shimmer(base, target, -5.0, 0.0, 30.0), base);
}

#[test]
fn shimmer_peaks_at_the_middle_of_the_band_and_moves_with_phase() {
    let base = [0.0, 0.0, 0.0, 1.0];
    let target = [1.0, 1.0, 1.0, 1.0];
    let spread = 40.0;
    let width = spread / 100.0;
    let centre = |phase: f32| (1.0 - width) * 3.5f32.mul_add(-phase, 2.5) + width * 0.5;
    let at_centre = shimmer(base, target, centre(0.5), 0.5, spread);
    assert!((at_centre[0] - 0.55).abs() < 1e-4);
    let later = shimmer(base, target, centre(0.5), 0.6, spread);
    assert!(later[0] < at_centre[0]);
}

#[test]
fn legacy_map_is_a_bell_over_the_range() {
    let base = [0.0, 0.0, 0.0, 1.0];
    let target = [1.0, 1.0, 1.0, 1.0];
    assert_eq!(legacy_map(base, target, -0.1, 1.0)[0], 0.0);
    assert!((legacy_map(base, target, 0.5, 1.0)[0] - 1.0).abs() < 1e-6);
    assert!((legacy_map(base, target, 0.5, 0.5)[0] - 0.5).abs() < 1e-6);
}

#[test]
fn source_hues_are_degrees_and_text_colors_match() {
    let red = source_array([0.0, 1.0, 0.5, 1.0]);
    assert!(red[0] > 0.99 && red[1] < 0.01);
    let green = text_color(GpuiTextColor {
        hue_degrees: 120.0,
        saturation: 1.0,
        lightness: 0.5,
        alpha: 0.5,
    });
    assert!(green[1] > 0.99 && green[0] < 0.01);
    assert_eq!(green[3], 0.5);
}
