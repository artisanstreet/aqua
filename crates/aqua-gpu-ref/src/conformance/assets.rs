use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};

pub const ATLAS: TextureHandle = TextureHandle(1);
pub const PHOTO: TextureHandle = TextureHandle(2);

pub const ATLAS_EXTENT: Extent = Extent::new(64, 32);
pub const PHOTO_EXTENT: Extent = Extent::new(32, 32);

fn atlas_coverage(x: u32, y: u32) -> u8 {
    let (fx, fy) = (x as f32, y as f32);
    let cell = (x / 16, y / 16);
    let local = ((fx % 16.0) - 7.5, (fy % 16.0) - 7.5);
    let distance = (local.0 * local.0 + local.1 * local.1).sqrt();
    let value = match (cell.0 + cell.1 * 4) % 4 {
        0 => (1.0 - (distance - 5.0).abs() / 2.0).clamp(0.0, 1.0),
        1 => ((x % 16) as f32 / 15.0).clamp(0.0, 1.0),
        2 => (((x + y) % 2) as f32).clamp(0.0, 1.0),
        _ => ((y % 16) as f32 / 15.0).clamp(0.0, 1.0),
    };
    (value * 255.0).round() as u8
}

fn photo_texel(x: u32, y: u32) -> [u8; 4] {
    let alpha = 0.35 + 0.65 * (x as f32 / 31.0);
    let red = 0.9 * (1.0 - y as f32 / 31.0);
    let green = 0.8 * (y as f32 / 31.0);
    let blue = 0.3 + 0.5 * (((x / 4 + y / 4) % 2) as f32);
    [red, green, blue]
        .map(|c| c * alpha)
        .map(|c| (c * 255.0).round() as u8)
        .into_iter()
        .chain([(alpha * 255.0).round() as u8])
        .collect::<Vec<_>>()
        .try_into()
        .expect("four channels")
}

fn full(extent: Extent) -> TextureRegion {
    TextureRegion { x: 0, y: 0, extent }
}

pub fn standard_uploads() -> UploadBatch {
    let atlas: Vec<u8> = (0..ATLAS_EXTENT.height)
        .flat_map(|y| (0..ATLAS_EXTENT.width).map(move |x| atlas_coverage(x, y)))
        .collect();
    let photo: Vec<u8> = (0..PHOTO_EXTENT.height)
        .flat_map(|y| (0..PHOTO_EXTENT.width).flat_map(move |x| photo_texel(x, y)))
        .collect();
    UploadBatch {
        textures: vec![
            TextureUpload {
                handle: ATLAS,
                op: TextureOp::Create {
                    extent: ATLAS_EXTENT,
                    format: TextureFormat::R8Unorm,
                },
            },
            TextureUpload {
                handle: ATLAS,
                op: TextureOp::Write {
                    region: full(ATLAS_EXTENT),
                    data: atlas,
                },
            },
            TextureUpload {
                handle: PHOTO,
                op: TextureOp::Create {
                    extent: PHOTO_EXTENT,
                    format: TextureFormat::Rgba8Unorm,
                },
            },
            TextureUpload {
                handle: PHOTO,
                op: TextureOp::Write {
                    region: full(PHOTO_EXTENT),
                    data: photo,
                },
            },
        ],
    }
}
