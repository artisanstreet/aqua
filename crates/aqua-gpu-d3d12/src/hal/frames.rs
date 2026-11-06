use std::time::Instant;

use windows::Win32::Graphics::Direct3D12::*;

use super::error::Error;
use super::ring::{RingError, RingLayout};
use super::timeline::Timeline;
use super::util::*;
use crate::ledger::{record, Kind, Token};

pub const INITIAL_UPLOAD_BYTES: u64 = 1 << 16;

pub struct UploadAlloc {
    pub resource: ID3D12Resource,
    pub offset: u64,
    pub ptr: *mut u8,
    pub gpu_address: u64,
}

struct Mapped {
    resource: ID3D12Resource,
    ptr: *mut u8,
    _token: Token,
}

pub struct UploadRing {
    device: ID3D12Device,
    current: Mapped,
    layout: RingLayout,
    graveyard: Vec<(u64, Mapped)>,
}

fn create_mapped(device: &ID3D12Device, capacity: u64) -> Result<Mapped, Error> {
    let resource = create_buffer(
        device,
        D3D12_HEAP_TYPE_UPLOAD,
        capacity,
        D3D12_RESOURCE_STATE_GENERIC_READ,
    )?;
    let ptr = map_whole(&resource)?;
    let _token = record("frame_upload_ring", Kind::UploadBuffer, capacity);
    Ok(Mapped {
        resource,
        ptr,
        _token,
    })
}

fn reserve(
    layout: &mut RingLayout,
    timeline: &Timeline,
    size: u64,
    alignment: u64,
) -> Result<Option<u64>, Error> {
    loop {
        match layout.allocate(size, alignment) {
            Ok(offset) => return Ok(Some(offset)),
            Err(RingError::TooLarge) => return Ok(None),
            Err(RingError::Full) => match layout.oldest_fence() {
                Some(fence) => {
                    timeline.wait(fence)?;
                    layout.retire(timeline.completed());
                }
                None => return Ok(None),
            },
        }
    }
}

impl UploadRing {
    pub fn new(device: &ID3D12Device, capacity: u64) -> Result<Self, Error> {
        Ok(Self {
            device: device.clone(),
            current: create_mapped(device, capacity)?,
            layout: RingLayout::new(capacity),
            graveyard: Vec::new(),
        })
    }

    pub fn capacity(&self) -> u64 {
        self.layout.capacity()
    }

    pub fn graveyard_len(&self) -> usize {
        self.graveyard.len()
    }

    pub fn allocate(
        &mut self,
        timeline: &Timeline,
        size: u64,
        alignment: u64,
    ) -> Result<UploadAlloc, Error> {
        let offset = match reserve(&mut self.layout, timeline, size, alignment)? {
            Some(offset) => offset,
            None => {
                let capacity =
                    (self.layout.capacity().max(size + alignment) * 2).next_power_of_two();
                self.replace(timeline.next_value(), capacity)?;
                reserve(&mut self.layout, timeline, size, alignment)?.ok_or(Error::NoAdapter)?
            }
        };
        Ok(UploadAlloc {
            resource: self.current.resource.clone(),
            offset,
            ptr: unsafe { self.current.ptr.add(offset as usize) },
            gpu_address: unsafe { self.current.resource.GetGPUVirtualAddress() } + offset,
        })
    }

    fn replace(&mut self, retire_at: u64, capacity: u64) -> Result<(), Error> {
        let fresh = create_mapped(&self.device, capacity)?;
        let old = std::mem::replace(&mut self.current, fresh);
        self.graveyard.push((retire_at, old));
        self.layout = RingLayout::new(capacity);
        Ok(())
    }

    pub fn end_frame(&mut self, fence: u64) {
        self.layout.mark(fence);
    }

    pub fn retire(&mut self, completed: u64) {
        self.layout.retire(completed);
        self.graveyard.retain(|(fence, _)| *fence > completed);
    }

    pub fn shrink_to_initial(&mut self, timeline: &Timeline) -> Result<(), Error> {
        self.retire(timeline.completed());
        match self.layout.is_idle() && self.layout.capacity() > INITIAL_UPLOAD_BYTES {
            true => {
                self.replace(timeline.completed(), INITIAL_UPLOAD_BYTES)?;
                self.retire(timeline.completed());
                Ok(())
            }
            false => Ok(()),
        }
    }
}

struct Slot {
    allocator: ID3D12CommandAllocator,
    fence: u64,
    _token: Token,
}

pub struct FrameRing {
    slots: Vec<Slot>,
    next: usize,
    pub upload: UploadRing,
}

pub struct CoreFrame {
    pub index: usize,
    pub allocator: ID3D12CommandAllocator,
    pub waited_us: u32,
}

impl FrameRing {
    pub fn new(device: &ID3D12Device, frames: usize) -> Result<Self, Error> {
        let slots = (0..frames)
            .map(|_| {
                Ok(Slot {
                    allocator: unsafe {
                        device.CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)?
                    },
                    fence: 0,
                    _token: record("command_allocator", Kind::CommandAllocator, 0),
                })
            })
            .collect::<Result<Vec<_>, Error>>()?;
        Ok(Self {
            slots,
            next: 0,
            upload: UploadRing::new(device, INITIAL_UPLOAD_BYTES)?,
        })
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn first_allocator(&self) -> &ID3D12CommandAllocator {
        &self.slots[0].allocator
    }

    pub fn begin(&mut self, timeline: &Timeline) -> Result<CoreFrame, Error> {
        let index = self.next;
        self.next = (self.next + 1) % self.slots.len();
        let started = Instant::now();
        timeline.wait(self.slots[index].fence)?;
        let waited_us = started.elapsed().as_micros() as u32;
        unsafe { self.slots[index].allocator.Reset()? };
        self.upload.retire(timeline.completed());
        Ok(CoreFrame {
            index,
            allocator: self.slots[index].allocator.clone(),
            waited_us,
        })
    }

    pub fn end(&mut self, index: usize, fence: u64) {
        self.slots[index].fence = fence;
        self.upload.end_frame(fence);
    }
}
