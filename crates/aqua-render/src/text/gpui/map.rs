use palette::{Hsla, IntoColor, rgb::Rgba};

pub fn shimmer_color(
    base: [f32; 4],
    target: [f32; 4],
    position: f32,
    phase: f32,
    spread: f32,
) -> [f32; 4] {
    let phase = if phase.is_finite() {
        phase.rem_euclid(1.0)
    } else {
        0.0
    };
    let spread = if spread.is_finite() {
        spread.clamp(0.0, 100.0)
    } else {
        0.0
    };
    let width = spread / 100.0;
    if width <= 0.0 || !position.is_finite() {
        return base;
    }
    let start = (1.0 - width) * 3.5f32.mul_add(-phase, 2.5);
    let offset = (position - start) / width;
    let strength = (offset / 0.4).min((1.0 - offset) / 0.4).clamp(0.0, 1.0);
    if strength <= 0.0 {
        return base;
    }
    let amount = strength * 0.55;
    let mut color = rgba(base);
    let target = rgba(target);
    color.red = (target.red - color.red).mul_add(amount, color.red);
    color.green = (target.green - color.green).mul_add(amount, color.green);
    color.blue = (target.blue - color.blue).mul_add(amount, color.blue);
    let mapped: Hsla = color.into_color();
    [
        mapped.hue.into_raw_degrees(),
        mapped.saturation,
        mapped.lightness,
        mapped.alpha,
    ]
}

fn rgba(color: [f32; 4]) -> Rgba {
    Hsla::new(color[0], color[1], color[2], color[3]).into_color()
}

pub fn indistinguishable(a: [f32; 4], b: [f32; 4]) -> bool {
    let a = rgba(a);
    let b = rgba(b);
    const STEP: f32 = 1.0 / 255.0;
    (a.red - b.red).abs() <= STEP
        && (a.green - b.green).abs() <= STEP
        && (a.blue - b.blue).abs() <= STEP
        && (a.alpha - b.alpha).abs() <= STEP
}
