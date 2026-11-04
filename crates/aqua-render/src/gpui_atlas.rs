use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::text::gpui::atlas::GlyphKey;

pub type SharedGpuiAtlas = Arc<Mutex<GpuiPolychromeAtlas>>;

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub(crate) enum AtlasKey {
    Image { asset: u64, frame: usize },
    Glyph(GlyphKey),
}

#[derive(Clone)]
pub(crate) struct AtlasPlacement {
    pub view: wgpu::TextureView,
    pub page_index: usize,
    pub tile_id: u32,
    pub page_size: [u32; 2],
    pub tile: [u32; 4],
    pub bearings: [i32; 2],
}

struct Page {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    size: [u32; 2],
    allocator: gpui_etagere::BucketedAtlasAllocator,
    live: usize,
}

struct Entry {
    page: usize,
    allocation: gpui_etagere::AllocId,
    tile: [u32; 4],
    bearings: [i32; 2],
}

#[derive(Default)]
pub struct GpuiPolychromeAtlas {
    pages: Vec<Option<Page>>,
    free: Vec<usize>,
    entries: HashMap<AtlasKey, Entry>,
}

impl GpuiPolychromeAtlas {
    pub(crate) fn get(&self, key: AtlasKey) -> Option<AtlasPlacement> {
        let entry = self.entries.get(&key)?;
        let page = self.pages.get(entry.page)?.as_ref()?;
        Some(AtlasPlacement {
            view: page.view.clone(),
            page_index: entry.page,
            tile_id: entry.allocation.serialize(),
            page_size: page.size,
            tile: entry.tile,
            bearings: entry.bearings,
        })
    }

    pub(crate) fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: AtlasKey,
        size: [u32; 2],
        bearings: [i32; 2],
        bgra: &[u8],
    ) -> Result<AtlasPlacement, String> {
        if let Some(placement) = self.get(key) {
            return Ok(placement);
        }
        let [width, height] = size;
        let maximum = device.limits().max_texture_dimension_2d.min(16384);
        if width == 0
            || height == 0
            || width > maximum
            || height > maximum
            || u64::from(width) * u64::from(height) * 4 != bgra.len() as u64
        {
            return Err("invalid GPUI polychrome atlas image".into());
        }
        let requested = gpui_etagere::Size::new(width as i32, height as i32);
        let mut allocation = None;
        for (index, page) in self.pages.iter_mut().enumerate().rev() {
            if let Some(page) = page {
                if let Some(slot) = page.allocator.allocate(requested) {
                    allocation = Some((index, slot));
                    break;
                }
            }
        }
        let (index, slot) = match allocation {
            Some(slot) => slot,
            None => {
                let live = self.pages.iter().flatten().take(2).count();
                let side = (256_u32 << live).min(maximum);
                let page_size = [side.max(width), side.max(height)];
                let texture = device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("gpui-polychrome-atlas"),
                    size: wgpu::Extent3d {
                        width: page_size[0],
                        height: page_size[1],
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Bgra8Unorm,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                    view_formats: &[],
                });
                let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
                let mut page = Page {
                    texture,
                    view,
                    size: page_size,
                    allocator: gpui_etagere::BucketedAtlasAllocator::new(gpui_etagere::Size::new(
                        page_size[0] as i32,
                        page_size[1] as i32,
                    )),
                    live: 0,
                };
                let slot = page
                    .allocator
                    .allocate(requested)
                    .ok_or("GPUI atlas allocation failed")?;
                let index = self.free.pop().unwrap_or(self.pages.len());
                if index == self.pages.len() {
                    self.pages.push(Some(page));
                } else {
                    self.pages[index] = Some(page);
                }
                (index, slot)
            }
        };
        let page = self.pages[index]
            .as_mut()
            .ok_or("GPUI atlas page missing")?;
        let [x, y] = [slot.rectangle.min.x as u32, slot.rectangle.min.y as u32];
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &page.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            bgra,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: Some(height),
            },
            wgpu::Extent3d {
                width,
                height,
                depth_or_array_layers: 1,
            },
        );
        page.live += 1;
        self.entries.insert(
            key,
            Entry {
                page: index,
                allocation: slot.id,
                tile: [x, y, width, height],
                bearings,
            },
        );
        self.get(key)
            .ok_or_else(|| "GPUI atlas placement missing".into())
    }

    pub(crate) fn remove(&mut self, key: AtlasKey) {
        let Some(entry) = self.entries.remove(&key) else {
            return;
        };
        let Some(page) = self.pages[entry.page].as_mut() else {
            return;
        };
        page.allocator.deallocate(entry.allocation);
        page.live -= 1;
        if page.live == 0 {
            self.pages[entry.page] = None;
            self.free.push(entry.page);
        }
    }

    pub(crate) fn remove_asset(&mut self, asset: u64) {
        let keys: Vec<_> = self
            .entries
            .keys()
            .copied()
            .filter(|key| matches!(key, AtlasKey::Image { asset: found, .. } if *found == asset))
            .collect();
        for key in keys {
            self.remove(key);
        }
    }
}
