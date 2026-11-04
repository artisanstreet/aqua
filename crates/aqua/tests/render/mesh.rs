use aqua::render::mesh::rasterize;
use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshStyle, GpuiMeshVertex};

fn identity() -> [[f32; 4]; 4] {
    [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ]
}

fn style(edge: f32) -> GpuiMeshStyle {
    let mut projection = identity();
    projection[2][2] = -0.5;
    projection[3][2] = 0.5;
    GpuiMeshStyle {
        view: identity(),
        projection,
        base_color: [0.5, 0.5, 0.5],
        edge_color: [0.0, 0.0, 0.0, edge],
    }
}

fn quad(depth: f32, color: [f32; 3]) -> Vec<GpuiMeshVertex> {
    let corner = |x: f32, y: f32| GpuiMeshVertex::new([x, y, depth], [0.0, 0.0, 1.0], Some(color));
    vec![
        corner(-0.5, -0.5),
        corner(0.5, -0.5),
        corner(0.5, 0.5),
        corner(-0.5, -0.5),
        corner(0.5, 0.5),
        corner(-0.5, 0.5),
    ]
}

fn pixel(pixels: &[u8], width: usize, x: usize, y: usize) -> [u8; 4] {
    let at = (y * width + x) * 4;
    [pixels[at], pixels[at + 1], pixels[at + 2], pixels[at + 3]]
}

#[test]
fn a_quad_covers_the_middle_of_the_image_only() {
    let mesh = GpuiMesh::new(quad(0.0, [1.0, 0.0, 0.0]));
    let pixels = rasterize(&mesh, &style(0.0), 32, 32);
    assert_eq!(pixel(&pixels, 32, 16, 16)[3], 255);
    assert_eq!(pixel(&pixels, 32, 1, 1)[3], 0);
    assert_eq!(pixel(&pixels, 32, 30, 16)[3], 0);
}

#[test]
fn nearer_surfaces_hide_farther_ones_regardless_of_draw_order() {
    let mut vertices = quad(0.2, [0.0, 1.0, 0.0]);
    vertices.extend(quad(-0.2, [1.0, 0.0, 0.0]));
    let near_last = GpuiMesh::new(vertices.clone());
    vertices.rotate_left(6);
    let near_first = GpuiMesh::new(vertices);
    let a = rasterize(&near_last, &style(0.0), 24, 24);
    let b = rasterize(&near_first, &style(0.0), 24, 24);
    assert_eq!(pixel(&a, 24, 12, 12), pixel(&b, 24, 12, 12));
}

#[test]
fn edge_color_darkens_the_triangle_borders() {
    let mesh = GpuiMesh::new(quad(0.0, [1.0, 1.0, 1.0]));
    let plain = rasterize(&mesh, &style(0.0), 32, 32);
    let outlined = rasterize(&mesh, &style(1.0), 32, 32);
    let edge = 8;
    assert!(pixel(&outlined, 32, edge, 16)[0] < pixel(&plain, 32, edge, 16)[0]);
    assert_eq!(pixel(&outlined, 32, 16, 20), pixel(&plain, 32, 16, 20));
}
