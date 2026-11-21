fn smooth(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn srgb(value: f32) -> f32 {
    if value <= 0.003_130_8 {
        value * 12.92
    } else {
        1.055 * value.max(0.0).powf(1.0 / 2.4) - 0.055
    }
}

fn gradient_segment(
    a: [f32; 3],
    b: [f32; 3],
    slope_a: [f32; 3],
    slope_b: [f32; 3],
    width: f32,
    position: f32,
) -> [f32; 3] {
    let t = (position / width).clamp(0.0, 1.0);
    let t2 = t * t;
    let t3 = t2 * t;
    std::array::from_fn(|c| {
        (2.0 * t3 - 3.0 * t2 + 1.0) * a[c]
            + (t3 - 2.0 * t2 + t) * width * slope_a[c]
            + (-2.0 * t3 + 3.0 * t2) * b[c]
            + (t3 - t2) * width * slope_b[c]
    })
}

pub(super) fn sky_color(p: &[f32], altitude: f32) -> [f32; 3] {
    let colors: [[f32; 3]; 5] = std::array::from_fn(|i| std::array::from_fn(|c| p[8 + i * 4 + c]));
    let slope = |a: usize, b: usize, width: f32| {
        std::array::from_fn(|c| (colors[b][c] - colors[a][c]) / width)
    };
    let slopes = [
        slope(0, 1, 0.24),
        slope(0, 2, 0.46),
        slope(1, 3, 0.44),
        slope(2, 4, 0.42),
        slope(3, 4, 0.20),
    ];
    let (i, width, position) = match altitude {
        t if t < 0.24 => (0, 0.24, t),
        t if t < 0.46 => (1, 0.22, t - 0.24),
        t if t < 0.68 => (2, 0.22, t - 0.46),
        t => (3, 0.20, t - 0.68),
    };
    lab_srgb(gradient_segment(
        colors[i],
        colors[i + 1],
        slopes[i],
        slopes[i + 1],
        width,
        position,
    ))
}

fn lab_srgb(lab: [f32; 3]) -> [f32; 3] {
    let l = (lab[0] + 0.396_337_78 * lab[1] + 0.215_803_76 * lab[2]).powi(3);
    let m = (lab[0] - 0.105_561_346 * lab[1] - 0.063_854_17 * lab[2]).powi(3);
    let s = (lab[0] - 0.089_484_18 * lab[1] - 1.291_485_5 * lab[2]).powi(3);
    [
        4.076_741_7 * l - 3.307_711_6 * m + 0.230_969_94 * s,
        -1.268_438 * l + 2.609_757_4 * m - 0.341_319_38 * s,
        -0.004_196_086_3 * l - 0.703_418_6 * m + 1.707_614_7 * s,
    ]
    .map(|v| srgb(v.max(0.0)))
}

fn hash(p: [f32; 2]) -> f32 {
    let fract = |v: f32| v - v.floor();
    let mut q = [
        fract(p[0] * 0.1031),
        fract(p[1] * 0.1031),
        fract(p[0] * 0.1031),
    ];
    let dot = q[0] * (q[1] + 33.33) + q[1] * (q[2] + 33.33) + q[2] * (q[0] + 33.33);
    for v in &mut q {
        *v += dot;
    }
    fract((q[0] + q[1]) * q[2])
}

fn noise(p: [f32; 2]) -> f32 {
    let cell = p.map(f32::floor);
    let f = p.map(|v| {
        let f = v - v.floor();
        f * f * (3.0 - 2.0 * f)
    });
    mix(
        mix(hash(cell), hash([cell[0] + 1.0, cell[1]]), f[0]),
        mix(
            hash([cell[0], cell[1] + 1.0]),
            hash([cell[0] + 1.0, cell[1] + 1.0]),
            f[0],
        ),
        f[1],
    )
}

pub(super) fn dither(pixel: [f32; 2]) -> [f32; 3] {
    let screen_noise = |x: f32, y: f32| {
        let a = x * 0.067_110_56 + y * 0.005_837_15;
        let b = 52.982_918 * (a - a.floor());
        b - b.floor()
    };
    [
        (screen_noise(pixel[0], pixel[1]) - 0.5) / 255.0,
        (screen_noise(pixel[0] + 17.0, pixel[1] + 59.0) - 0.5) / 255.0,
        (screen_noise(pixel[0] + 43.0, pixel[1] + 11.0) - 0.5) / 255.0,
    ]
}

pub(super) fn shade(p: &[f32], uv: [f32; 2]) -> [f32; 4] {
    let unit = p[0].min(p[1] * 1.4);
    let x = (uv[0] - 0.5) * p[0] / unit;
    let y = (uv[1] - p[32]) * p[1] / unit;
    let altitude = 0.16 * x * x - y;
    let pixel = 1.0 / unit;
    let background = [p[4], p[5], p[6]];
    let above = smooth(-pixel, pixel * 1.5, altitude);
    let gradient = 0.88 - altitude.max(0.0) * 2.2 + (p[2] * 0.03).sin() * 0.002;
    let sky = sky_color(p, gradient);
    let burn_time = p[2] * 0.55;
    let coarse = noise([x * 22.0 - burn_time * 0.3, burn_time * 0.4]);
    let serration = noise([x * 100.0 - burn_time * 0.5, burn_time * 1.4]);
    let chatter = noise([x * 270.0 + burn_time * 0.8, burn_time * 3.7]);
    let warp = (coarse - 0.5) * 0.0012 + (chatter - 0.5) * 0.0006;
    let distance_to_rim = altitude + warp;
    let reach = 0.0010 + serration.powi(3) * 0.005;
    let flame = noise([x * 180.0 + coarse - 0.5, altitude * 180.0 - burn_time * 2.5]);
    let burn = (-distance_to_rim.abs() / reach).exp()
        * smooth(0.25, 0.85, flame)
        * (1.0 - smooth(0.004, 0.009, distance_to_rim))
        * smooth(-0.003, 0.001, distance_to_rim);
    let rim = (-distance_to_rim.abs() * 1000.0).exp()
        * (0.85 + chatter * 0.12)
        * (0.95 + serration * 0.10);
    let glow = (-distance_to_rim.abs() * 150.0).exp() * 0.20;
    let sun = (-x * x * 35.0).exp() * (-distance_to_rim.abs() * 140.0).exp() * 0.10;
    let blue = sky_color(p, 0.68);
    let dither = dither([(uv[0] * p[0]).floor(), ((1.0 - uv[1]) * p[1]).floor()]);
    let edge_pixel = (uv[0].min(1.0 - uv[0]) * p[0]).min(uv[1].min(1.0 - uv[1]) * p[1]);
    let edge = smooth(0.0, 0.13, uv[0])
        * smooth(0.0, 0.13, 1.0 - uv[0])
        * smooth(0.0, 0.12, uv[1])
        * smooth(0.0, 0.16, 1.0 - uv[1]);
    let mut color = [0.0; 3];
    for c in 0..3 {
        color[c] = mix(background[c], sky[c], above)
            + p[28 + c] * (rim * 0.70 + burn * 0.40 + glow * 0.40 + sun)
            + blue[c] * (glow * 0.40 + burn * 0.50);
        color[c] = mix(p[4 + c], color[c], edge);
        color[c] = (color[c] + dither[c] * smooth(0.0, 1.0, edge_pixel)).max(0.0);
    }
    let radius = p[31] * p[3];
    let corner = [
        (uv[0] - 0.5).abs() * p[0] - p[0] * 0.5 + radius,
        (uv[1] - 0.5).abs() * p[1] - p[1] * 0.5 + radius,
    ];
    let distance = (corner[0].max(0.0).powi(2) + corner[1].max(0.0).powi(2)).sqrt()
        + corner[0].max(corner[1]).min(0.0)
        - radius;
    let coverage = (0.5 - distance).clamp(0.0, 1.0);
    [
        color[0] * coverage,
        color[1] * coverage,
        color[2] * coverage,
        coverage,
    ]
}
