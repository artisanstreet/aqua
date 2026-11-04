use std::collections::HashMap;

use aqua_graph::{Extent, TextureFormat, TextureHandle};
use aqua_protocol::{TextureOp, UploadBatch};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;

use super::renderer::PersistentView;
use super::util::*;
use super::Error;

const PITCH_ALIGNMENT: u64 = D3D12_TEXTURE_DATA_PITCH_ALIGNMENT as u64;
const PLACEMENT_ALIGNMENT: u64 = D3D12_TEXTURE_DATA_PLACEMENT_ALIGNMENT as u64;
const MIN_STAGING_BYTES: u64 = 1 << 16;
const RETAINED_STAGING_BYTES: u64 = 1 << 20;

struct Entry {
    resource: ID3D12Resource,
    format: TextureFormat,
    extent: Extent,
    state: D3D12_RESOURCE_STATES,
}

struct Staging {
    resource: ID3D12Resource,
    ptr: *mut u8,
    capacity: u64,
    busy_until: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PersistentStats {
    pub textures: u32,
    pub resident_bytes: u64,
    pub staging_bytes: u64,
    pub uploaded_bytes: u64,
}

pub struct PersistentTextures {
    device: ID3D12Device,
    entries: HashMap<TextureHandle, Entry>,
    staging: Vec<Staging>,
    graveyard: Vec<(u64, ID3D12Resource)>,
    uploaded_bytes: u64,
}

pub fn dxgi_format(format: TextureFormat) -> Option<DXGI_FORMAT> {
    match format {
        TextureFormat::R8Unorm => Some(DXGI_FORMAT_R8_UNORM),
        TextureFormat::Rgba8Unorm => Some(DXGI_FORMAT_R8G8B8A8_UNORM),
        _ => None,
    }
}

fn texture_bytes(extent: Extent, format: TextureFormat) -> u64 {
    extent.texels() * format.bytes_per_texel()
}

fn staging_need(batches: &[UploadBatch], formats: &HashMap<TextureHandle, TextureFormat>) -> u64 {
    let mut formats = formats.clone();
    let mut total = 0;
    for batch in batches {
        for upload in &batch.textures {
            match &upload.op {
                TextureOp::Create { format, .. } => {
                    formats.insert(upload.handle, *format);
                }
                TextureOp::Write { region, .. } => {
                    let bytes = formats
                        .get(&upload.handle)
                        .map(|format| format.bytes_per_texel())
                        .unwrap_or(4);
                    let pitch = align_up(region.extent.width as u64 * bytes, PITCH_ALIGNMENT);
                    total = align_up(
                        total + pitch * region.extent.height as u64,
                        PLACEMENT_ALIGNMENT,
                    );
                }
                TextureOp::Release => {
                    formats.remove(&upload.handle);
                }
            }
        }
    }
    total
}

impl PersistentTextures {
    pub fn new(device: &ID3D12Device) -> Self {
        Self {
            device: device.clone(),
            entries: HashMap::new(),
            staging: Vec::new(),
            graveyard: Vec::new(),
            uploaded_bytes: 0,
        }
    }

    pub fn view(&self, handle: TextureHandle) -> Option<PersistentView> {
        self.entries.get(&handle).map(|entry| PersistentView {
            resource: entry.resource.clone(),
            format: dxgi_format(entry.format).unwrap_or(DXGI_FORMAT_UNKNOWN),
        })
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
            staging_bytes: self.staging.iter().map(|staging| staging.capacity).sum(),
            uploaded_bytes: self.uploaded_bytes,
        }
    }

    pub fn retire(&mut self, completed: u64) {
        self.graveyard.retain(|(fence, _)| *fence > completed);
        self.staging.retain(|staging| {
            staging.busy_until > completed || staging.capacity <= RETAINED_STAGING_BYTES
        });
    }

    pub fn trim_staging(&mut self, completed: u64) {
        self.staging
            .retain(|staging| staging.busy_until > completed);
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.staging.clear();
        self.graveyard.clear();
    }

    fn create(
        &mut self,
        handle: TextureHandle,
        extent: Extent,
        format: TextureFormat,
        frame_fence: u64,
    ) -> Result<(), Error> {
        let dxgi =
            dxgi_format(format).ok_or(Error::Texture("unsupported persistent texture format"))?;
        if extent.width == 0 || extent.height == 0 {
            return Err(Error::Texture("persistent texture has an empty extent"));
        }
        let mut resource: Option<ID3D12Resource> = None;
        unsafe {
            self.device.CreateCommittedResource(
                &heap_properties(D3D12_HEAP_TYPE_DEFAULT),
                D3D12_HEAP_FLAG_NONE,
                &texture_desc(extent.width, extent.height, dxgi, D3D12_RESOURCE_FLAG_NONE),
                D3D12_RESOURCE_STATE_COMMON,
                None,
                &mut resource,
            )?
        };
        let resource = resource.ok_or(Error::NoAdapter)?;
        if let Some(previous) = self.entries.insert(
            handle,
            Entry {
                resource,
                format,
                extent,
                state: D3D12_RESOURCE_STATE_COMMON,
            },
        ) {
            self.graveyard.push((frame_fence, previous.resource));
        }
        Ok(())
    }

