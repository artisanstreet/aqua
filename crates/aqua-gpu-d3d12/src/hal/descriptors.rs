use windows::Win32::Graphics::Direct3D12::*;

use super::error::Error;
use super::ring::{RingError, RingLayout, SlotAllocator};
use super::timeline::Timeline;

#[derive(Clone, Copy, Debug)]
pub struct Descriptor {
    pub index: u32,
    pub cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
}

#[derive(Clone, Copy, Debug)]
pub struct VisibleDescriptor {
    pub index: u32,
    pub cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub gpu: D3D12_GPU_DESCRIPTOR_HANDLE,
}

pub struct CpuHeap {
    pub heap: ID3D12DescriptorHeap,
    stride: usize,
    start: usize,
    slots: SlotAllocator,
}

impl CpuHeap {
    pub fn new(
        device: &ID3D12Device,
        kind: D3D12_DESCRIPTOR_HEAP_TYPE,
        capacity: u32,
    ) -> Result<Self, Error> {
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: kind,
                NumDescriptors: capacity,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_NONE,
                NodeMask: 0,
            })?
        };
        let stride = unsafe { device.GetDescriptorHandleIncrementSize(kind) } as usize;
        let start = unsafe { heap.GetCPUDescriptorHandleForHeapStart() }.ptr;
        Ok(Self {
            heap,
            stride,
            start,
            slots: SlotAllocator::new(capacity),
        })
    }

    pub fn allocate(&mut self) -> Result<Descriptor, Error> {
        let index = self.slots.allocate().ok_or(Error::DescriptorsExhausted)?;
        Ok(Descriptor {
            index,
            cpu: D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.start + index as usize * self.stride,
            },
        })
    }

    pub fn release(&mut self, descriptor: Descriptor) {
        self.slots.release(descriptor.index);
    }

    pub fn live(&self) -> u32 {
        self.slots.live()
    }
}

pub struct ShaderVisibleHeap {
    pub heap: ID3D12DescriptorHeap,
    stride: usize,
    cpu_start: usize,
    gpu_start: u64,
    static_count: u32,
    statics: SlotAllocator,
    dynamic: RingLayout,
}

#[derive(Clone, Copy, Debug)]
pub struct DynamicRange {
    pub first: u32,
    pub count: u32,
    pub cpu: D3D12_CPU_DESCRIPTOR_HANDLE,
    pub gpu: D3D12_GPU_DESCRIPTOR_HANDLE,
}

impl ShaderVisibleHeap {
    pub fn new(
        device: &ID3D12Device,
        static_count: u32,
        dynamic_count: u32,
    ) -> Result<Self, Error> {
        let heap: ID3D12DescriptorHeap = unsafe {
            device.CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: static_count + dynamic_count,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })?
        };
        let stride = unsafe {
            device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
        } as usize;
        Ok(Self {
            stride,
            cpu_start: unsafe { heap.GetCPUDescriptorHandleForHeapStart() }.ptr,
            gpu_start: unsafe { heap.GetGPUDescriptorHandleForHeapStart() }.ptr,
            heap,
            static_count,
            statics: SlotAllocator::new(static_count),
            dynamic: RingLayout::new(dynamic_count as u64),
        })
    }

    fn at(&self, index: u32) -> (D3D12_CPU_DESCRIPTOR_HANDLE, D3D12_GPU_DESCRIPTOR_HANDLE) {
        (
            D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: self.cpu_start + index as usize * self.stride,
            },
            D3D12_GPU_DESCRIPTOR_HANDLE {
                ptr: self.gpu_start + index as u64 * self.stride as u64,
            },
        )
    }

    pub fn allocate_static(&mut self) -> Result<VisibleDescriptor, Error> {
        let index = self.statics.allocate().ok_or(Error::DescriptorsExhausted)?;
        let (cpu, gpu) = self.at(index);
        Ok(VisibleDescriptor { index, cpu, gpu })
    }

    pub fn release_static(&mut self, descriptor: VisibleDescriptor) {
        self.statics.release(descriptor.index);
    }

    pub fn allocate_dynamic(
        &mut self,
        timeline: &Timeline,
        count: u32,
    ) -> Result<DynamicRange, Error> {
        loop {
            match self.dynamic.allocate(count as u64, 1) {
                Ok(offset) => {
                    let first = self.static_count + offset as u32;
                    let (cpu, gpu) = self.at(first);
                    return Ok(DynamicRange {
                        first,
                        count,
                        cpu,
                        gpu,
                    });
                }
                Err(RingError::Full) => match self.dynamic.oldest_fence() {
                    Some(fence) => {
                        timeline.wait(fence)?;
                        self.dynamic.retire(timeline.completed());
                    }
                    None => return Err(Error::DescriptorsExhausted),
                },
                Err(RingError::TooLarge) => return Err(Error::DescriptorsExhausted),
            }
        }
    }

    pub fn end_frame(&mut self, fence: u64) {
        self.dynamic.mark(fence);
    }

    pub fn retire(&mut self, completed: u64) {
        self.dynamic.retire(completed);
    }

    pub fn live_static(&self) -> u32 {
        self.statics.live()
    }
}
