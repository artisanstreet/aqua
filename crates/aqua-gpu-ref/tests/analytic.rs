use aqua_core::{geometry::Rect, Oklch};
use aqua_gpu_ref::conformance::cases::{build, hex, rect, shadow};
use aqua_gpu_ref::conformance::Session;
use aqua_gpu_ref::erf::normal_cdf;
use aqua_gpu_ref::shadow::blurred_coverage;
use aqua_gpu_ref::RefBackend;
use aqua_graph::{
    Extent, Gradient, GradientQuad, GradientStop, PaintCommand, Quad, ShadowInstance,
};

fn instance(
    bounds: Rect,
    radius: f32,
    sigma: f32,
    spread: f32,
    offset: [f32; 2],
) -> ShadowInstance {
    ShadowInstance {
        bounds,
        radius,
        offset,
        sigma,
        spread,
        color: hex("#000000", 1.0),
        clip: None,
        opacity: 1.0,
    }
}

fn brute_force(shadow: &ShadowInstance, px: f32, py: f32) -> f64 {
    let sigma = shadow.sigma as f64;
    let half = (
        shadow.bounds.w as f64 / 2.0 + shadow.spread as f64,
        shadow.bounds.h as f64 / 2.0 + shadow.spread as f64,
    );
    let center = (
        shadow.bounds.x as f64 + shadow.bounds.w as f64 / 2.0 + shadow.offset[0] as f64,
        shadow.bounds.y as f64 + shadow.bounds.h as f64 / 2.0 + shadow.offset[1] as f64,
    );
    let radius = (shadow.radius as f64 + shadow.spread as f64)
        .max(0.0)
        .min(half.0.min(half.1));
    let step = 0.1;
    let reach = 5.0 * sigma;
    let mut total = 0.0;
    let mut mass = 0.0;
    let mut y = py as f64 - reach;
    while y <= py as f64 + reach {
        let mut x = px as f64 - reach;
        while x <= px as f64 + reach {
            let weight = (-(((x - px as f64).powi(2) + (y - py as f64).powi(2))
                / (2.0 * sigma * sigma)))
                .exp();
            let (qx, qy) = (
                (x - center.0).abs() - half.0 + radius,
                (y - center.1).abs() - half.1 + radius,
            );
            let distance =
                (qx.max(0.0).powi(2) + qy.max(0.0).powi(2)).sqrt() + qx.max(qy).min(0.0) - radius;
            mass += weight;
            if distance <= 0.0 {
                total += weight;
            }
            x += step;
        }
        y += step;
    }
    total / mass
}

#[test]
fn square_shadow_matches_the_separable_erf_closed_form() {
    let shadow = instance(rect(20.0, 20.0, 40.0, 30.0), 0.0, 5.0, 0.0, [0.0, 0.0]);
    for (px, py) in [
        (40.5, 35.5),
        (20.5, 35.5),
        (15.5, 20.5),
        (62.5, 52.5),
        (10.5, 10.5),
        (80.5, 35.5),
    ] {
        let axis = |p: f64, low: f64, high: f64| {
            normal_cdf((p - low) / 5.0) - normal_cdf((p - high) / 5.0)
        };
        let expected = axis(px as f64, 20.0, 60.0) * axis(py as f64, 20.0, 50.0);
        let actual = blurred_coverage(&shadow, px, py) as f64;
        assert!(
            (actual - expected).abs() < 2e-4,
            "({px},{py}) {actual} vs {expected}"
        );
    }
}

