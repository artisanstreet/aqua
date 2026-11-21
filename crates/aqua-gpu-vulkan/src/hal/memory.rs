use std::cell::Cell;
use std::sync::Arc;

use aqua_graph::Extent;
use ash::vk;

use super::device::{Device, Garbage};
use super::error::{Checked, Error};
use crate::ledger::{record, Kind, Token};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Undefined,
    Attachment,
    Sampled,
    Storage,
    TransferSource,
    TransferDestination,
}

impl Access {
    fn layout(self) -> vk::ImageLayout {
        match self {
            Access::Undefined => vk::ImageLayout::UNDEFINED,
            Access::Attachment => vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
            Access::Sampled => vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
            Access::Storage => vk::ImageLayout::GENERAL,
            Access::TransferSource => vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            Access::TransferDestination => vk::ImageLayout::TRANSFER_DST_OPTIMAL,
        }
    }

    fn stage(self) -> vk::PipelineStageFlags2 {
        match self {
            Access::Undefined => vk::PipelineStageFlags2::NONE,
            Access::Attachment => vk::PipelineStageFlags2::COLOR_ATTACHMENT_OUTPUT,
            Access::Sampled => {
                vk::PipelineStageFlags2::FRAGMENT_SHADER | vk::PipelineStageFlags2::COMPUTE_SHADER
            }
            Access::Storage => vk::PipelineStageFlags2::COMPUTE_SHADER,
            Access::TransferSource | Access::TransferDestination => {
                vk::PipelineStageFlags2::ALL_TRANSFER
            }
        }
    }

    fn access(self) -> vk::AccessFlags2 {
        match self {
            Access::Undefined => vk::AccessFlags2::NONE,
            Access::Attachment => {
                vk::AccessFlags2::COLOR_ATTACHMENT_READ | vk::AccessFlags2::COLOR_ATTACHMENT_WRITE
            }
            Access::Sampled => vk::AccessFlags2::SHADER_SAMPLED_READ,
            Access::Storage => vk::AccessFlags2::SHADER_STORAGE_WRITE,
            Access::TransferSource => vk::AccessFlags2::TRANSFER_READ,
            Access::TransferDestination => vk::AccessFlags2::TRANSFER_WRITE,
        }
    }

    fn writes(self) -> bool {
        matches!(
            self,
            Access::Attachment | Access::Storage | Access::TransferDestination
        )
    }
}

pub const COLOR: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

#[derive(Clone, Copy)]
pub struct ImageRef<'a> {
    pub image: vk::Image,
    pub view: vk::ImageView,
    pub format: vk::Format,
    pub extent: Extent,
    pub access: &'a Cell<Access>,
}

impl ImageRef<'_> {
    pub fn same_image(&self, other: &ImageRef<'_>) -> bool {
        self.image == other.image
    }

    pub fn transition(&self, to: Access) -> Option<vk::ImageMemoryBarrier2<'static>> {
        let from = self.access.get();
        if from == to && !from.writes() {
            return None;
        }
        self.access.set(to);
        Some(
            vk::ImageMemoryBarrier2::default()
                .src_stage_mask(from.stage())
                .src_access_mask(from.access())
                .dst_stage_mask(to.stage())
                .dst_access_mask(to.access())
                .old_layout(from.layout())
                .new_layout(to.layout())
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(self.image)
                .subresource_range(COLOR),
        )
    }
}

pub fn barrier(
    device: &Device,
    command: vk::CommandBuffer,
    barriers: &[vk::ImageMemoryBarrier2<'_>],
) {
    if barriers.is_empty() {
        return;
    }
    let dependency = vk::DependencyInfo::default().image_memory_barriers(barriers);
    unsafe { device.raw.cmd_pipeline_barrier2(command, &dependency) };
}

pub struct ImageDesc {
    pub extent: Extent,
    pub format: vk::Format,
    pub usage: vk::ImageUsageFlags,
    pub label: &'static str,
    pub kind: Kind,
    pub bytes: u64,
}

pub struct Image {
    device: Arc<Device>,
    raw: vk::Image,
    view: vk::ImageView,
    memory: vk::DeviceMemory,
    format: vk::Format,
    extent: Extent,
    access: Cell<Access>,
    _token: Token,
}

impl Image {
    pub fn new(device: &Arc<Device>, desc: ImageDesc) -> Result<Self, Error> {
        let raw_device = &device.raw;
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(desc.format)
            .extent(vk::Extent3D {
                width: desc.extent.width.max(1),
                height: desc.extent.height.max(1),
                depth: 1,
            })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(desc.usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let raw = device
            .observe(unsafe { raw_device.create_image(&info, None) }.checked("vkCreateImage"))?;
        let requirements = unsafe { raw_device.get_image_memory_requirements(raw) };
        let memory = device
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::DEVICE_LOCAL,
                desc.label,
            )
            .and_then(|memory_type| {
                let allocation = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type);
                device.observe(
                    unsafe { raw_device.allocate_memory(&allocation, None) }
                        .checked("vkAllocateMemory"),
                )
            });
        let memory = match memory {
            Ok(memory) => memory,
            Err(error) => {
                unsafe { raw_device.destroy_image(raw, None) };
                return Err(error);
            }
        };
        let view = unsafe { raw_device.bind_image_memory(raw, memory, 0) }
            .checked("vkBindImageMemory")
            .and_then(|()| {
                let view = vk::ImageViewCreateInfo::default()
                    .image(raw)
                    .view_type(vk::ImageViewType::TYPE_2D)
                    .format(desc.format)
                    .subresource_range(COLOR);
                unsafe { raw_device.create_image_view(&view, None) }.checked("vkCreateImageView")
            });
        let view = match device.observe(view) {
            Ok(view) => view,
            Err(error) => {
                unsafe {
                    raw_device.destroy_image(raw, None);
                    raw_device.free_memory(memory, None);
                }
                return Err(error);
            }
        };
        Ok(Self {
            device: Arc::clone(device),
            raw,
            view,
            memory,
            format: desc.format,
            extent: desc.extent,
            access: Cell::new(Access::Undefined),
            _token: record(desc.label, desc.kind, desc.bytes.max(requirements.size)),
        })
    }

    pub fn as_ref(&self) -> ImageRef<'_> {
        ImageRef {
            image: self.raw,
            view: self.view,
            format: self.format,
            extent: self.extent,
            access: &self.access,
        }
    }

    pub fn extent(&self) -> Extent {
        self.extent
    }

    pub fn format(&self) -> vk::Format {
        self.format
    }
}

