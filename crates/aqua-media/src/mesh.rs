#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MeshError {
    Invalid(String),
}

impl std::fmt::Display for MeshError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MeshError::Invalid(reason) => write!(f, "invalid mesh: {reason}"),
        }
    }
}

impl std::error::Error for MeshError {}

pub const MAX_MESH_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Debug, Default)]
pub struct MeshData {
    pub positions: Vec<[f32; 3]>,
    pub normals: Option<Vec<[f32; 3]>>,
    pub uvs: Option<Vec<[f32; 2]>>,
    pub colors: Option<Vec<[f32; 4]>>,
    pub indices: Vec<u32>,
}

impl MeshData {
    pub fn from_positions_indices(positions: Vec<[f32; 3]>, indices: Vec<u32>) -> Self {
        Self {
            positions,
            indices,
            ..Default::default()
        }
    }

    pub fn with_colors(mut self, colors: Vec<[f32; 4]>) -> Self {
        self.colors = Some(colors);
        self
    }

    pub fn with_normals(mut self, normals: Vec<[f32; 3]>) -> Self {
        self.normals = Some(normals);
        self
    }

    pub fn with_uvs(mut self, uvs: Vec<[f32; 2]>) -> Self {
        self.uvs = Some(uvs);
        self
    }

    pub fn validate(&self) -> Result<(), MeshError> {
        if self.positions.is_empty() {
            return Err(MeshError::Invalid("no positions".into()));
        }
        let n = self.positions.len();
        if let Some(v) = &self.normals {
            if v.len() != n {
                return Err(MeshError::Invalid("normals length".into()));
            }
        }
        if let Some(v) = &self.uvs {
            if v.len() != n {
                return Err(MeshError::Invalid("uv length".into()));
            }
        }
        if let Some(v) = &self.colors {
            if v.len() != n {
                return Err(MeshError::Invalid("colors length".into()));
            }
        }
        for i in &self.indices {
            if (*i as usize) >= n {
                return Err(MeshError::Invalid("index out of range".into()));
            }
        }
        for p in &self.positions {
            if !p.iter().all(|c| c.is_finite()) {
                return Err(MeshError::Invalid("non-finite position".into()));
            }
        }
        if self.byte_len() > MAX_MESH_BYTES {
            return Err(MeshError::Invalid("mesh exceeds byte cap".into()));
        }
        Ok(())
    }

    pub fn byte_len(&self) -> usize {
        let mut bytes = self.positions.len() * 12 + self.indices.len() * 4;
        if let Some(v) = &self.normals {
            bytes += v.len() * 12;
        }
        if let Some(v) = &self.uvs {
            bytes += v.len() * 8;
        }
        if let Some(v) = &self.colors {
            bytes += v.len() * 16;
        }
        bytes
    }

    pub fn triangle_count(&self) -> usize {
        if self.indices.is_empty() {
            self.positions.len() / 3
        } else {
            self.indices.len() / 3
        }
    }

    pub fn aabb(&self) -> Option<([f32; 3], [f32; 3])> {
        if self.positions.is_empty() {
            return None;
        }
        let mut min = [f32::INFINITY; 3];
        let mut max = [f32::NEG_INFINITY; 3];
        for p in &self.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if min.iter().any(|c| !c.is_finite()) {
            return None;
        }
        Some((min, max))
    }

    pub fn quad(z: f32, color: [f32; 4]) -> Self {
        Self {
            positions: vec![
                [-1.0, -1.0, z],
                [1.0, -1.0, z],
                [1.0, 1.0, z],
                [-1.0, 1.0, z],
            ],
            colors: Some(vec![color; 4]),
            indices: vec![0, 1, 2, 0, 2, 3],
            ..Default::default()
        }
    }

    pub fn cube() -> Self {
        let positions = vec![
            [-1.0, -1.0, -1.0],
            [1.0, -1.0, -1.0],
            [1.0, 1.0, -1.0],
            [-1.0, 1.0, -1.0],
            [-1.0, -1.0, 1.0],
            [1.0, -1.0, 1.0],
            [1.0, 1.0, 1.0],
            [-1.0, 1.0, 1.0],
        ];
        let colors: Vec<[f32; 4]> = positions
            .iter()
            .map(|p| {
                [
                    (p[0] + 1.0) * 0.5,
                    (p[1] + 1.0) * 0.5,
                    (p[2] + 1.0) * 0.5,
                    1.0,
                ]
            })
            .collect();
        Self {
            positions,
            colors: Some(colors),
            indices: vec![
                0, 1, 2, 0, 2, 3, // -z
                4, 6, 5, 4, 7, 6, // +z
                4, 5, 1, 4, 1, 0, // -y
                3, 2, 6, 3, 6, 7, // +y
                1, 5, 6, 1, 6, 2, // +x
                4, 0, 3, 4, 3, 7, // -x
            ],
            ..Default::default()
        }
    }

    pub fn grid(n: u32, y: f32, color: [f32; 4]) -> Self {
        let n = n.max(1);
        let mut positions = Vec::new();
        let mut colors = Vec::new();
        let mut indices = Vec::new();
        for j in 0..=n {
            for i in 0..=n {
                positions.push([
                    -1.0 + 2.0 * i as f32 / n as f32,
                    y,
                    -1.0 + 2.0 * j as f32 / n as f32,
                ]);
                colors.push(color);
            }
        }
        let row = n + 1;
        for j in 0..n {
            for i in 0..n {
                let a = j * row + i;
                indices.extend([a, a + 1, a + row + 1, a, a + row + 1, a + row]);
            }
        }
        Self {
            positions,
            colors: Some(colors),
            indices,
            ..Default::default()
        }
    }
}
