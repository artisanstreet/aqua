use aqua_text::tone::{linear_to_srgb, srgb_to_linear, ToneCurve};

fn blended_encoded(alpha: f32, text: f32, backdrop: f32) -> f32 {
    let linear =
        srgb_to_linear(backdrop) + (srgb_to_linear(text) - srgb_to_linear(backdrop)) * alpha;
    linear_to_srgb(linear)
}

fn directwrite_encoded(curve: &ToneCurve, coverage: f32, text: f32, backdrop: f32) -> f32 {
    backdrop + (text - backdrop) * curve.adjust(coverage, text).clamp(0.0, 1.0)
}

#[test]
fn known_backdrop_reproduces_the_srgb_space_blend_for_any_pair() {
    let curve = ToneCurve::native();
    for text in [0.0f32, 0.2, 0.5, 0.78, 1.0] {
        for backdrop in [0.0f32, 0.12, 0.5, 0.8, 0.96, 1.0] {
            if (text - backdrop).abs() < 0.05 {
                continue;
            }
            for step in 0..=16 {
                let coverage = step as f32 / 16.0;
                let ours = blended_encoded(
                    curve.linear_blend_alpha_over(coverage, text, backdrop),
                    text,
                    backdrop,
                );
                let theirs = directwrite_encoded(&curve, coverage, text, backdrop);
                assert!(
                    (ours - theirs).abs() < 1.0 / 255.0,
                    "text {text} backdrop {backdrop} coverage {coverage}: {ours} vs {theirs}"
                );
            }
        }
    }
}

#[test]
fn assuming_a_contrasting_backdrop_is_visibly_wrong_on_mid_tones() {
    let curve = ToneCurve::native();
    let worst = (0..=16)
        .map(|step| {
            let coverage = step as f32 / 16.0;
            let assumed = blended_encoded(curve.linear_blend_alpha(coverage, 0.78), 0.78, 0.12);
            let exact = directwrite_encoded(&curve, coverage, 0.78, 0.12);
            (assumed - exact).abs()
        })
        .fold(0.0f32, f32::max);
    assert!(worst > 2.0 / 255.0, "worst {worst}");
}
