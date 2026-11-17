use std::time::Duration;

use aqua_core::scene::GpuiShadowLayer;
use aqua_core::{GpuiBackground, GpuiHsla, Point, Transform2D};
use aqua_motion::Easing;
use aqua_style::timeline::{Key, Timeline};
use aqua_style::Paint;

const CLEAR: GpuiHsla = GpuiHsla {
    h: 0.0,
    s: 0.0,
    l: 0.0,
    a: 0.0,
};

fn rest() -> Paint {
    Paint {
        opacity: Some(0.8),
        ..Paint::default()
    }
}

fn fade() -> Timeline {
    Timeline::new(Duration::from_secs(1)).opacity([
        Key::new(0.0, 0.0, Easing::Linear),
        Key::new(0.5, 1.0, Easing::Linear),
    ])
}

#[test]
fn a_lane_eases_between_keys_and_holds_its_ends() {
    let timeline = fade();
    assert_eq!(timeline.sample(&rest(), (0.0, 0.0), 0.0).opacity, Some(0.0));
    assert_eq!(
        timeline.sample(&rest(), (0.0, 0.0), 0.25).opacity,
        Some(0.4)
    );
    assert_eq!(
        timeline.sample(&rest(), (0.0, 0.0), 0.75).opacity,
        Some(0.8)
    );
}

#[test]
fn a_finished_run_shows_the_resting_paint_untouched() {
    let held = Timeline::new(Duration::from_secs(1)).opacity([Key::new(0.0, 0.5, Easing::Linear)]);
    assert_eq!(held.sample(&rest(), (0.0, 0.0), 1.0), rest());
}

#[test]
fn a_held_run_keeps_its_last_keys_after_it_finishes() {
    let vanish = Timeline::new(Duration::from_secs(1))
        .opacity([
            Key::new(0.0, 1.0, Easing::Linear),
            Key::new(1.0, 0.0, Easing::Linear),
        ])
        .hold();
    assert_eq!(vanish.sample(&rest(), (0.0, 0.0), 2.0).opacity, Some(0.0));
}

#[test]
fn scale_pivots_on_a_fraction_of_the_node() {
    let timeline = Timeline::new(Duration::from_secs(1)).scale(
        (1.0, 0.0),
        [
            Key::new(0.0, 2.0, Easing::Linear),
            Key::new(1.0, 1.0, Easing::Linear),
        ],
    );
    let transform = timeline
        .sample(&Paint::default(), (100.0, 40.0), 0.0)
        .transform
        .unwrap();
    assert_eq!(
        transform.apply(Point::new(100.0, 0.0)),
        Point::new(100.0, 0.0)
    );
    assert_eq!(
        transform.apply(Point::new(50.0, 10.0)),
        Point::new(0.0, 20.0)
    );
}

#[test]
fn motion_is_applied_inside_the_resting_transform() {
    let rest = Paint {
        transform: Some(Transform2D::translate(10.0, 0.0)),
        ..Paint::default()
    };
    let timeline = Timeline::new(Duration::from_secs(1)).translate([Key::new(
        0.0,
        (5.0, 5.0),
        Easing::Linear,
    )]);
    let transform = timeline.sample(&rest, (0.0, 0.0), 0.0).transform.unwrap();
    assert_eq!(transform.apply(Point::new(0.0, 0.0)), Point::new(15.0, 5.0));
}

#[test]
fn a_shadow_lane_adds_a_layer_over_the_resting_shadows() {
    let base = GpuiShadowLayer {
        dx: 0.0,
        dy: 1.0,
        blur: 2.0,
        spread: 0.0,
        inset: false,
        color: CLEAR,
    };
    let lift = GpuiShadowLayer {
        dy: 12.0,
        blur: 28.0,
        ..base
    };
    let rest = Paint {
        gpui_shadows: vec![base],
        ..Paint::default()
    };
    let timeline = Timeline::new(Duration::from_secs(1)).shadow([
        Key::new(0.0, lift, Easing::Linear),
        Key::new(
            1.0,
            GpuiShadowLayer {
                dy: 0.0,
                blur: 0.0,
                ..base
            },
            Easing::Linear,
        ),
    ]);
    let shadows = timeline.sample(&rest, (0.0, 0.0), 0.5).gpui_shadows;
    assert_eq!(shadows.len(), 2);
    assert_eq!(shadows[0], base);
    assert_eq!((shadows[1].dy, shadows[1].blur), (6.0, 14.0));
}

#[test]
fn gradient_backgrounds_blend_stop_by_stop() {
    let gradient = |edge: f32| GpuiBackground::LinearGradient {
        angle: 45.0,
        color_space: aqua_core::GpuiGradientColorSpace::Oklab,
        stops: [
            aqua_core::GpuiGradientStop {
                percentage: edge,
                color: CLEAR,
            },
            aqua_core::GpuiGradientStop {
                percentage: edge + 0.1,
                color: CLEAR,
            },
        ],
    };
    let timeline = Timeline::new(Duration::from_secs(1)).background([
        Key::new(0.0, gradient(1.0), Easing::Linear),
        Key::new(1.0, gradient(0.0), Easing::Linear),
    ]);
    assert_eq!(
        timeline
            .sample(&Paint::default(), (0.0, 0.0), 0.5)
            .gpui_background,
        Some(gradient(0.5))
    );
}
