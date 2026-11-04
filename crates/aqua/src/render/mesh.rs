use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshStyle, GpuiMeshVertex};

pub const TRIANGLE_LIMIT: usize = 400_000;
const EDGE_HALF_WIDTH: f32 = 0.6;

type Vec3 = [f32; 3];

fn transform(matrix: &[[f32; 4]; 4], vector: [f32; 4]) -> [f32; 4] {
    std::array::from_fn(|row| {
        (0..4)
            .map(|column| matrix[column][row] * vector[column])
            .sum()
    })
}

fn dot(a: Vec3, b: Vec3) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: Vec3) -> Vec3 {
    let length = dot(v, v).sqrt();
    match length > 1e-10 {
        true => [v[0] / length, v[1] / length, v[2] / length],
        false => v,
    }
}

fn linear(channel: f32) -> f32 {
    match channel <= 0.04045 {
        true => channel / 12.92,
        false => ((channel + 0.055) / 1.055).powf(2.4),
    }
}

fn encoded(channel: f32) -> f32 {
    let value = channel.clamp(0.0, 1.0);
    match value <= 0.003_130_8 {
        true => value * 12.92,
        false => 1.055 * value.powf(1.0 / 2.4) - 0.055,
    }
}

struct Corner {
    screen: [f32; 2],
    depth: f32,
    inverse_w: f32,
    view_position: Vec3,
    view_normal: Vec3,
    albedo: Vec3,
}

fn corner(
    vertex: &GpuiMeshVertex,
    style: &GpuiMeshStyle,
    width: f32,
    height: f32,
) -> Option<Corner> {
    let position = transform(
        &style.view,
        [
            vertex.position[0],
            vertex.position[1],
            vertex.position[2],
            1.0,
        ],
    );
    let clip = transform(&style.projection, position);
    let normal = transform(
        &style.view,
        [
            f32::from(vertex.normal[0]) / 32767.0,
            f32::from(vertex.normal[1]) / 32767.0,
            f32::from(vertex.normal[2]) / 32767.0,
            0.0,
        ],
    );
    match clip[3] > 1e-6 {
        false => None,
        true => {
            let inverse_w = 1.0 / clip[3];
            let albedo = match vertex.color[3] > 127 {
                true => [
                    linear(f32::from(vertex.color[0]) / 255.0),
                    linear(f32::from(vertex.color[1]) / 255.0),
                    linear(f32::from(vertex.color[2]) / 255.0),
                ],
                false => style.base_color,
            };
            Some(Corner {
                screen: [
                    (clip[0] * inverse_w * 0.5 + 0.5) * width,
                    (1.0 - (clip[1] * inverse_w * 0.5 + 0.5)) * height,
                ],
                depth: clip[2] * inverse_w,
                inverse_w,
                view_position: [position[0], position[1], position[2]],
                view_normal: [normal[0], normal[1], normal[2]],
                albedo,
            })
        }
    }
}

fn shade(position: Vec3, normal: Vec3, albedo: Vec3) -> Vec3 {
    let key_color = [1.0, 0.96, 0.9];
    let eye = normalize([-position[0], -position[1], -position[2]]);
    let mut normal = match dot(normal, normal) < 1e-20 {
        true => eye,
        false => normalize(normal),
    };
    if dot(normal, eye) < 0.0 {
        normal = [-normal[0], -normal[1], -normal[2]];
    }
    let key_direction = normalize([-0.45, 0.65, 0.6]);
    let fill_direction = normalize([0.7, 0.05, 0.5]);
    let rim_direction = normalize([0.15, 0.45, -0.9]);
    let half = normalize([key_direction[0], key_direction[1], key_direction[2] + 1.0]);
    let sky = (normal[1] * 0.5 + 0.5).clamp(0.0, 1.0);
    let ambient: Vec3 = std::array::from_fn(|i| {
        let low = [0.12, 0.11, 0.1][i];
        let high = [0.3, 0.31, 0.34][i];
        low + (high - low) * sky
    });
    let key = dot(normal, key_direction).max(0.0);
    let fill = dot(normal, fill_direction).max(0.0);
    let grazing = 1.0 - dot(normal, eye).clamp(0.0, 1.0);
    let rim = dot(normal, rim_direction).max(0.0) * grazing;
    let specular = dot(normal, half).max(0.0).powf(48.0) * 0.22 * key.min(1.0).sqrt();
    std::array::from_fn(|i| {
        let light = ambient[i]
            + key_color[i] * key
            + [0.26, 0.29, 0.34][i] * fill
            + [0.32, 0.32, 0.34][i] * rim;
        albedo[i] * light + key_color[i] * specular
    })
}

