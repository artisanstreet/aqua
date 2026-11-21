use std::sync::Arc;
use std::time::Duration;

use ash::prelude::VkResult;
use ash::vk;

use crate::hal::{barrier, Access, Buffer, Checked, Device, Error, Garbage, ImageRef};
use crate::ledger::Kind;

pub const SLOTS: usize = 2;
const MIN_RING_BYTES: u64 = 1 << 16;
const STAMPS: u32 = 64;
const SETS_PER_POOL: u32 = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PassTime {
    pub label: &'static str,
    pub duration: Duration,
}

#[derive(Default)]
pub struct Stream {
    bytes: Vec<u8>,
}

impl Stream {
    pub fn push(&mut self, align: u64, data: &[u8]) -> u64 {
        let offset = (self.bytes.len() as u64).next_multiple_of(align.max(1));
        self.bytes.resize(offset as usize, 0);
        self.bytes.extend_from_slice(data);
        offset
    }
}

struct DescriptorPools {
    pools: Vec<vk::DescriptorPool>,
    current: usize,
}

fn create_pool(device: &Device) -> Result<vk::DescriptorPool, Error> {
    let sizes = [
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::UNIFORM_BUFFER)
            .descriptor_count(SETS_PER_POOL),
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLED_IMAGE)
            .descriptor_count(SETS_PER_POOL * 2),
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::SAMPLER)
            .descriptor_count(SETS_PER_POOL),
        vk::DescriptorPoolSize::default()
            .ty(vk::DescriptorType::STORAGE_IMAGE)
            .descriptor_count(SETS_PER_POOL / 4),
    ];
    let info = vk::DescriptorPoolCreateInfo::default()
        .max_sets(SETS_PER_POOL)
        .pool_sizes(&sizes);
    unsafe { device.raw.create_descriptor_pool(&info, None) }.checked("vkCreateDescriptorPool")
}

impl DescriptorPools {
    fn reset(&mut self, device: &Device) -> Result<(), Error> {
        self.current = 0;
        self.pools.iter().try_for_each(|pool| {
            unsafe {
                device
                    .raw
                    .reset_descriptor_pool(*pool, vk::DescriptorPoolResetFlags::empty())
            }
            .checked("vkResetDescriptorPool")
        })
    }

    fn allocate(
        &mut self,
        device: &Device,
        layout: vk::DescriptorSetLayout,
    ) -> Result<vk::DescriptorSet, Error> {
        loop {
            if self.current == self.pools.len() {
                self.pools.push(create_pool(device)?);
            }
            let layouts = [layout];
            let info = vk::DescriptorSetAllocateInfo::default()
                .descriptor_pool(self.pools[self.current])
                .set_layouts(&layouts);
            match unsafe { device.raw.allocate_descriptor_sets(&info) } {
                Ok(sets) => return Ok(sets[0]),
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY | vk::Result::ERROR_FRAGMENTED_POOL) => {
                    self.current += 1;
                }
                Err(result) => {
                    return VkResult::<vk::DescriptorSet>::Err(result)
                        .checked("vkAllocateDescriptorSets");
                }
            }
        }
    }
}

struct Slot {
    command_pool: vk::CommandPool,
    command: vk::CommandBuffer,
    fence: vk::Fence,
    serial: u64,
    ring: Option<Buffer>,
    descriptors: DescriptorPools,
    queries: Option<vk::QueryPool>,
    labels: Vec<&'static str>,
    stamps: u32,
}

