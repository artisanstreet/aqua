use std::collections::HashMap;
use std::time::Instant;

use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, UploadBatch};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::formats::persistent;
use super::Error;
use crate::ledger::{record, Kind as LedgerKind, Token};
use crate::pool::{Pool, POOL_TTL};

pub struct Entry {
    pub texture: ID3D11Texture2D,
    pub srv: ID3D11ShaderResourceView,
    pub format: TextureFormat,
    pub extent: Extent,
    _token: Token,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentStats {
    pub textures: u32,
    pub resident_bytes: u64,
    pub staging_bytes: u64,
    pub uploaded_bytes: u64,
    pub pooled_textures: u32,
    pub pooled_bytes: u64,
    pub reused: u64,
}

pub struct PersistentTextures {
    device: ID3D11Device,
    entries: HashMap<TextureHandle, Entry>,
    pool: Pool<(Extent, TextureFormat), Entry>,
    uploaded_bytes: u64,
    reused: u64,
}

const SMALL_PAGE_BYTES: u64 = 512 * 1024;

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

impl PersistentTextures {
    pub fn new(device: &ID3D11Device) -> Self {
        Self {
            device: device.clone(),
            entries: HashMap::new(),
            pool: Pool::default(),
            uploaded_bytes: 0,
            reused: 0,
        }
    }

    pub fn get(&self, handle: TextureHandle) -> Option<&Entry> {
        self.entries.get(&handle)
    }

    pub fn format(&self, handle: TextureHandle) -> Option<TextureFormat> {
        self.entries.get(&handle).map(|entry| entry.format)
    }

    pub fn extent(&self, handle: TextureHandle) -> Option<Extent> {
        self.entries.get(&handle).map(|entry| entry.extent)
    }

    pub fn stats(&self) -> PersistentStats {
        PersistentStats {
            textures: self.entries.len() as u32,
            resident_bytes: self
                .entries
                .values()
                .map(|entry| texture_bytes(entry.extent, entry.format))
                .sum(),
            staging_bytes: 0,
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

    pub fn clear(&mut self) {
        self.entries.clear();
        self.pool.clear();
    }

    pub fn drain_pool(&mut self) -> usize {
        self.pool.clear()
    }

    pub fn expire_pool(&mut self, now: Instant) -> usize {
        self.pool.expire(now, POOL_TTL)
    }

    fn retire(&mut self, entry: Entry) {
        self.pool
            .put((entry.extent, entry.format), entry, Instant::now());
    }

    fn reuse(
        &mut self,
        context: &ID3D11DeviceContext1,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
    ) -> Option<Entry> {
        let in_place = self
            .entries
            .get(&handle)
            .is_some_and(|entry| entry.extent == extent && entry.format == format);
        let entry = match in_place {
            true => self.entries.remove(&handle)?,
            false => self.pool.take(&(extent, format))?,
        };
        let pitch = extent.width as u64 * format.bytes_per_texel();
        let zeros = vec![0u8; (pitch * extent.height as u64) as usize];
        unsafe {
            context.UpdateSubresource(
                &entry.texture,
                0,
                None,
                zeros.as_ptr() as *const _,
                pitch as u32,
                0,
            )
        };
        self.reused += 1;
        Some(entry)
    }

    fn create(
        &mut self,
        context: &ID3D11DeviceContext1,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
    ) -> Result<(), Error> {
        let dxgi =
            persistent(format).ok_or(Error::Texture("unsupported persistent texture format"))?;
        if extent.width == 0 || extent.height == 0 {
            return Err(Error::Texture("persistent texture has an empty extent"));
        }
        if let Some(entry) = self.reuse(context, handle, extent, format) {
            if let Some(replaced) = self.entries.insert(handle, entry) {
                self.retire(replaced);
            }
            return Ok(());
        }
        let desc = D3D11_TEXTURE2D_DESC {
            Width: extent.width,
            Height: extent.height,
            MipLevels: 1,
            ArraySize: 1,
            Format: dxgi,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Usage: D3D11_USAGE_DEFAULT,
            BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
            CPUAccessFlags: 0,
            MiscFlags: 0,
        };
        let mut texture = None;
        unsafe {
            self.device
                .CreateTexture2D(&desc, None, Some(&mut texture))?
        };
        let texture: ID3D11Texture2D = texture.ok_or(Error::NoAdapter)?;
        let mut srv = None;
        unsafe {
            self.device
                .CreateShaderResourceView(&texture, None, Some(&mut srv))?
        };
        let created = self.entries.insert(
            handle,
            Entry {
                texture,
                srv: srv.ok_or(Error::NoAdapter)?,
                format,
                extent,
                _token: record(
                    texture_label(extent, format),
                    LedgerKind::Texture,
                    texture_bytes(extent, format),
                ),
            },
        );
        if let Some(replaced) = created {
            self.retire(replaced);
        }
        Ok(())
    }

    pub fn apply(
        &mut self,
        context: &ID3D11DeviceContext1,
        batches: &[UploadBatch],
    ) -> Result<u64, Error> {
        let mut written = 0u64;
        self.expire_pool(Instant::now());
        for upload in batches.iter().flat_map(|batch| &batch.textures) {
            match &upload.op {
                TextureOp::Create { extent, format } => {
                    self.create(context, upload.handle, *extent, *format)?
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
                    let target = D3D11_BOX {
                        left: region.x,
                        top: region.y,
                        front: 0,
                        right: region.x + extent.width,
                        bottom: region.y + extent.height,
                        back: 1,
                    };
                    unsafe {
                        context.UpdateSubresource(
                            &entry.texture,
                            0,
                            Some(&target),
                            data.as_ptr() as *const _,
                            (extent.width as u64 * bytes) as u32,
                            0,
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