#[test]
fn rounded_shadow_matches_dense_two_dimensional_integration() {
    let cases = [
        instance(rect(20.0, 20.0, 24.0, 24.0), 12.0, 3.0, 0.0, [0.0, 0.0]),
        instance(rect(20.0, 20.0, 30.0, 18.0), 7.0, 2.5, 2.0, [3.0, -2.0]),
        instance(rect(20.0, 20.0, 30.0, 18.0), 4.0, 4.0, -3.0, [0.0, 0.0]),
    ];
    for shadow in &cases {
        for (px, py) in [
            (23.5, 23.5),
            (30.5, 29.5),
            (20.5, 20.5),
            (45.5, 38.5),
            (35.5, 18.5),
            (50.5, 40.5),
        ] {
            let expected = brute_force(shadow, px, py);
            let actual = blurred_coverage(shadow, px, py) as f64;
            assert!(
                (actual - expected).abs() < 6e-3,
                "{shadow:?} ({px},{py}) {actual} vs {expected}"
            );
        }
    }
}

#[test]
fn hard_shadow_is_a_signed_distance_edge_and_zero_spread_edge_is_half_coverage() {
    let hard = instance(rect(10.0, 10.0, 20.0, 20.0), 0.0, 0.0, 0.0, [0.0, 0.0]);
    assert_eq!(blurred_coverage(&hard, 20.5, 20.5), 1.0);
    assert_eq!(blurred_coverage(&hard, 40.5, 20.5), 0.0);
    assert!((blurred_coverage(&hard, 10.0, 20.5) - 0.5).abs() < 1e-6);
    let soft = instance(rect(-200.0, 10.0, 400.0, 400.0), 0.0, 6.0, 0.0, [0.0, 0.0]);
    assert!((blurred_coverage(&soft, 20.0, 10.0) - 0.5).abs() < 2e-4);
}

fn render_pixels(plan_case: &aqua_gpu_ref::conformance::Case) -> aqua_protocol::SurfacePixels {
    let mut backend = RefBackend::new();
    Session::new(&mut backend)
        .render_case(plan_case)
        .expect("renders")
}

fn gradient_case(from: Oklch, to: Oklch) -> aqua_gpu_ref::conformance::Case {
    let extent = Extent::new(64, 4);
    build(
        "gradient",
        extent,
        Oklch::TRANSPARENT,
        &[PaintCommand::GradientQuad(GradientQuad {
            quad: Quad {
                bounds: rect(0.0, 0.0, 64.0, 4.0),
                radius: 0.0,
                border_width: 0.0,
                fill: Oklch::TRANSPARENT,
                border_color: Oklch::TRANSPARENT,
            },
            gradient: Gradient {
                start: [0.0, 0.0],
                end: [64.0, 0.0],
                stops: [
                    GradientStop {
                        offset: 0.0,
                        color: from,
                    },
                    GradientStop {
                        offset: 1.0,
                        color: to,
                    },
                ],
            },
        })],
    )
}

#[test]
fn gradient_interpolates_lightness_linearly_in_oklab() {
    let pixels = render_pixels(&gradient_case(hex("#000000", 1.0), hex("#ffffff", 1.0)));
    let red_at = |x: usize| pixels.rgba8[x * 4] as f32 / 255.0;
    let lightness_to_linear = |x: usize| {
        let l = (x as f32 + 0.5) / 64.0;
        l * l * l
    };
    for x in [0usize, 15, 31, 47, 63] {
        assert!(
            (red_at(x) - lightness_to_linear(x)).abs() < 2.5 / 255.0,
            "x={x}: {} vs {}",
            red_at(x),
            lightness_to_linear(x)
        );
    }
}

#[test]
fn gradient_alpha_is_interpolated_straight_then_premultiplied() {
    let pixels = render_pixels(&gradient_case(hex("#ffffff", 1.0), hex("#ffffff", 0.0)));
    for x in [8usize, 32, 56] {
        let alpha = pixels.rgba8[x * 4 + 3] as f32 / 255.0;
        let expected = 1.0 - (x as f32 + 0.5) / 64.0;
        assert!(
            (alpha - expected).abs() < 1.5 / 255.0,
            "x={x}: {alpha} vs {expected}"
        );
        let red = pixels.rgba8[x * 4] as f32 / 255.0;
        assert!(
            (red - expected).abs() < 2.0 / 255.0,
            "white stays white once premultiplied"
        );
    }
}

