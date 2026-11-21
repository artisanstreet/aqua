use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, UploadBatch};
use ash::vk;

use super::formats::persistent;
use super::frames::Frame;
use super::Error;
use crate::hal::{Access, Device, Image, ImageDesc, COLOR};
use crate::ledger::Kind as LedgerKind;
use crate::pool::{Pool, POOL_TTL};

pub struct Entry {
    pub image: Image,
    pub format: TextureFormat,
    pub extent: Extent,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentStats {
    pub textures: u32,
    pub resident_bytes: u64,
    pub uploaded_bytes: u64,
    pub pooled_textures: u32,
    pub pooled_bytes: u64,
    pub reused: u64,
}

pub struct PersistentTextures {
    device: Arc<Device>,
    entries: HashMap<TextureHandle, Entry>,
    pool: Pool<(Extent, TextureFormat), Entry>,
    uploaded_bytes: u64,
    reused: u64,
}

const SMALL_PAGE_BYTES: u64 = 512 * 1024;
const STAGING_ALIGNMENT: u64 = 16;

fn texture_label(extent: Extent, format: TextureFormat) -> &'static str {
    let large = texture_bytes(extent, format) > SMALL_PAGE_BYTES;
    match (format, large) {
        (TextureFormat::R8Unorm, false) => "persistent_r8_small",
        (TextureFormat::R8Unorm, true) => "persistent_r8_page",
        (TextureFormat::Rgba8Unorm, false) => "persistent_rgba8_small",
        (TextureFormat::Rgba8Unorm, true) => "persistent_rgba8_page",
        (_, false) => "persistent_other_small",
        (_, true) => "persistent_other_page",
    }
}

fn texture_bytes(extent: Extent, format: TextureFormat) -> u64 {
    extent.texels() * format.bytes_per_texel()
}

pub fn staging_bytes(batches: &[UploadBatch]) -> u64 {
    batches
        .iter()
        .flat_map(|batch| &batch.textures)
        .map(|upload| match &upload.op {
            TextureOp::Write { data, .. } => data.len() as u64 + STAGING_ALIGNMENT,
            TextureOp::Create { .. } | TextureOp::Release => 0,
        })
        .sum()
}

fn clear(frame: &mut Frame<'_>, image: &Image) {
    let target = image.as_ref();
    let transitions: Vec<_> = target
        .transition(Access::TransferDestination)
        .into_iter()
        .collect();
    frame.barrier(&transitions);
    let zero = vk::ClearColorValue { float32: [0.0; 4] };
    unsafe {
        frame.device().raw.cmd_clear_color_image(
            frame.command(),
            target.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &zero,
            &[COLOR],
        )
    };
}

impl PersistentTextures {
    pub fn new(device: &Arc<Device>) -> Self {
        Self {
            device: Arc::clone(device),
            entries: HashMap::new(),
            pool: Pool::default(),
            uploaded_bytes: 0,
            reused: 0,
        }
    }

    pub fn get(&self, handle: TextureHandle) -> Option<&Entry> {
        self.entries.get(&handle)
    }

    pub fn stats(&self) -> PersistentStats {
        PersistentStats {
            textures: self.entries.len() as u32,
            resident_bytes: self
                .entries
                .values()
                .map(|entry| texture_bytes(entry.extent, entry.format))
                .sum(),
            uploaded_bytes: self.uploaded_bytes,
            pooled_textures: self.pool.len() as u32,
            pooled_bytes: self
                .pool
                .iter()
                .map(|(_, entry)| texture_bytes(entry.extent, entry.format))
                .sum(),
            reused: self.reused,
        }
    }

    pub fn drain_pool(&mut self) -> u64 {
        let bytes = self.stats().pooled_bytes;
        self.pool.clear();
        bytes
    }

    fn retire(&mut self, entry: Entry) {
        self.pool
            .put((entry.extent, entry.format), entry, Instant::now());
    }

    fn create(
        &mut self,
        frame: &mut Frame<'_>,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
    ) -> Result<(), Error> {
        let vulkan =
            persistent(format).ok_or(Error::Texture("unsupported persistent texture format"))?;
        if extent.width == 0 || extent.height == 0 {
            return Err(Error::Texture("persistent texture has an empty extent"));
        }
        let in_place = self
            .entries
            .get(&handle)
            .is_some_and(|entry| entry.extent == extent && entry.format == format);
        let reused = match in_place {
            true => self.entries.remove(&handle),
            false => self.pool.take(&(extent, format)),
        };
        let entry = match reused {
            Some(entry) => {
                self.reused += 1;
                entry
            }
            None => Entry {
                image: Image::new(
                    &self.device,
                    ImageDesc {
                        extent,
                        format: vulkan,
                        usage: vk::ImageUsageFlags::SAMPLED
                            | vk::ImageUsageFlags::TRANSFER_DST
                            | vk::ImageUsageFlags::TRANSFER_SRC,
                        label: texture_label(extent, format),
                        kind: LedgerKind::Texture,
                        bytes: texture_bytes(extent, format),
                    },
                )?,
                format,
                extent,
            },
        };
        clear(frame, &entry.image);
        if let Some(replaced) = self.entries.insert(handle, entry) {
            self.retire(replaced);
        }
        Ok(())
    }

    pub fn apply(&mut self, frame: &mut Frame<'_>, batches: &[UploadBatch]) -> Result<u64, Error> {
        let mut written = 0u64;
        self.pool.expire(Instant::now(), POOL_TTL);
        for upload in batches.iter().flat_map(|batch| &batch.textures) {
            match &upload.op {
                TextureOp::Create { extent, format } => {
                    self.create(frame, upload.handle, *extent, *format)?
                }
                TextureOp::Release => {
                    if let Some(entry) = self.entries.remove(&upload.handle) {
                        self.retire(entry);
                    }
                }
                TextureOp::Write { region, data } => {
                    let entry = self
                        .entries
                        .get(&upload.handle)
                        .ok_or(Error::Texture("write to an unknown texture"))?;
                    let bytes = entry.format.bytes_per_texel();
                    let extent = region.extent;
                    if region.x.saturating_add(extent.width) > entry.extent.width
                        || region.y.saturating_add(extent.height) > entry.extent.height
                    {
                        return Err(Error::Texture("write outside the texture"));
                    }
                    if data.len() as u64 != extent.texels() * bytes {
                        return Err(Error::Texture("write data does not match its region"));
                    }
                    if extent.width == 0 || extent.height == 0 {
                        continue;
                    }
                    let offset = frame.stream.push(STAGING_ALIGNMENT, data);
                    let target = entry.image.as_ref();
                    let transitions: Vec<_> = target
                        .transition(Access::TransferDestination)
                        .into_iter()
                        .collect();
                    frame.barrier(&transitions);
                    let copy = vk::BufferImageCopy::default()
                        .buffer_offset(offset)
                        .image_subresource(vk::ImageSubresourceLayers {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            mip_level: 0,
                            base_array_layer: 0,
                            layer_count: 1,
                        })
                        .image_offset(vk::Offset3D {
                            x: region.x as i32,
                            y: region.y as i32,
                            z: 0,
                        })
                        .image_extent(vk::Extent3D {
                            width: extent.width,
                            height: extent.height,
                            depth: 1,
                        });
                    unsafe {
                        frame.device().raw.cmd_copy_buffer_to_image(
                            frame.command(),
                            frame.ring(),
                            target.image,
                            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                            &[copy],
                        )
                    };
                    written += data.len() as u64;
                }
            }
        }
        self.uploaded_bytes += written;
        Ok(written)
    }
}