fn create_slot(device: &Device, timestamps: bool) -> Result<Slot, Error> {
    let raw = &device.raw;
    let pool_info = vk::CommandPoolCreateInfo::default()
        .queue_family_index(device.family)
        .flags(vk::CommandPoolCreateFlags::TRANSIENT);
    let command_pool =
        unsafe { raw.create_command_pool(&pool_info, None) }.checked("vkCreateCommandPool")?;
    let allocate = vk::CommandBufferAllocateInfo::default()
        .command_pool(command_pool)
        .level(vk::CommandBufferLevel::PRIMARY)
        .command_buffer_count(1);
    let command =
        unsafe { raw.allocate_command_buffers(&allocate) }.checked("vkAllocateCommandBuffers")?[0];
    let fence = unsafe {
        raw.create_fence(
            &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
            None,
        )
    }
    .checked("vkCreateFence")?;
    let queries = match timestamps && device.info().timestamp_period.is_some() {
        true => Some(
            unsafe {
                raw.create_query_pool(
                    &vk::QueryPoolCreateInfo::default()
                        .query_type(vk::QueryType::TIMESTAMP)
                        .query_count(STAMPS),
                    None,
                )
            }
            .checked("vkCreateQueryPool")?,
        ),
        false => None,
    };
    Ok(Slot {
        command_pool,
        command,
        fence,
        serial: 0,
        ring: None,
        descriptors: DescriptorPools {
            pools: Vec::new(),
            current: 0,
        },
        queries,
        labels: Vec::with_capacity(STAMPS as usize),
        stamps: 0,
    })
}

#[derive(Default)]
struct Times {
    latest: Option<Duration>,
    passes: Vec<PassTime>,
}

pub struct Frames {
    device: Arc<Device>,
    slots: Vec<Slot>,
    next: usize,
    stream: Stream,
    times: Times,
    readback: Slot,
    readback_buffer: Option<Buffer>,
}

impl Frames {
    pub fn new(device: &Arc<Device>, timestamps: bool) -> Result<Self, Error> {
        Ok(Self {
            device: Arc::clone(device),
            slots: (0..SLOTS)
                .map(|_| create_slot(device, timestamps))
                .collect::<Result<_, _>>()?,
            next: 0,
            stream: Stream::default(),
            times: Times::default(),
            readback: create_slot(device, false)?,
            readback_buffer: None,
        })
    }

    pub fn gpu_time(&self) -> Option<Duration> {
        self.times.latest
    }

    pub fn pass_times(&self) -> &[PassTime] {
        &self.times.passes
    }

    pub fn ring_bytes(&self) -> u64 {
        self.slots
            .iter()
            .filter_map(|slot| slot.ring.as_ref())
            .map(Buffer::capacity)
            .sum()
    }

    pub fn shrink(&mut self) {
        self.slots.iter_mut().for_each(|slot| slot.ring = None);
        self.readback_buffer = None;
    }

    fn wait(device: &Device, slot: &Slot) -> Result<(), Error> {
        let waited = unsafe { device.raw.wait_for_fences(&[slot.fence], true, u64::MAX) }
            .checked("vkWaitForFences");
        device.observe(waited)?;
        device.completed(slot.serial);
        Ok(())
    }

    fn collect_times(device: &Device, slot: &Slot, times: &mut Times) {
        let (Some(pool), Some(period)) = (slot.queries, device.info().timestamp_period) else {
            return;
        };
        if slot.stamps < 2 {
            return;
        }
        let mut results = vec![[0u64; 2]; slot.stamps as usize];
        let read = unsafe {
            device.raw.get_query_pool_results(
                pool,
                0,
                &mut results,
                vk::QueryResultFlags::TYPE_64 | vk::QueryResultFlags::WITH_AVAILABILITY,
            )
        };
        if read.is_err() || results.iter().any(|[_, available]| *available == 0) {
            return;
        }
        let span = |start: u64, end: u64| {
            end.checked_sub(start)
                .map(|ticks| Duration::from_nanos((ticks as f64 * period as f64) as u64))
        };
        let stamps: Vec<u64> = results.iter().map(|[value, _]| *value).collect();
        times.latest = span(stamps[0], stamps[stamps.len() - 1]);
        times.passes.clear();
        times
            .passes
            .extend(
                slot.labels
                    .iter()
                    .zip(stamps[1..].windows(2))
                    .filter_map(|(label, pair)| {
                        span(pair[0], pair[1]).map(|duration| PassTime { label, duration })
                    }),
            );
    }