#[test]
fn shadow_alpha_never_exceeds_the_color_alpha_and_decays_monotonically_outside() {
    let extent = Extent::new(80, 40);
    let case = build(
        "shadow",
        extent,
        Oklch::TRANSPARENT,
        &[shadow(
            rect(20.0, 8.0, 30.0, 24.0),
            6.0,
            [0.0, 0.0],
            5.0,
            0.0,
            "#000000",
            0.6,
        )],
    );
    let pixels = render_pixels(&case);
    let alpha = |x: usize, y: usize| pixels.rgba8[(y * 80 + x) * 4 + 3];
    assert!(pixels
        .rgba8
        .chunks(4)
        .all(|p| p[3] <= (0.6f32 * 255.0).round() as u8));
    let row: Vec<u8> = (50..80).map(|x| alpha(x, 20)).collect();
    assert!(row.windows(2).all(|pair| pair[0] >= pair[1]));
    assert_eq!(alpha(79, 20), 0);
    assert!(alpha(35, 20) >= 150);
}

fn dense_reference(shadow: &ShadowInstance, px: f64, py: f64) -> f64 {
    let sigma = shadow.sigma as f64;
    let spread = shadow.spread as f64;
    let half = (
        shadow.bounds.w as f64 / 2.0 + spread,
        shadow.bounds.h as f64 / 2.0 + spread,
    );
    let center = (
        shadow.bounds.x as f64 + shadow.bounds.w as f64 / 2.0 + shadow.offset[0] as f64,
        shadow.bounds.y as f64 + shadow.bounds.h as f64 / 2.0 + shadow.offset[1] as f64,
    );
    let radius = (shadow.radius as f64 + spread)
        .max(0.0)
        .min(half.0.min(half.1));
    let low = (center.1 - half.1).max(py - 6.0 * sigma);
    let high = (center.1 + half.1).min(py + 6.0 * sigma);
    let steps = (((high - low) / 0.05).ceil() as usize).max(1);
    let step = (high - low) / steps as f64;
    let scale = sigma * std::f64::consts::SQRT_2;
    (0..steps)
        .map(|index| {
            let y = low + step * (index as f64 + 0.5);
            let dy = (y - center.1).abs();
            let straight = half.1 - radius;
            let width = match dy <= straight {
                true => half.0,
                false => {
                    let into = dy - straight;
                    half.0 - radius + (radius * radius - into * into).max(0.0).sqrt()
                }
            };
            let weight = normal_cdf((y + step / 2.0 - py) / sigma)
                - normal_cdf((y - step / 2.0 - py) / sigma);
            let dx = px - center.0;
            weight
                * 0.5
                * (aqua_gpu_ref::erf::erf((dx + width) / scale)
                    - aqua_gpu_ref::erf::erf((dx - width) / scale))
        })
        .sum()
}

#[test]
fn large_shadows_render_a_1440p_frame_quickly_and_match_dense_integration() {
    use aqua_gpu_ref::raster::Layer;
    use aqua_gpu_ref::shadow::draw_shadow;
    let shadow = instance(
        rect(300.0, 200.0, 1800.0, 900.0),
        24.0,
        40.0,
        10.0,
        [0.0, 20.0],
    );
    let mut layer = Layer::filled(2560, 1440, [0.0; 4]);
    let started = std::time::Instant::now();
    draw_shadow(&mut layer, &shadow);
    assert!(
        started.elapsed().as_secs_f32() < 10.0,
        "{:?}",
        started.elapsed()
    );
    [
        (1200usize, 600usize),
        (320, 220),
        (150, 600),
        (2100, 1100),
        (1200, 1000),
        (305, 205),
    ]
    .iter()
    .for_each(|&(x, y)| {
        let expected = dense_reference(&shadow, x as f64 + 0.5, y as f64 + 0.5);
        let actual = layer.at(x, y)[3] as f64;
        assert!(
            (actual - expected).abs() < 5e-4,
            "({x},{y}) {actual} vs {expected}"
        );
    });
}
