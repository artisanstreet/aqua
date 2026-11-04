use std::mem::{transmute_copy, ManuallyDrop};

use windows::core::Interface;
use windows::Win32::Graphics::Direct3D12::*;

use super::util::{aliasing, transition};
use super::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BarrierMode {
    Queue,
    Enhanced,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Access {
    pub sync: D3D12_BARRIER_SYNC,
    pub access: D3D12_BARRIER_ACCESS,
    pub layout: D3D12_BARRIER_LAYOUT,
}

pub fn access_of(state: D3D12_RESOURCE_STATES) -> Access {
    match state {
        s if s == D3D12_RESOURCE_STATE_RENDER_TARGET => Access {
            sync: D3D12_BARRIER_SYNC_RENDER_TARGET,
            access: D3D12_BARRIER_ACCESS_RENDER_TARGET,
            layout: D3D12_BARRIER_LAYOUT_RENDER_TARGET,
        },
        s if s == D3D12_RESOURCE_STATE_PIXEL_SHADER_RESOURCE => Access {
            sync: D3D12_BARRIER_SYNC_PIXEL_SHADING,
            access: D3D12_BARRIER_ACCESS_SHADER_RESOURCE,
            layout: D3D12_BARRIER_LAYOUT_SHADER_RESOURCE,
        },
        s if s == D3D12_RESOURCE_STATE_COPY_SOURCE => Access {
            sync: D3D12_BARRIER_SYNC_COPY,
            access: D3D12_BARRIER_ACCESS_COPY_SOURCE,
            layout: D3D12_BARRIER_LAYOUT_COPY_SOURCE,
        },
        s if s == D3D12_RESOURCE_STATE_COPY_DEST => Access {
            sync: D3D12_BARRIER_SYNC_COPY,
            access: D3D12_BARRIER_ACCESS_COPY_DEST,
            layout: D3D12_BARRIER_LAYOUT_COPY_DEST,
        },
        s if s == D3D12_RESOURCE_STATE_UNORDERED_ACCESS => Access {
            sync: D3D12_BARRIER_SYNC_ALL_SHADING,
            access: D3D12_BARRIER_ACCESS_UNORDERED_ACCESS,
            layout: D3D12_BARRIER_LAYOUT_UNORDERED_ACCESS,
        },
        _ => Access {
            sync: D3D12_BARRIER_SYNC_NONE,
            access: D3D12_BARRIER_ACCESS_NO_ACCESS,
            layout: D3D12_BARRIER_LAYOUT_COMMON,
        },
    }
}

pub fn initial_layout(state: D3D12_RESOURCE_STATES) -> D3D12_BARRIER_LAYOUT {
    access_of(state).layout
}

fn texture_barrier(
    resource: &ID3D12Resource,
    before: Access,
    after: Access,
    flags: D3D12_TEXTURE_BARRIER_FLAGS,
) -> D3D12_TEXTURE_BARRIER {
    D3D12_TEXTURE_BARRIER {
        SyncBefore: before.sync,
        SyncAfter: after.sync,
        AccessBefore: before.access,
        AccessAfter: after.access,
        LayoutBefore: before.layout,
        LayoutAfter: after.layout,
        pResource: ManuallyDrop::new(Some(unsafe { transmute_copy(resource) })),
        Subresources: D3D12_BARRIER_SUBRESOURCE_RANGE {
            IndexOrFirstMipLevel: u32::MAX,
            NumMipLevels: 0,
            FirstArraySlice: 0,
            NumArraySlices: 0,
            FirstPlane: 0,
            NumPlanes: 0,
        },
        Flags: flags,
    }
}

pub fn texture_transition(
    resource: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_TEXTURE_BARRIER {
    texture_barrier(
        resource,
        access_of(before),
        access_of(after),
        D3D12_TEXTURE_BARRIER_FLAG_NONE,
    )
}

pub fn texture_activation(resource: &ID3D12Resource) -> D3D12_TEXTURE_BARRIER {
    texture_barrier(
        resource,
        Access {
            sync: D3D12_BARRIER_SYNC_ALL,
            access: D3D12_BARRIER_ACCESS_NO_ACCESS,
            layout: D3D12_BARRIER_LAYOUT_UNDEFINED,
        },
        access_of(D3D12_RESOURCE_STATE_RENDER_TARGET),
        D3D12_TEXTURE_BARRIER_FLAG_DISCARD,
    )
}

#[derive(Clone)]
pub struct Emitter {
    mode: BarrierMode,
    list7: Option<ID3D12GraphicsCommandList7>,
}

impl Emitter {
    pub fn new(sync: BarrierMode, list: &ID3D12GraphicsCommandList) -> Result<Self, Error> {
        let list7 = match sync {
            BarrierMode::Queue => None,
            BarrierMode::Enhanced => Some(
                list.cast::<ID3D12GraphicsCommandList7>()
                    .map_err(|_| Error::Unsupported("enhanced barriers need command list 7"))?,
            ),
        };
        Ok(Self { mode: sync, list7 })
    }

    pub fn batch(&self) -> Batch {
        Batch {
            mode: self.mode,
            legacy: Vec::new(),
            activations: Vec::new(),
            textures: Vec::new(),
            activated: Vec::new(),
        }
    }

    pub fn submit(&self, list: &ID3D12GraphicsCommandList, batch: Batch) -> u32 {
        let total = batch.len();
        match &self.list7 {
            None => {
                if !batch.legacy.is_empty() {
                    unsafe { list.ResourceBarrier(&batch.legacy) };
                }
            }
            Some(list7) => {
                if !batch.legacy.is_empty() {
                    unsafe { list.ResourceBarrier(&batch.legacy) };
                }
                for textures in [&batch.activations, &batch.textures] {
                    if textures.is_empty() {
                        continue;
                    }
                    let group = D3D12_BARRIER_GROUP {
                        Type: D3D12_BARRIER_TYPE_TEXTURE,
                        NumBarriers: textures.len() as u32,
                        Anonymous: D3D12_BARRIER_GROUP_0 {
                            pTextureBarriers: textures.as_ptr(),
                        },
                    };
                    unsafe { list7.Barrier(&[group]) };
                }
            }
        }
        total
    }
}

pub struct Batch {
    mode: BarrierMode,
    legacy: Vec<D3D12_RESOURCE_BARRIER>,
    activations: Vec<D3D12_TEXTURE_BARRIER>,
    textures: Vec<D3D12_TEXTURE_BARRIER>,
    activated: Vec<*mut std::ffi::c_void>,
}

impl Batch {
    pub fn len(&self) -> u32 {
        (self.legacy.len() + self.activations.len() + self.textures.len()) as u32
    }

    pub fn activate(&mut self, resource: &ID3D12Resource) -> Option<D3D12_RESOURCE_STATES> {
        match self.mode {
            BarrierMode::Queue => {
                self.legacy.push(aliasing(resource));
                None
            }
            BarrierMode::Enhanced => {
                let raw = resource.as_raw();
                if !self.activated.contains(&raw) {
                    self.activated.push(raw);
                    self.activations.push(texture_activation(resource));
                }
                Some(D3D12_RESOURCE_STATE_RENDER_TARGET)
            }
        }
    }

    pub fn transition(
        &mut self,
        resource: &ID3D12Resource,
        before: D3D12_RESOURCE_STATES,
        after: D3D12_RESOURCE_STATES,
    ) {
        match self.mode {
            BarrierMode::Queue => self.legacy.push(transition(resource, before, after)),
            BarrierMode::Enhanced => self
                .textures
                .push(texture_transition(resource, before, after)),
        }
    }

    pub fn transition_legacy(
        &mut self,
        resource: &ID3D12Resource,
        before: D3D12_RESOURCE_STATES,
        after: D3D12_RESOURCE_STATES,
    ) {
        self.legacy.push(transition(resource, before, after));
    }
}