    fn staging_for(&mut self, needed: u64, completed: u64) -> Result<usize, Error> {
        let found = self
            .staging
            .iter()
            .position(|staging| staging.busy_until <= completed && staging.capacity >= needed);
        match found {
            Some(index) => Ok(index),
            None => {
                let capacity = needed.max(MIN_STAGING_BYTES).next_power_of_two();
                let resource = create_buffer(
                    &self.device,
                    D3D12_HEAP_TYPE_UPLOAD,
                    capacity,
                    D3D12_RESOURCE_STATE_GENERIC_READ,
                )?;
                let ptr = map_whole(&resource)?;
                self.staging.retain(|staging| {
                    staging.busy_until > completed || staging.capacity >= capacity
                });
                self.staging.push(Staging {
                    resource,
                    ptr,
                    capacity,
                    busy_until: 0,
                });
                Ok(self.staging.len() - 1)
            }
        }
    }

    pub fn record(
        &mut self,
        list: &ID3D12GraphicsCommandList,
        batches: &[UploadBatch],
        frame_fence: u64,
        completed: u64,
    ) -> Result<u64, Error> {
        if batches.iter().all(|batch| batch.textures.is_empty()) {
            return Ok(0);
        }
        let formats: HashMap<TextureHandle, TextureFormat> = self
            .entries
            .iter()
            .map(|(handle, entry)| (*handle, entry.format))
            .collect();
        let needed = staging_need(batches, &formats);
        let staging_index = self.staging_for(needed.max(1), completed)?;
        let (staging_resource, staging_ptr) = {
            let staging = &mut self.staging[staging_index];
            staging.busy_until = frame_fence;
            (staging.resource.clone(), staging.ptr)
        };
        let mut cursor = 0u64;
        let mut copying: Vec<TextureHandle> = Vec::new();
        let mut written = 0u64;
        for batch in batches {
            for upload in &batch.textures {
                match &upload.op {
                    TextureOp::Create { extent, format } => {
                        self.create(upload.handle, *extent, *format, frame_fence)?
                    }
                    TextureOp::Release => {
                        copying.retain(|handle| *handle != upload.handle);
                        if let Some(entry) = self.entries.remove(&upload.handle) {
                            self.graveyard.push((frame_fence, entry.resource));
                        }
                    }
                    TextureOp::Write { region, data } => {
                        let entry = self
                            .entries
                            .get_mut(&upload.handle)
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
                        if entry.state != D3D12_RESOURCE_STATE_COPY_DEST {
                            unsafe {
                                list.ResourceBarrier(&[transition(
                                    &entry.resource,
                                    entry.state,
                                    D3D12_RESOURCE_STATE_COPY_DEST,
                                )])
                            };
                            entry.state = D3D12_RESOURCE_STATE_COPY_DEST;
                            copying.push(upload.handle);
                        }
                        let row_bytes = extent.width as u64 * bytes;
                        let pitch = align_up(row_bytes, PITCH_ALIGNMENT);
                        for row in 0..extent.height as u64 {
                            unsafe {
                                std::ptr::copy_nonoverlapping(
                                    data.as_ptr().add((row * row_bytes) as usize),
                                    staging_ptr.add((cursor + row * pitch) as usize),
                                    row_bytes as usize,
                                )
                            };
                        }
                        let footprint = D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                            Offset: cursor,
                            Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                Format: dxgi_format(entry.format).unwrap_or(DXGI_FORMAT_UNKNOWN),
                                Width: extent.width,
                                Height: extent.height,
                                Depth: 1,
                                RowPitch: pitch as u32,
                            },
                        };
                        let source = footprint_location(&staging_resource, footprint);
                        let destination = texture_location(&entry.resource);
                        unsafe {
                            list.CopyTextureRegion(
                                &destination,
                                region.x,
                                region.y,
                                0,
                                &source,
                                None,
                            )
                        };
                        cursor =
                            align_up(cursor + pitch * extent.height as u64, PLACEMENT_ALIGNMENT);
                        written += data.len() as u64;
                    }
                }
            }
        }
        let mut restore: Vec<D3D12_RESOURCE_BARRIER> = Vec::with_capacity(copying.len());
        for handle in &copying {
            if let Some(entry) = self.entries.get_mut(handle) {
                entry.state = D3D12_RESOURCE_STATE_COMMON;
                restore.push(transition(
                    &entry.resource,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    D3D12_RESOURCE_STATE_COMMON,
                ));
            }
        }
        if !restore.is_empty() {
            unsafe { list.ResourceBarrier(&restore) };
        }
        self.uploaded_bytes += written;
        Ok(written)
    }
}
