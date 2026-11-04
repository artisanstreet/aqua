pub const WIDTH: u32 = 450;
pub const HEIGHT: u32 = 300;

const BACKGROUND: [u8; 4] = [10, 13, 23, 255];
const CAMERA_DISTANCE: f32 = 3.2;
const LIGHT: [f32; 3] = [-0.4, 0.7, -0.6];

const CORNERS: [[f32; 3]; 8] = [
    [-0.6, -0.6, -0.6],
    [0.6, -0.6, -0.6],
    [0.6, 0.6, -0.6],
    [-0.6, 0.6, -0.6],
    [-0.6, -0.6, 0.6],
    [0.6, -0.6, 0.6],
    [0.6, 0.6, 0.6],
    [-0.6, 0.6, 0.6],
];

struct Face {
    corners: [usize; 4],
    normal: [f32; 3],
    color: [f32; 3],
}

const FACES: [Face; 6] = [
    Face {
        corners: [0, 1, 2, 3],
        normal: [0.0, 0.0, -1.0],
        color: [0.9, 0.35, 0.3],
    },
    Face {
        corners: [5, 4, 7, 6],
        normal: [0.0, 0.0, 1.0],
        color: [0.3, 0.8, 0.45],
    },
    Face {
        corners: [4, 0, 3, 7],
        normal: [-1.0, 0.0, 0.0],
        color: [0.3, 0.55, 0.95],
    },
    Face {
        corners: [1, 5, 6, 2],
        normal: [1.0, 0.0, 0.0],
        color: [0.95, 0.8, 0.3],
    },
    Face {
        corners: [3, 2, 6, 7],
        normal: [0.0, 1.0, 0.0],
        color: [0.7, 0.4, 0.9],
    },
    Face {
        corners: [4, 5, 1, 0],
        normal: [0.0, -1.0, 0.0],
        color: [0.35, 0.85, 0.85],
    },
];

#[derive(Clone, Copy)]
struct Vertex {
    x: f32,
    y: f32,
    inverse_depth: f32,
}

fn rotate(point: [f32; 3], yaw: f32, pitch: f32) -> [f32; 3] {
    let (sin_yaw, cos_yaw) = yaw.sin_cos();
    let (sin_pitch, cos_pitch) = pitch.sin_cos();
    let x = point[0] * cos_yaw + point[2] * sin_yaw;
    let z = -point[0] * sin_yaw + point[2] * cos_yaw;
    [
        x,
        point[1] * cos_pitch - z * sin_pitch,
        point[1] * sin_pitch + z * cos_pitch,
    ]
}

fn project(point: [f32; 3]) -> Vertex {
    let depth = point[2] + CAMERA_DISTANCE;
    let focal = 1.0 / 30f32.to_radians().tan();
    let half_height = HEIGHT as f32 / 2.0;
    Vertex {
        x: WIDTH as f32 / 2.0 + point[0] / depth * focal * half_height,
        y: half_height - point[1] / depth * focal * half_height,
        inverse_depth: 1.0 / depth,
    }
}

fn edge(a: Vertex, b: Vertex, x: f32, y: f32) -> f32 {
    (x - a.x) * (b.y - a.y) - (y - a.y) * (b.x - a.x)
}

fn shade(color: [f32; 3], normal: [f32; 3]) -> [u8; 4] {
    let length = LIGHT.iter().map(|c| c * c).sum::<f32>().sqrt();
    let lambert = normal
        .iter()
        .zip(LIGHT)
        .map(|(n, l)| n * l / length)
        .sum::<f32>()
        .max(0.0);
    let light = 0.35 + 0.65 * lambert;
    let channel = |c: f32| (c * light * 255.0).round().clamp(0.0, 255.0) as u8;
    [channel(color[0]), channel(color[1]), channel(color[2]), 255]
}

fn fill_triangle(pixels: &mut [u8], depth: &mut [f32], triangle: [Vertex; 3], color: [u8; 4]) {
    let [a, b, c] = triangle;
    let area = edge(a, b, c.x, c.y);
    if area.abs() < f32::EPSILON {
        return;
    }
    let min_x = a.x.min(b.x).min(c.x).floor().max(0.0) as usize;
    let max_x = (a.x.max(b.x).max(c.x).ceil() as usize).min(WIDTH as usize - 1);
    let min_y = a.y.min(b.y).min(c.y).floor().max(0.0) as usize;
    let max_y = (a.y.max(b.y).max(c.y).ceil() as usize).min(HEIGHT as usize - 1);
    (min_y..=max_y)
        .flat_map(|y| (min_x..=max_x).map(move |x| (x, y)))
        .for_each(|(x, y)| {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let w0 = edge(b, c, px, py) / area;
            let w1 = edge(c, a, px, py) / area;
            let w2 = edge(a, b, px, py) / area;
            if w0 < 0.0 || w1 < 0.0 || w2 < 0.0 {
                return;
            }
            let inverse = w0 * a.inverse_depth + w1 * b.inverse_depth + w2 * c.inverse_depth;
            let index = y * WIDTH as usize + x;
            if inverse > depth[index] {
                depth[index] = inverse;
                pixels[index * 4..index * 4 + 4].copy_from_slice(&color);
            }
        });
}

pub fn render(seconds: f32) -> Vec<u8> {
    let (yaw, pitch) = (seconds * 0.9, 0.55 + 0.25 * (seconds * 0.5).sin());
    let mut pixels = BACKGROUND.repeat((WIDTH * HEIGHT) as usize);
    let mut depth = vec![0.0f32; (WIDTH * HEIGHT) as usize];
    let vertices = CORNERS.map(|corner| project(rotate(corner, yaw, pitch)));
    FACES.iter().for_each(|face| {
        let normal = rotate(face.normal, yaw, pitch);
        let color = shade(face.color, normal);
        let [a, b, c, d] = face.corners.map(|index| vertices[index]);
        fill_triangle(&mut pixels, &mut depth, [a, b, c], color);
        fill_triangle(&mut pixels, &mut depth, [a, c, d], color);
    });
    pixels
}
