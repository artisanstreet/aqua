use std::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct GpuiMeshVertex {
    pub position: [f32; 3],
    pub normal: [i16; 4],
    pub color: [u8; 4],
}

impl GpuiMeshVertex {
    pub fn new(position: [f32; 3], normal: [f32; 3], color: Option<[f32; 3]>) -> Self {
        let normal = normal.map(|value| if value.is_finite() { value } else { 0.0 });
        let pack = |value: f32| (value.clamp(-1.0, 1.0) * 32767.0).round() as i16;
        let color = color.map_or([0; 4], |rgb| {
            let [r, g, b] = rgb.map(|channel| {
                let channel = if channel.is_finite() {
                    channel.clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let encoded = if channel <= 0.003_130_8 {
                    channel * 12.92
                } else {
                    1.055 * channel.powf(1.0 / 2.4) - 0.055
                };
                (encoded * 255.0).round() as u8
            });
            [r, g, b, 255]
        });
        Self {
            position,
            normal: [pack(normal[0]), pack(normal[1]), pack(normal[2]), 0],
            color,
        }
    }
}

#[derive(Debug)]
pub struct GpuiMesh {
    id: u64,
    vertices: Vec<GpuiMeshVertex>,
}

impl PartialEq for GpuiMesh {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for GpuiMesh {}

impl GpuiMesh {
    pub fn new(mut vertices: Vec<GpuiMeshVertex>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        vertices.truncate(vertices.len() / 3 * 3);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            vertices,
        }
    }

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn vertices(&self) -> &[GpuiMeshVertex] {
        &self.vertices
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuiMeshStyle {
    pub view: [[f32; 4]; 4],
    pub projection: [[f32; 4]; 4],
    pub base_color: [f32; 3],
    pub edge_color: [f32; 4],
}