    fn ensure_ring(device: &Arc<Device>, slot: &mut Slot, bytes: u64) -> Result<(), Error> {
        let wanted = bytes.max(MIN_RING_BYTES).next_power_of_two();
        if slot
            .ring
            .as_ref()
            .is_some_and(|ring| ring.capacity() >= bytes)
        {
            return Ok(());
        }
        slot.ring = Some(Buffer::host(
            device,
            wanted,
            vk::BufferUsageFlags::VERTEX_BUFFER
                | vk::BufferUsageFlags::UNIFORM_BUFFER
                | vk::BufferUsageFlags::TRANSFER_SRC,
            "frame_ring",
            Kind::DynamicBuffer,
        )?);
        Ok(())
    }

    pub fn begin(&mut self, ring_bytes: u64) -> Result<Frame<'_>, Error> {
        self.device.check("frame begin")?;
        let index = self.next;
        self.next = (self.next + 1) % self.slots.len();
        let device = &self.device;
        let slot = &mut self.slots[index];
        Self::wait(device, slot)?;
        Self::collect_times(device, slot, &mut self.times);
        Self::ensure_ring(device, slot, ring_bytes)?;
        slot.descriptors.reset(device)?;
        unsafe {
            device
                .raw
                .reset_command_pool(slot.command_pool, vk::CommandPoolResetFlags::empty())
        }
        .checked("vkResetCommandPool")?;
        unsafe {
            device.raw.begin_command_buffer(
                slot.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }
        .checked("vkBeginCommandBuffer")?;
        slot.labels.clear();
        slot.stamps = 0;
        if let Some(pool) = slot.queries {
            unsafe {
                device
                    .raw
                    .cmd_reset_query_pool(slot.command, pool, 0, STAMPS);
                device.raw.cmd_write_timestamp2(
                    slot.command,
                    vk::PipelineStageFlags2::ALL_COMMANDS,
                    pool,
                    0,
                );
            }
            slot.stamps = 1;
        }
        self.stream.bytes.clear();
        Ok(Frame {
            device,
            slot,
            stream: &mut self.stream,
            barriers: 0,
        })
    }