fn edge(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (p[0] - a[0]) * (b[1] - a[1]) - (p[1] - a[1]) * (b[0] - a[0])
}

pub fn rasterize(mesh: &GpuiMesh, style: &GpuiMeshStyle, width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    let mut pixels = vec![0u8; w * h * 4];
    let mut depth = vec![f32::INFINITY; w * h];
    let (fw, fh) = (width as f32, height as f32);
    for triangle in mesh.vertices().as_chunks::<3>().0 {
        let corners: Option<Vec<Corner>> = triangle
            .iter()
            .map(|vertex| corner(vertex, style, fw, fh))
            .collect();
        let Some(corners) = corners else { continue };
        let (a, b, c) = (&corners[0], &corners[1], &corners[2]);
        let area = edge(a.screen, b.screen, c.screen);
        if area.abs() < 1e-8 {
            continue;
        }
        let min_x = a.screen[0]
            .min(b.screen[0])
            .min(c.screen[0])
            .floor()
            .max(0.0) as usize;
        let max_x = (a.screen[0].max(b.screen[0]).max(c.screen[0]).ceil().min(fw) as usize).min(w);
        let min_y = a.screen[1]
            .min(b.screen[1])
            .min(c.screen[1])
            .floor()
            .max(0.0) as usize;
        let max_y = (a.screen[1].max(b.screen[1]).max(c.screen[1]).ceil().min(fh) as usize).min(h);
        let gradient = |first: &Corner, second: &Corner| {
            (
                (second.screen[1] - first.screen[1]) / area,
                -(second.screen[0] - first.screen[0]) / area,
            )
        };
        let gradients = [gradient(b, c), gradient(c, a), gradient(a, b)];
        let widths: [f32; 3] =
            std::array::from_fn(|i| (gradients[i].0.abs() + gradients[i].1.abs()).max(1e-9));
        for y in min_y..max_y {
            for x in min_x..max_x {
                let p = [x as f32 + 0.5, y as f32 + 0.5];
                let l0 = edge(b.screen, c.screen, p) / area;
                let l1 = edge(c.screen, a.screen, p) / area;
                let l2 = edge(a.screen, b.screen, p) / area;
                if l0 < 0.0 || l1 < 0.0 || l2 < 0.0 {
                    continue;
                }
                let z = l0 * a.depth + l1 * b.depth + l2 * c.depth;
                let slot = y * w + x;
                if !(0.0..=1.0).contains(&z) || z >= depth[slot] {
                    continue;
                }
                depth[slot] = z;
                let weights = [l0 * a.inverse_w, l1 * b.inverse_w, l2 * c.inverse_w];
                let total = weights.iter().sum::<f32>().max(1e-12);
                let mix = |pick: &dyn Fn(&Corner) -> Vec3| -> Vec3 {
                    std::array::from_fn(|i| {
                        (weights[0] * pick(a)[i]
                            + weights[1] * pick(b)[i]
                            + weights[2] * pick(c)[i])
                            / total
                    })
                };
                let position = mix(&|corner| corner.view_position);
                let normal = mix(&|corner| corner.view_normal);
                let albedo = mix(&|corner| corner.albedo);
                let mut color = shade(position, normal, albedo);
                if style.edge_color[3] > 0.0 {
                    let distance = (l0 / widths[0]).min(l1 / widths[1]).min(l2 / widths[2]);
                    let coverage =
                        (EDGE_HALF_WIDTH + 0.5 - distance).clamp(0.0, 1.0) * style.edge_color[3];
                    color = std::array::from_fn(|i| {
                        color[i] + (style.edge_color[i] - color[i]) * coverage
                    });
                }
                let at = slot * 4;
                pixels[at] = (encoded(color[0]) * 255.0).round() as u8;
                pixels[at + 1] = (encoded(color[1]) * 255.0).round() as u8;
                pixels[at + 2] = (encoded(color[2]) * 255.0).round() as u8;
                pixels[at + 3] = 255;
            }
        }
    }
    pixels
}

#[must_use]
pub fn style_bits(style: &GpuiMeshStyle) -> u64 {
    let mut state = 0xcbf2_9ce4_8422_2325u64;
    let mut feed = |value: f32| {
        state = (state ^ u64::from(value.to_bits())).wrapping_mul(0x0000_0100_0000_01b3);
    };
    style.view.iter().flatten().for_each(|v| feed(*v));
    style.projection.iter().flatten().for_each(|v| feed(*v));
    style.base_color.iter().for_each(|v| feed(*v));
    style.edge_color.iter().for_each(|v| feed(*v));
    state
}
