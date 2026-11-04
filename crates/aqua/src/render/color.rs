use aqua_core::{GpuiHsla, GpuiTextColor, Oklch};

pub type Srgba = [f32; 4];

#[must_use]
pub fn oklch(color: Srgba) -> Oklch {
    let byte = |value: f32| (value.clamp(0.0, 1.0) * 255.0).round() as u8;
    Oklch::from_srgb8(byte(color[0]), byte(color[1]), byte(color[2]), color[3])
}

#[must_use]
pub fn srgb(color: Oklch) -> Srgba {
    let [r, g, b, a] = color.to_srgb8();
    [
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
        f32::from(a) / 255.0,
    ]
}

#[must_use]
pub fn hsla(color: GpuiHsla) -> Srgba {
    color.to_srgba()
}

#[must_use]
pub fn hsla_array(color: [f32; 4]) -> Srgba {
    hsla(GpuiHsla {
        h: color[0],
        s: color[1],
        l: color[2],
        a: color[3],
    })
}

#[must_use]
pub fn source_array(color: [f32; 4]) -> Srgba {
    hsla_array([
        color[0].rem_euclid(360.0) / 360.0,
        color[1],
        color[2],
        color[3],
    ])
}

#[must_use]
pub fn text_color(color: GpuiTextColor) -> Srgba {
    source_array([
        color.hue_degrees,
        color.saturation,
        color.lightness,
        color.alpha,
    ])
}

#[must_use]
pub fn with_alpha(color: Srgba, factor: f32) -> Srgba {
    [
        color[0],
        color[1],
        color[2],
        (color[3] * factor).clamp(0.0, 1.0),
    ]
}

fn mix(base: Srgba, target: Srgba, amount: f32) -> Srgba {
    [
        base[0] + (target[0] - base[0]) * amount,
        base[1] + (target[1] - base[1]) * amount,
        base[2] + (target[2] - base[2]) * amount,
        base[3],
    ]
}

#[must_use]
pub fn shimmer(base: Srgba, target: Srgba, position: f32, phase: f32, spread: f32) -> Srgba {
    let phase = match phase.is_finite() {
        true => phase.rem_euclid(1.0),
        false => 0.0,
    };
    let spread = match spread.is_finite() {
        true => spread.clamp(0.0, 100.0),
        false => 0.0,
    };
    let width = spread / 100.0;
    match width <= 0.0 || !position.is_finite() {
        true => base,
        false => {
            let start = (1.0 - width) * 3.5f32.mul_add(-phase, 2.5);
            let offset = (position - start) / width;
            let strength = (offset / 0.4).min((1.0 - offset) / 0.4).clamp(0.0, 1.0);
            mix(base, target, strength * 0.55)
        }
    }
}

#[must_use]
pub fn legacy_map(base: Srgba, target: Srgba, offset: f32, strength: f32) -> Srgba {
    let bell = (offset / 0.4).min((1.0 - offset) / 0.4).clamp(0.0, 1.0);
    mix(base, target, bell * strength.clamp(0.0, 1.0))
}