    pub fn read_image(&mut self, image: ImageRef<'_>, texel_bytes: u64) -> Result<Vec<u8>, Error> {
        self.device.check("surface readback")?;
        let bytes = image.extent.texels() * texel_bytes;
        let device = &self.device;
        let slot = &mut self.readback;
        Self::wait(device, slot)?;
        if self
            .readback_buffer
            .as_ref()
            .is_none_or(|buffer| buffer.capacity() < bytes)
        {
            self.readback_buffer = Some(Buffer::host(
                device,
                bytes.next_power_of_two(),
                vk::BufferUsageFlags::TRANSFER_DST,
                "readback_buffer",
                Kind::StagingTexture,
            )?);
        }
        let buffer = self
            .readback_buffer
            .as_ref()
            .expect("the readback buffer was just ensured");
        let raw = &device.raw;
        unsafe { raw.reset_command_pool(slot.command_pool, vk::CommandPoolResetFlags::empty()) }
            .checked("vkResetCommandPool")?;
        unsafe {
            raw.begin_command_buffer(
                slot.command,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
        }
        .checked("vkBeginCommandBuffer")?;
        let before = image.access.get();
        let transitions: Vec<_> = image
            .transition(Access::TransferSource)
            .into_iter()
            .collect();
        barrier(device, slot.command, &transitions);
        let region = vk::BufferImageCopy::default()
            .image_subresource(vk::ImageSubresourceLayers {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            })
            .image_extent(vk::Extent3D {
                width: image.extent.width,
                height: image.extent.height,
                depth: 1,
            });
        unsafe {
            raw.cmd_copy_image_to_buffer(
                slot.command,
                image.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                buffer.raw(),
                &[region],
            )
        };
        if before == Access::Present {
            let restore: Vec<_> = image.transition(Access::Present).into_iter().collect();
            barrier(device, slot.command, &restore);
        }
        submit(device, slot, None)?;
        Self::wait(device, slot)?;
        buffer.read(bytes as usize, <[u8]>::to_vec)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Presentable {
    pub acquired: vk::Semaphore,
    pub rendered: vk::Semaphore,
}

fn submit(
    device: &Device,
    slot: &mut Slot,
    presentable: Option<Presentable>,
) -> Result<u64, Error> {
    let raw = &device.raw;
    unsafe { raw.end_command_buffer(slot.command) }.checked("vkEndCommandBuffer")?;
    let waits: Vec<vk::Semaphore> = presentable.iter().map(|sync| sync.acquired).collect();
    let signals: Vec<vk::Semaphore> = presentable.iter().map(|sync| sync.rendered).collect();
    let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
    let submitted = device
        .check("queue submit")
        .and_then(|()| unsafe { raw.reset_fences(&[slot.fence]) }.checked("vkResetFences"))
        .and_then(|()| {
            let commands = [slot.command];
            let info = vk::SubmitInfo::default()
                .command_buffers(&commands)
                .wait_semaphores(&waits)
                .wait_dst_stage_mask(&stages)
                .signal_semaphores(&signals);
            unsafe { raw.queue_submit(device.queue, &[info], slot.fence) }.checked("vkQueueSubmit")
        });
    device.observe(submitted)?;
    slot.serial = device.begin_submission();
    Ok(slot.serial)
}

pub struct Frame<'a> {
    device: &'a Arc<Device>,
    slot: &'a mut Slot,
    pub stream: &'a mut Stream,
    barriers: u32,
}

impl Frame<'_> {
    pub fn device(&self) -> &Arc<Device> {
        self.device
    }

    pub fn command(&self) -> vk::CommandBuffer {
        self.slot.command
    }

    pub fn ring(&self) -> vk::Buffer {
        self.slot
            .ring
            .as_ref()
            .expect("a frame always holds its ring")
            .raw()
    }

    pub fn barriers(&self) -> u32 {
        self.barriers
    }

    pub fn barrier(&mut self, barriers: &[vk::ImageMemoryBarrier2<'_>]) {
        self.barriers += barriers.len() as u32;
        barrier(self.device, self.slot.command, barriers);
    }

    pub fn mark(&mut self, label: &'static str) {
        let Some(pool) = self.slot.queries else {
            return;
        };
        if self.slot.stamps + 1 >= STAMPS {
            return;
        }
        unsafe {
            self.device.raw.cmd_write_timestamp2(
                self.slot.command,
                vk::PipelineStageFlags2::ALL_COMMANDS,
                pool,
                self.slot.stamps,
            )
        };
        self.slot.labels.push(label);
        self.slot.stamps += 1;
    }

    pub fn descriptor_set(
        &mut self,
        layout: vk::DescriptorSetLayout,
    ) -> Result<vk::DescriptorSet, Error> {
        self.slot.descriptors.allocate(self.device, layout)
    }

    pub fn submit(self) -> Result<(), Error> {
        self.finish(None).map(drop)
    }

    pub fn submit_presentable(self, presentable: Presentable) -> Result<u64, Error> {
        self.finish(Some(presentable))
    }

    fn finish(self, presentable: Option<Presentable>) -> Result<u64, Error> {
        if let Some(pool) = self.slot.queries {
            unsafe {
                self.device.raw.cmd_write_timestamp2(
                    self.slot.command,
                    vk::PipelineStageFlags2::ALL_COMMANDS,
                    pool,
                    self.slot.stamps,
                )
            };
            self.slot.stamps += 1;
        }
        self.slot
            .ring
            .as_ref()
            .expect("a frame always holds its ring")
            .write(&self.stream.bytes)?;
        submit(self.device, self.slot, presentable)
    }
}

fn retire_slot(device: &Device, slot: &mut Slot) {
    device.retire(Garbage::CommandPool(slot.command_pool));
    device.retire(Garbage::Fence(slot.fence));
    slot.descriptors
        .pools
        .drain(..)
        .for_each(|pool| device.retire(Garbage::DescriptorPool(pool)));
    if let Some(pool) = slot.queries.take() {
        device.retire(Garbage::QueryPool(pool));
    }
}

impl Drop for Frames {
    fn drop(&mut self) {
        let device = Arc::clone(&self.device);
        let _ = device.idle();
        self.slots
            .iter_mut()
            .chain(std::iter::once(&mut self.readback))
            .for_each(|slot| retire_slot(&device, slot));
    }
}
