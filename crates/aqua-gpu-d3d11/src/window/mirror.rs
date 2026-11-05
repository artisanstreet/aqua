use std::collections::HashMap;

use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, TextureRegion, TextureUpload, UploadBatch};

struct Image {
    extent: Extent,
    format: TextureFormat,
    bytes: Vec<u8>,
}

#[derive(Default)]
pub struct TextureMirror {
    images: HashMap<TextureHandle, Image>,
}

impl TextureMirror {
    pub fn apply(&mut self, batch: &UploadBatch) {
        for upload in &batch.textures {
            match &upload.op {
                TextureOp::Create { extent, format } => {
                    let size = (extent.texels() * format.bytes_per_texel()) as usize;
                    self.images.insert(
                        upload.handle,
                        Image {
                            extent: *extent,
                            format: *format,
                            bytes: vec![0; size],
                        },
                    );
                }
                TextureOp::Write { region, data } => {
                    if let Some(image) = self.images.get_mut(&upload.handle) {
                        write_region(image, region, data);
                    }
                }
                TextureOp::Release => {
                    self.images.remove(&upload.handle);
                }
            }
        }
    }

    pub fn replay(&self) -> UploadBatch {
        let mut handles: Vec<&TextureHandle> = self.images.keys().collect();
        handles.sort_by_key(|handle| handle.0);
        let textures = handles
            .into_iter()
            .flat_map(|handle| {
                let image = &self.images[handle];
                [
                    TextureUpload {
                        handle: *handle,
                        op: TextureOp::Create {
                            extent: image.extent,
                            format: image.format,
                        },
                    },
                    TextureUpload {
                        handle: *handle,
                        op: TextureOp::Write {
                            region: TextureRegion {
                                x: 0,
                                y: 0,
                                extent: image.extent,
                            },
                            data: image.bytes.clone(),
                        },
                    },
                ]
            })
            .collect();
        UploadBatch { textures }
    }

    pub fn textures(&self) -> usize {
        self.images.len()
    }

    pub fn resident_bytes(&self) -> u64 {
        self.images
            .values()
            .map(|image| image.bytes.len() as u64)
            .sum()
    }

    pub fn clear(&mut self) {
        self.images.clear();
    }
}

fn write_region(image: &mut Image, region: &TextureRegion, data: &[u8]) {
    let texel = image.format.bytes_per_texel() as usize;
    let row_bytes = region.extent.width as usize * texel;
    let image_row_bytes = image.extent.width as usize * texel;
    let fits = region.x as u64 + region.extent.width as u64 <= image.extent.width as u64
        && region.y as u64 + region.extent.height as u64 <= image.extent.height as u64
        && data.len() == row_bytes * region.extent.height as usize;
    if fits {
        for row in 0..region.extent.height as usize {
            let target = (region.y as usize + row) * image_row_bytes + region.x as usize * texel;
            image.bytes[target..target + row_bytes]
                .copy_from_slice(&data[row * row_bytes..(row + 1) * row_bytes]);
        }
    }
}
