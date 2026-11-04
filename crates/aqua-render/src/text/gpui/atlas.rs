use std::collections::HashMap;

use aqua_text::raster::RasterKey;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct GlyphKey {
    pub cache: RasterKey,
    pub subpixel: bool,
    pub emoji: bool,
    pub scale_bits: u32,
    pub font_size_bits: u32,
    pub x_variant: u8,
    pub hint: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Tile {
    pub id: u32,
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
}

#[derive(Clone, Copy)]
pub(super) struct GlyphBitmap<'a> {
    pub width: u32,
    pub height: u32,
    pub left: i32,
    pub top: i32,
    pub rgba: &'a [u8],
}

pub(super) struct Atlas {
    texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    width: u32,
    height: u32,
    allocator: gpui_etagere::BucketedAtlasAllocator,
    tiles: HashMap<GlyphKey, Tile>,
}

impl Atlas {
    pub fn new(device: &wgpu::Device, width: u32, height: u32) -> Result<Self, String> {
        let max = device.limits().max_texture_dimension_2d;
        if width == 0 || height == 0 || width > max || height > max {
            return Err("invalid glyph atlas dimensions".into());
        }
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpui-glyph-atlas"),
            size: wgpu::Extent3d {
                width,
                height,
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
        Ok(Self {
            texture,
            view,
            width,
            height,
            allocator: gpui_etagere::BucketedAtlasAllocator::new(gpui_etagere::Size::new(
                width as i32,
                height as i32,
            )),
            tiles: HashMap::new(),
        })
    }

    pub fn size(&self) -> [u32; 2] {
        [self.width, self.height]
    }

    pub fn get(&self, key: &GlyphKey) -> Option<Tile> {
        self.tiles.get(key).copied()
    }

    pub fn insert(
        &mut self,
        queue: &wgpu::Queue,
        key: GlyphKey,
        bitmap: GlyphBitmap<'_>,
    ) -> Result<Tile, String> {
        let GlyphBitmap {
            width,
            height,
            left,
            top,
            rgba,
        } = bitmap;
        if let Some(tile) = self.get(&key) {
            return Ok(tile);
        }
        let count = u64::from(width) * u64::from(height) * 4;
        if width == 0 || height == 0 || count != rgba.len() as u64 {
            return Err("invalid glyph bitmap".into());
        }
        if width > self.width || height > self.height {
            return Err("glyph exceeds atlas dimensions".into());
        }
        let allocation = self
            .allocator
            .allocate(gpui_etagere::Size::new(width as i32, height as i32))
            .ok_or("glyph atlas full")?;
        let x = allocation.rectangle.min.x as u32;
        let y = allocation.rectangle.min.y as u32;
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &self.texture,
                mip_level: 0,
                origin: wgpu::Origin3d { x, y, z: 0 },
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
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
        let tile = Tile {
            id: allocation.id.serialize(),
            x,
            y,
            width,
            height,
            left,
            top,
        };
        self.tiles.insert(key, tile);
        Ok(tile)
    }
}

pub(super) struct AtlasPages {
    pages: Vec<Atlas>,
}

impl AtlasPages {
    pub fn new() -> Self {
        Self { pages: Vec::new() }
    }

    pub fn get(&self, key: &GlyphKey) -> Option<(usize, Tile)> {
        self.pages
            .iter()
            .enumerate()
            .find_map(|(index, page)| page.get(key).map(|tile| (index, tile)))
    }

    pub fn view(&self, index: usize) -> &wgpu::TextureView {
        &self.pages[index].view
    }

    pub fn size(&self, index: usize) -> [u32; 2] {
        self.pages[index].size()
    }

    pub fn insert(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        key: GlyphKey,
        bitmap: GlyphBitmap<'_>,
    ) -> Result<(usize, Tile), String> {
        for (index, page) in self.pages.iter().enumerate() {
            if let Some(tile) = page.get(&key) {
                return Ok((index, tile));
            }
        }
        for (index, page) in self.pages.iter_mut().enumerate().rev() {
            match page.insert(queue, key, GlyphBitmap { ..bitmap }) {
                Ok(tile) => return Ok((index, tile)),
                Err(error)
                    if error == "glyph atlas full" || error == "glyph exceeds atlas dimensions" => {
                }
                Err(error) => return Err(error),
            }
        }
        let maximum = device.limits().max_texture_dimension_2d.min(16384);
        let side = (256_u32 << self.pages.len().min(2)).min(maximum);
        let width = side.max(bitmap.width);
        let height = side.max(bitmap.height);
        let mut page = Atlas::new(device, width, height)?;
        let tile = page.insert(queue, key, bitmap)?;
        let index = self.pages.len();
        self.pages.push(page);
        Ok((index, tile))
    }
}
