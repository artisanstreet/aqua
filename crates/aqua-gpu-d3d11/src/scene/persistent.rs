use std::collections::HashMap;

use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, UploadBatch};
use windows::Win32::Graphics::Direct3D11::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::Error;
use super::formats::persistent;
use crate::ledger::{Kind as LedgerKind, Token, record};

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
}

pub struct PersistentTextures {
    device: ID3D11Device,
    entries: HashMap<TextureHandle, Entry>,
    uploaded_bytes: u64,
}

fn texture_label(format: TextureFormat) -> &'static str {
    match format {
        TextureFormat::R8Unorm => "persistent_r8",
        TextureFormat::Rgba8Unorm => "persistent_rgba8",
        _ => "persistent_other",
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
            uploaded_bytes: 0,
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
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    fn create(
        &mut self,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
    ) -> Result<(), Error> {
        let dxgi =
            persistent(format).ok_or(Error::Texture("unsupported persistent texture format"))?;
        if extent.width == 0 || extent.height == 0 {
            return Err(Error::Texture("persistent texture has an empty extent"));
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
        self.entries.insert(
            handle,
            Entry {
                texture,
                srv: srv.ok_or(Error::NoAdapter)?,
                format,
                extent,
                _token: record(
                    texture_label(format),
                    LedgerKind::Texture,
                    texture_bytes(extent, format),
                ),
            },
        );
        Ok(())
    }

    pub fn apply(
        &mut self,
        context: &ID3D11DeviceContext1,
        batches: &[UploadBatch],
    ) -> Result<u64, Error> {
        let mut written = 0u64;
        for upload in batches.iter().flat_map(|batch| &batch.textures) {
            match &upload.op {
                TextureOp::Create { extent, format } => {
                    self.create(upload.handle, *extent, *format)?
                }
                TextureOp::Release => {
                    self.entries.remove(&upload.handle);
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
