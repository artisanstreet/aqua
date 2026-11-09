use aqua_core::Transform2D;
use aqua_core::vector::Brush;

#[derive(Clone, Debug, PartialEq)]
pub struct CurvePaint {
    pub brush: Brush,
    pub from_em: Transform2D,
}

impl CurvePaint {
    pub fn texels(&self) -> Vec<[f32; 4]> {
        let matrix = self.from_em;
        let (kind, stops, axis, radial) = match &self.brush {
            Brush::Solid(color) => (1.0, vec![(0.0, *color)], [0.0; 4], [0.0; 4]),
            Brush::Linear { start, end, stops } => (
                2.0,
                stops.iter().map(|s| (s.position, s.color)).collect(),
                [start.x, start.y, end.x, end.y],
                [0.0; 4],
            ),
            Brush::Radial {
                center,
                focal,
                radius,
                stops,
            } => (
                3.0,
                stops.iter().map(|s| (s.position, s.color)).collect(),
                [center.x, center.y, focal.x, focal.y],
                [*radius, 0.0, 0.0, 0.0],
            ),
        };
        let mut texels = vec![
            [kind, stops.len() as f32, 0.0, 0.0],
            [matrix.a, matrix.b, matrix.c, matrix.d],
            [matrix.tx, matrix.ty, 0.0, 0.0],
            axis,
            radial,
        ];
        for (offset, color) in stops {
            texels.push([offset, 0.0, 0.0, 0.0]);
            texels.push(color.to_srgba().map(|channel| channel.clamp(0.0, 1.0)));
        }
        texels
    }

    pub fn heap_bytes(&self) -> usize {
        match &self.brush {
            Brush::Solid(_) => 0,
            Brush::Linear { stops, .. } | Brush::Radial { stops, .. } => {
                stops.len() * size_of::<aqua_core::vector::GradientStop>()
            }
        }
    }
}

pub fn sample(fetch: impl Fn(u32) -> [f32; 4], paint: u32, em: [f32; 2]) -> [f32; 4] {
    let header = fetch(paint);
    if header[1] == 1.0 {
        let color = fetch(paint + 6);
        return [
            color[0] * color[3],
            color[1] * color[3],
            color[2] * color[3],
            color[3],
        ];
    }
    let matrix = fetch(paint + 1);
    let offset = fetch(paint + 2);
    let p = [
        matrix[0] * em[0] + matrix[2] * em[1] + offset[0],
        matrix[1] * em[0] + matrix[3] * em[1] + offset[1],
    ];
    let axis = fetch(paint + 3);
    let along = match header[0] as u32 {
        2 => {
            let delta = [axis[2] - axis[0], axis[3] - axis[1]];
            ((p[0] - axis[0]) * delta[0] + (p[1] - axis[1]) * delta[1])
                / (delta[0] * delta[0] + delta[1] * delta[1]).max(1.0e-20)
        }
        3 => {
            let radius = fetch(paint + 4)[0];
            let ray = [p[0] - axis[2], p[1] - axis[3]];
            let focal = [axis[2] - axis[0], axis[3] - axis[1]];
            let a = ray[0] * ray[0] + ray[1] * ray[1];
            let b = focal[0] * ray[0] + focal[1] * ray[1];
            let c = focal[0] * focal[0] + focal[1] * focal[1] - radius * radius;
            a / (-b + (b * b - a * c).max(0.0).sqrt()).max(1.0e-20)
        }
        _ => 0.0,
    };
    let count = header[1] as u32;
    let mut left = fetch(paint + 5)[0];
    let mut color = fetch(paint + 6);
    for index in 1..count {
        let right = fetch(paint + 5 + index * 2)[0];
        let next = fetch(paint + 6 + index * 2);
        if along < right {
            let t = ((along - left) / (right - left).max(1.0e-20)).clamp(0.0, 1.0);
            color = std::array::from_fn(|i| color[i] + (next[i] - color[i]) * t);
            break;
        }
        left = right;
        color = next;
    }
    [
        color[0] * color[3],
        color[1] * color[3],
        color[2] * color[3],
        color[3],
    ]
}
