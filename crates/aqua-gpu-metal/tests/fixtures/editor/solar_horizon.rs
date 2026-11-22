fn fract(value: f32) -> f32 {
    value - value.floor()
}

fn hash(p: [f32; 2]) -> f32 {
    let mut q = [
        fract(p[0] * 0.1031),
        fract(p[1] * 0.1031),
        fract(p[0] * 0.1031),
    ];
    let dot = q[0] * (q[1] + 33.33) + q[1] * (q[2] + 33.33) + q[2] * (q[0] + 33.33);
    q.iter_mut().for_each(|v| *v += dot);
    fract((q[0] + q[1]) * q[2])
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn smooth(low: f32, high: f32, value: f32) -> f32 {
    let t = ((value - low) / (high - low)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn noise(p: [f32; 2]) -> f32 {
    let cell = p.map(f32::floor);
    let f = p.map(|v| {
        let f = fract(v);
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

fn turbulence(p: [f32; 2]) -> f32 {
    noise(p) * 0.57
        + noise(p.map(|v| v * 2.03 + 7.1)) * 0.28
        + noise(p.map(|v| v * 4.11 + 19.7)) * 0.15
}

pub fn shade(p: &[f32], uv: [f32; 2]) -> [f32; 4] {
    let unit = p[0].min(p[1] * 1.4).max(1.0);
    let time = p[2] * 0.08;
    let burn_time = p[2] * 0.7;
    let x = (uv[0] - 0.5) * p[0] / unit;
    let y = (uv[1] - p[20]) * p[1] / unit;
    let altitude = 0.16 * x * x - y;
    let edge = smooth(0.0, 0.13, uv[0])
        * smooth(0.0, 0.13, 1.0 - uv[0])
        * smooth(0.0, 0.12, uv[1])
        * smooth(0.0, 0.16, 1.0 - uv[1]);
    let above = smooth(-0.002, 0.003, altitude);
    let coarse = turbulence([x * 20.0 + time * 0.25, time * 0.55]);
    let serration = turbulence([x * 75.0 - burn_time * 0.6, burn_time * 1.6]);
    let chatter = noise([x * 260.0 - burn_time * 2.0, burn_time * 5.5]);
    let warp = (coarse - 0.5) * 0.0015 + (chatter - 0.5) * 0.00065;
    let drift = (x * 12.0 - time * 0.9).sin() * 0.0005;
    let distance = altitude + warp + drift;
    let flame = turbulence([
        x * 130.0 + (coarse - 0.5) * 2.0,
        altitude * 260.0 - burn_time * 2.4,
    ]);
    let reach = 0.0012 + serration.powi(3) * 0.007;
    let fringe = (-distance.abs() / reach).exp() * (flame * 1.9 - 0.45).clamp(0.0, 1.0).powf(1.3);
    let tearing = ((serration - 0.35) * 2.0).clamp(0.0, 1.0);
    let burn_mask = (1.0 - smooth(0.004, 0.010, distance)) * smooth(-0.004, 0.001, distance);
    let plasma = fringe * (0.45 + tearing) * burn_mask;
    let surge = noise([x * 26.0 + burn_time * 0.35, burn_time * 0.8]);
    let pulse = 0.86 + surge * 0.22 + (burn_time * 1.7 + x * 8.0).sin() * 0.04;
    let current = turbulence([x * 7.0 - time * 1.3, altitude * 12.0 + time * 0.6]);
    let heat = noise([x * 12.0 - time * 1.4, time * 0.35]);
    let atmosphere_height = 0.13 + (current - 0.5) * 0.026;
    let blue = (-((altitude - atmosphere_height) / 0.14).powi(2)).exp()
        * above
        * 0.52
        * (0.92 + current * 0.16);
    let haze = (-altitude.max(0.0) * 38.0).exp() * above * 0.38 * (0.94 + heat * 0.12);
    let glow = (-distance.abs() * 110.0).exp()
        * (0.38 + serration * 0.04 + heat * 0.12)
        * (0.94 + surge * 0.12);
    let rim = (-distance.abs() * 1000.0).exp() * (0.82 + chatter * 0.14) * pulse;
    let filament = (-(distance - (flame - 0.5) * 0.0015).abs() * 1100.0).exp();
    let grain = (hash([(uv[0] * p[0]).floor(), (uv[1] * p[1]).floor()]) - 0.5) * 0.055;
    let red_note = smooth(0.38, 0.72, noise([x * 9.0 + 4.0 - time * 0.7, time * 0.2])) * 0.44;
    let mut result = [0.0, 0.0, 0.0, 1.0];
    for channel in 0..3 {
        let background = p[4 + channel];
        let ember = mix(p[12 + channel], p[24 + channel], red_note);
        let color = background
            + p[8 + channel] * blue
            + ember * (haze + glow + plasma * 0.62)
            + p[16 + channel] * (rim * 0.50 + filament * 0.08 + plasma * 0.06);
        let color = color + grain * (color - background).max(0.0).sqrt();
        result[channel] = mix(background, color.max(0.0), edge);
    }
    let radius = p[21] * p[3];
    let corner = [
        (uv[0] - 0.5).abs() * p[0] - p[0] * 0.5 + radius,
        (uv[1] - 0.5).abs() * p[1] - p[1] * 0.5 + radius,
    ];
    let distance = (corner[0].max(0.0).powi(2) + corner[1].max(0.0).powi(2)).sqrt()
        + corner[0].max(corner[1]).min(0.0)
        - radius;
    let coverage = (0.5 - distance).clamp(0.0, 1.0);
    result.map(|channel| channel * coverage)
}
