use aqua_gpu_ref::conformance::realistic::cases;
use aqua_gpu_ref::conformance::Session;
use aqua_gpu_ref::RefBackend;
use aqua_protocol::SurfacePixels;

fn render(name: &str) -> SurfacePixels {
    let case = cases()
        .into_iter()
        .find(|case| case.name == name)
        .unwrap_or_else(|| panic!("missing case {name}"));
    Session::new(&mut RefBackend::new())
        .render_case(&case)
        .expect("renders")
}

fn red(pixels: &SurfacePixels, x: usize, y: usize) -> u8 {
    pixels.rgba8[(y * pixels.extent.width as usize + x) * 4]
}

fn row_step(pixels: &SurfacePixels, y: usize, from: usize, to: usize) -> u8 {
    (from..to - 1)
        .map(|x| red(pixels, x, y).abs_diff(red(pixels, x + 1, y)))
        .max()
        .unwrap_or(0)
}

#[test]
fn blur_over_text_flattens_the_glyphs_inside_and_keeps_them_outside() {
    let pixels = render("blur_over_text_light_on_dark");
    let inside = row_step(&pixels, 56, 40, 120);
    let outside = row_step(&pixels, 4, 4, 150);
    assert!(inside <= 24, "inside step {inside}");
    assert!(outside >= 128, "outside step {outside}");
}

#[test]
fn glass_over_text_keeps_text_legible_above_the_pane() {
    let pixels = render("glass_over_text_with_label");
    let label = row_step(&pixels, 48, 34, 90);
    let backdrop = row_step(&pixels, 76, 34, 90);
    assert!(
        label > 3 * backdrop.max(1),
        "label {label} backdrop {backdrop}"
    );
}

#[test]
fn distinct_corner_radii_cut_each_corner_differently() {
    let pixels = render("corner_clip_four_distinct_radii");
    let blue = |x: usize, y: usize| pixels.rgba8[(y * pixels.extent.width as usize + x) * 4 + 2];
    assert!(blue(22, 20) > 180, "small radius keeps its corner");
    assert!(blue(138, 20) < 120, "large radius cuts its corner");
    assert!(blue(19, 95) < 120, "bottom-left radius 18 cuts its corner");
    assert!(
        blue(141, 95) > 180,
        "bottom-right radius 2 keeps its corner"
    );
}

#[test]
fn top_rounded_chip_has_square_bottom_corners() {
    let pixels = render("corner_clip_top_rounded_chip");
    assert_eq!(red(&pixels, 25, 79), 232);
    assert_ne!(red(&pixels, 25, 41), 232);
}

#[test]
fn shadow_is_clipped_to_the_rounded_clip() {
    let pixels = render("shadow_clipped_by_rounded_clip");
    let outside = red(&pixels, 5, 56);
    let inside = red(&pixels, 12, 70);
    assert_ne!(outside, inside);
}