impl Drop for Image {
    fn drop(&mut self) {
        self.device.retire(Garbage::Image {
            image: self.raw,
            view: self.view,
            memory: self.memory,
        });
    }
}

pub struct Buffer {
    device: Arc<Device>,
    raw: vk::Buffer,
    memory: vk::DeviceMemory,
    capacity: u64,
    _token: Token,
}

impl Buffer {
    pub fn host(
        device: &Arc<Device>,
        capacity: u64,
        usage: vk::BufferUsageFlags,
        label: &'static str,
        kind: Kind,
    ) -> Result<Self, Error> {
        let raw_device = &device.raw;
        let info = vk::BufferCreateInfo::default()
            .size(capacity.max(16))
            .usage(usage)
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let raw = device
            .observe(unsafe { raw_device.create_buffer(&info, None) }.checked("vkCreateBuffer"))?;
        let requirements = unsafe { raw_device.get_buffer_memory_requirements(raw) };
        let memory = device
            .memory_type(
                requirements.memory_type_bits,
                vk::MemoryPropertyFlags::HOST_VISIBLE | vk::MemoryPropertyFlags::HOST_COHERENT,
                label,
            )
            .and_then(|memory_type| {
                let allocation = vk::MemoryAllocateInfo::default()
                    .allocation_size(requirements.size)
                    .memory_type_index(memory_type);
                unsafe { raw_device.allocate_memory(&allocation, None) }.checked("vkAllocateMemory")
            })
            .and_then(|memory| {
                match unsafe { raw_device.bind_buffer_memory(raw, memory, 0) }
                    .checked("vkBindBufferMemory")
                {
                    Ok(()) => Ok(memory),
                    Err(error) => {
                        unsafe { raw_device.free_memory(memory, None) };
                        Err(error)
                    }
                }
            });
        match device.observe(memory) {
            Ok(memory) => Ok(Self {
                device: Arc::clone(device),
                raw,
                memory,
                capacity: capacity.max(16),
                _token: record(label, kind, requirements.size),
            }),
            Err(error) => {
                unsafe { raw_device.destroy_buffer(raw, None) };
                Err(error)
            }
        }
    }

    pub fn raw(&self) -> vk::Buffer {
        self.raw
    }

    pub fn capacity(&self) -> u64 {
        self.capacity
    }

    pub fn write(&self, bytes: &[u8]) -> Result<(), Error> {
        assert!(
            bytes.len() as u64 <= self.capacity,
            "a {} byte write does not fit a {} byte host buffer",
            bytes.len(),
            self.capacity
        );
        if bytes.is_empty() {
            return Ok(());
        }
        let device = &self.device.raw;
        let mapped = self.device.observe(
            unsafe {
                device.map_memory(
                    self.memory,
                    0,
                    bytes.len() as u64,
                    vk::MemoryMapFlags::empty(),
                )
            }
            .checked("vkMapMemory"),
        )?;
        unsafe {
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), mapped.cast::<u8>(), bytes.len());
            device.unmap_memory(self.memory);
        }
        Ok(())
    }

    pub fn read<T>(&self, length: usize, read: impl FnOnce(&[u8]) -> T) -> Result<T, Error> {
        assert!(
            length as u64 <= self.capacity,
            "a {length} byte read does not fit a {} byte host buffer",
            self.capacity
        );
        let device = &self.device.raw;
        let mapped = self.device.observe(
            unsafe {
                device.map_memory(self.memory, 0, vk::WHOLE_SIZE, vk::MemoryMapFlags::empty())
            }
            .checked("vkMapMemory"),
        )?;
        let value = read(unsafe { std::slice::from_raw_parts(mapped.cast::<u8>(), length) });
        unsafe { device.unmap_memory(self.memory) };
        Ok(value)
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.device.retire(Garbage::Buffer {
            buffer: self.raw,
            memory: self.memory,
        });
    }
}
