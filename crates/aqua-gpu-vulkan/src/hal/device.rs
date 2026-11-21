use std::ffi::CStr;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ash::vk;

use super::budget::MemoryBudget;
use super::error::{Checked, Error};
use super::instance::{Instance, Purpose, SurfaceLoaders, Validation};
use super::selection::{choose, Adapter, AdapterPreference, Candidate, Fitness};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Descriptors {
    #[default]
    Automatic,
    Pool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DeviceOptions {
    pub preference: AdapterPreference,
    pub validation: Validation,
    pub descriptors: Descriptors,
}

impl DeviceOptions {
    pub fn from_env() -> Self {
        Self {
            preference: AdapterPreference::from_env(),
            validation: Validation::from_env(),
            descriptors: Descriptors::Automatic,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Binding {
    Push,
    Pool,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DeviceInfo {
    pub name: String,
    pub kind: Adapter,
    pub api_version: u32,
    pub driver_version: u32,
    pub vendor_id: u32,
    pub device_id: u32,
    pub cache_uuid: [u8; vk::UUID_SIZE],
    pub max_image_extent: u32,
    pub uniform_alignment: u64,
    pub timestamp_period: Option<f32>,
    pub binding: Binding,
    pub memory_budget: bool,
    pub storage_without_format: bool,
}

pub(crate) enum Garbage {
    Image {
        image: vk::Image,
        view: vk::ImageView,
        memory: vk::DeviceMemory,
    },
    Buffer {
        buffer: vk::Buffer,
        memory: vk::DeviceMemory,
    },
    Pipeline(vk::Pipeline),
    PipelineLayout(vk::PipelineLayout),
    SetLayout(vk::DescriptorSetLayout),
    DescriptorPool(vk::DescriptorPool),
    CommandPool(vk::CommandPool),
    Fence(vk::Fence),
    QueryPool(vk::QueryPool),
    Sampler(vk::Sampler),
    ShaderModule(vk::ShaderModule),
    PipelineCache(vk::PipelineCache),
}

pub(crate) struct Presenting<'a> {
    pub loaders: &'a SurfaceLoaders,
    pub swapchain: &'a ash::khr::swapchain::Device,
    pub physical: vk::PhysicalDevice,
    pub family: u32,
    pub incremental: bool,
}

pub struct Device {
    instance: Instance,
    swapchain: Option<ash::khr::swapchain::Device>,
    incremental_present: bool,
    physical: vk::PhysicalDevice,
    pub(crate) raw: ash::Device,
    pub(crate) queue: vk::Queue,
    pub(crate) family: u32,
    pub(crate) push: Option<ash::khr::push_descriptor::Device>,
    info: DeviceInfo,
    memory: vk::PhysicalDeviceMemoryProperties,
    graveyard: Mutex<Vec<(u64, Garbage)>>,
    submitted: AtomicU64,
    completed: AtomicU64,
    lost: AtomicBool,
}

const REQUIRED_API: u32 = vk::API_VERSION_1_3;

fn kind(device_type: vk::PhysicalDeviceType) -> Adapter {
    match device_type {
        vk::PhysicalDeviceType::DISCRETE_GPU => Adapter::Discrete,
        vk::PhysicalDeviceType::INTEGRATED_GPU => Adapter::Integrated,
        vk::PhysicalDeviceType::VIRTUAL_GPU => Adapter::Virtual,
        vk::PhysicalDeviceType::CPU => Adapter::Software,
        _ => Adapter::Other,
    }
}

fn has_extension(extensions: &[vk::ExtensionProperties], name: &CStr) -> bool {
    extensions
        .iter()
        .any(|extension| extension.extension_name_as_c_str() == Ok(name))
}

struct Probe {
    physical: vk::PhysicalDevice,
    properties: vk::PhysicalDeviceProperties,
    family: Option<u32>,
    timestamps: bool,
    push: bool,
    budget: bool,
    swapchain: bool,
    incremental: bool,
    without_format: bool,
}

fn graphics_family(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Option<(u32, bool)> {
    let wanted = vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE;
    unsafe { instance.get_physical_device_queue_family_properties(physical) }
        .iter()
        .enumerate()
        .find(|(_, family)| family.queue_flags.contains(wanted))
        .map(|(index, family)| (index as u32, family.timestamp_valid_bits > 0))
}

fn fitness(instance: &ash::Instance, probe: &Probe, purpose: Purpose) -> Fitness {
    match (purpose, probe.swapchain) {
        (Purpose::Present, false) => Fitness::Lacks("VK_KHR_swapchain"),
        _ => core_fitness(instance, probe),
    }
}

fn core_fitness(instance: &ash::Instance, probe: &Probe) -> Fitness {
    let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut features = vk::PhysicalDeviceFeatures2::default().push_next(&mut v13);
    if probe.properties.api_version >= REQUIRED_API {
        unsafe { instance.get_physical_device_features2(probe.physical, &mut features) };
    }
    let dual = features.features.dual_src_blend == vk::TRUE;
    match (
        probe.properties.api_version >= REQUIRED_API,
        probe.family.is_some(),
        dual,
        v13.dynamic_rendering == vk::TRUE,
        v13.synchronization2 == vk::TRUE,
    ) {
        (false, ..) => Fitness::Lacks("Vulkan 1.3"),
        (_, false, ..) => Fitness::Lacks("a graphics and compute queue"),
        (_, _, false, ..) => Fitness::Lacks("dual-source blending"),
        (_, _, _, false, _) => Fitness::Lacks("dynamic rendering"),
        (_, _, _, _, false) => Fitness::Lacks("synchronization2"),
        _ => Fitness::Usable,
    }
}

fn probe(instance: &ash::Instance, physical: vk::PhysicalDevice) -> Result<Probe, Error> {
    let properties = unsafe { instance.get_physical_device_properties(physical) };
    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }
        .checked("vkEnumerateDeviceExtensionProperties")?;
    let family = graphics_family(instance, physical);
    let core = unsafe { instance.get_physical_device_features(physical) };
    Ok(Probe {
        without_format: core.shader_storage_image_write_without_format == vk::TRUE
            && core.shader_storage_image_read_without_format == vk::TRUE,
        physical,
        properties,
        family: family.map(|(index, _)| index),
        timestamps: family.is_some_and(|(_, timestamps)| timestamps),
        push: has_extension(&extensions, ash::khr::push_descriptor::NAME),
        budget: has_extension(&extensions, ash::ext::memory_budget::NAME),
        swapchain: has_extension(&extensions, ash::khr::swapchain::NAME),
        incremental: has_extension(&extensions, ash::khr::incremental_present::NAME),
    })
}

fn device_name(properties: &vk::PhysicalDeviceProperties) -> String {
    properties
        .device_name_as_c_str()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|_| "unnamed Vulkan device".to_owned())
}

fn select(
    instance: &ash::Instance,
    preference: &AdapterPreference,
    purpose: Purpose,
) -> Result<Probe, Error> {
    let devices =
        unsafe { instance.enumerate_physical_devices() }.checked("vkEnumeratePhysicalDevices")?;
    let mut probes = devices
        .into_iter()
        .map(|physical| probe(instance, physical))
        .collect::<Result<Vec<_>, Error>>()?;
    let candidates: Vec<Candidate> = probes
        .iter()
        .map(|probe| Candidate {
            name: device_name(&probe.properties),
            kind: kind(probe.properties.device_type),
            fitness: fitness(instance, probe, purpose),
        })
        .collect();
    match choose(preference, &candidates) {
        Some(index) => Ok(probes.swap_remove(index)),
        None => Err(Error::NoAdapter {
            preference: preference.clone(),
            found: candidates
                .iter()
                .map(|candidate| match &candidate.fitness {
                    Fitness::Usable => format!("{} ({:?})", candidate.name, candidate.kind),
                    Fitness::Lacks(what) => format!("{} (lacks {what})", candidate.name),
                })
                .collect::<Vec<_>>()
                .join(", "),
        }),
    }
}

impl Device {
    pub fn create(options: &DeviceOptions) -> Result<Arc<Self>, Error> {
        Self::create_for(options, Purpose::Offscreen)
    }

    pub fn create_presenting(options: &DeviceOptions) -> Result<Arc<Self>, Error> {
        Self::create_for(options, Purpose::Present)
    }

    fn create_for(options: &DeviceOptions, purpose: Purpose) -> Result<Arc<Self>, Error> {
        let instance = Instance::new(options.validation, purpose)?;
        let chosen = select(&instance.raw, &options.preference, purpose)?;
        let family = chosen
            .family
            .expect("a usable adapter always has a graphics and compute queue");
        let push = chosen.push && options.descriptors == Descriptors::Automatic;
        let mut extensions: Vec<*const i8> = Vec::new();
        if push {
            extensions.push(ash::khr::push_descriptor::NAME.as_ptr());
        }
        if chosen.budget {
            extensions.push(ash::ext::memory_budget::NAME.as_ptr());
        }
        let presenting = purpose == Purpose::Present;
        let incremental_present = presenting && chosen.incremental;
        if presenting {
            extensions.push(ash::khr::swapchain::NAME.as_ptr());
        }
        if incremental_present {
            extensions.push(ash::khr::incremental_present::NAME.as_ptr());
        }
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default()
            .queue_family_index(family)
            .queue_priorities(&priorities)];
        let core = vk::PhysicalDeviceFeatures::default()
            .dual_src_blend(true)
            .shader_storage_image_write_without_format(chosen.without_format)
            .shader_storage_image_read_without_format(chosen.without_format);
        let mut v13 = vk::PhysicalDeviceVulkan13Features::default()
            .dynamic_rendering(true)
            .synchronization2(true);
        let info = vk::DeviceCreateInfo::default()
            .queue_create_infos(&queues)
            .enabled_extension_names(&extensions)
            .enabled_features(&core)
            .push_next(&mut v13);
        let raw = unsafe { instance.raw.create_device(chosen.physical, &info, None) }
            .checked("vkCreateDevice")?;
        let queue = unsafe { raw.get_device_queue(family, 0) };
        let memory = unsafe {
            instance
                .raw
                .get_physical_device_memory_properties(chosen.physical)
        };
        let limits = chosen.properties.limits;
        let info = DeviceInfo {
            name: device_name(&chosen.properties),
            kind: kind(chosen.properties.device_type),
            api_version: chosen.properties.api_version,
            driver_version: chosen.properties.driver_version,
            vendor_id: chosen.properties.vendor_id,
            device_id: chosen.properties.device_id,
            cache_uuid: chosen.properties.pipeline_cache_uuid,
            max_image_extent: limits.max_image_dimension2_d,
            uniform_alignment: limits.min_uniform_buffer_offset_alignment.max(16),
            timestamp_period: (chosen.timestamps
                && limits.timestamp_compute_and_graphics == vk::TRUE)
                .then_some(limits.timestamp_period),
            binding: match push {
                true => Binding::Push,
                false => Binding::Pool,
            },
            memory_budget: chosen.budget,
            storage_without_format: chosen.without_format,
        };
        Ok(Arc::new(Self {
            push: push.then(|| ash::khr::push_descriptor::Device::new(&instance.raw, &raw)),
            swapchain: presenting.then(|| ash::khr::swapchain::Device::new(&instance.raw, &raw)),
            incremental_present,
            physical: chosen.physical,
            instance,
            raw,
            queue,
            family,
            info,
            memory,
            graveyard: Mutex::new(Vec::new()),
            submitted: AtomicU64::new(0),
            completed: AtomicU64::new(0),
            lost: AtomicBool::new(false),
        }))
    }

    pub fn info(&self) -> &DeviceInfo {
        &self.info
    }

    pub fn validating(&self) -> bool {
        self.instance.validating()
    }

    pub fn validation_errors(&self) -> Vec<String> {
        self.instance.validation_errors()
    }

    pub(crate) fn presenting(&self) -> Option<Presenting<'_>> {
        match (&self.instance.surfaces, &self.swapchain) {
            (Some(loaders), Some(swapchain)) => Some(Presenting {
                loaders,
                swapchain,
                physical: self.physical,
                family: self.family,
                incremental: self.incremental_present,
            }),
            _ => None,
        }
    }

    pub fn incremental_present(&self) -> bool {
        self.incremental_present
    }

    pub(crate) fn completed_serial(&self) -> u64 {
        self.completed.load(Ordering::SeqCst)
    }

    pub(crate) fn wait_semaphore(&self, semaphore: vk::Semaphore) -> Result<(), Error> {
        let semaphores = [semaphore];
        let stages = [vk::PipelineStageFlags::ALL_COMMANDS];
        let info = vk::SubmitInfo::default()
            .wait_semaphores(&semaphores)
            .wait_dst_stage_mask(&stages);
        let submitted = unsafe {
            self.raw
                .queue_submit(self.queue, &[info], vk::Fence::null())
        }
        .checked("vkQueueSubmit");
        self.observe(submitted)
    }

    pub fn inject_loss(&self) {
        self.lost.store(true, Ordering::SeqCst);
    }

    pub fn is_lost(&self) -> bool {
        self.lost.load(Ordering::SeqCst)
    }

    pub(crate) fn check(&self, call: &'static str) -> Result<(), Error> {
        match self.is_lost() {
            true => Err(Error::DeviceLost { call }),
            false => Ok(()),
        }
    }

    pub(crate) fn observe<T>(&self, result: Result<T, Error>) -> Result<T, Error> {
        if let Err(Error::DeviceLost { .. }) = &result {
            self.lost.store(true, Ordering::SeqCst);
        }
        result
    }

    pub(crate) fn memory_type(
        &self,
        bits: u32,
        wanted: vk::MemoryPropertyFlags,
        purpose: &'static str,
    ) -> Result<u32, Error> {
        (0..self.memory.memory_type_count)
            .find(|index| {
                bits & (1 << index) != 0
                    && self.memory.memory_types[*index as usize]
                        .property_flags
                        .contains(wanted)
            })
            .ok_or(Error::NoMemoryType { purpose, wanted })
    }

    pub fn memory_budget(&self) -> Option<MemoryBudget> {
        if !self.info.memory_budget {
            return None;
        }
        let mut budget = vk::PhysicalDeviceMemoryBudgetPropertiesEXT::default();
        let mut properties = vk::PhysicalDeviceMemoryProperties2::default().push_next(&mut budget);
        unsafe {
            self.instance
                .raw
                .get_physical_device_memory_properties2(self.physical, &mut properties)
        };
        let heaps = properties.memory_properties.memory_heap_count as usize;
        let local = properties.memory_properties.memory_heaps[..heaps]
            .iter()
            .enumerate()
            .filter(|(_, heap)| heap.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
            .map(|(index, _)| index);
        let (budget, usage) = local.fold((0u64, 0u64), |(total, used), index| {
            (
                total + budget.heap_budget[index],
                used + budget.heap_usage[index],
            )
        });
        Some(MemoryBudget { budget, usage })
    }

    pub(crate) fn begin_submission(&self) -> u64 {
        self.submitted.fetch_add(1, Ordering::SeqCst) + 1
    }

    pub(crate) fn retire(&self, garbage: Garbage) {
        let serial = self.submitted.load(Ordering::SeqCst) + 1;
        self.graveyard
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .push((serial, garbage));
    }

    pub(crate) fn completed(&self, serial: u64) {
        let completed = self
            .completed
            .fetch_max(serial, Ordering::SeqCst)
            .max(serial);
        let ready: Vec<Garbage> = {
            let mut graveyard = self
                .graveyard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let (ready, waiting) = std::mem::take(&mut *graveyard)
                .into_iter()
                .partition(|(retired, _)| *retired <= completed);
            *graveyard = waiting;
            ready.into_iter().map(|(_, garbage)| garbage).collect()
        };
        ready.into_iter().for_each(|garbage| self.destroy(garbage));
    }

    pub(crate) fn idle(&self) -> Result<(), Error> {
        let waited = unsafe { self.raw.device_wait_idle() }.checked("vkDeviceWaitIdle");
        self.observe(waited)?;
        self.completed(self.submitted.load(Ordering::SeqCst));
        Ok(())
    }

    fn destroy(&self, garbage: Garbage) {
        let device = &self.raw;
        unsafe {
            match garbage {
                Garbage::Image {
                    image,
                    view,
                    memory,
                } => {
                    device.destroy_image_view(view, None);
                    device.destroy_image(image, None);
                    device.free_memory(memory, None);
                }
                Garbage::Buffer { buffer, memory } => {
                    device.destroy_buffer(buffer, None);
                    device.free_memory(memory, None);
                }
                Garbage::Pipeline(pipeline) => device.destroy_pipeline(pipeline, None),
                Garbage::PipelineLayout(layout) => device.destroy_pipeline_layout(layout, None),
                Garbage::SetLayout(layout) => device.destroy_descriptor_set_layout(layout, None),
                Garbage::DescriptorPool(pool) => device.destroy_descriptor_pool(pool, None),
                Garbage::CommandPool(pool) => device.destroy_command_pool(pool, None),
                Garbage::Fence(fence) => device.destroy_fence(fence, None),
                Garbage::QueryPool(pool) => device.destroy_query_pool(pool, None),
                Garbage::Sampler(sampler) => device.destroy_sampler(sampler, None),
                Garbage::ShaderModule(module) => device.destroy_shader_module(module, None),
                Garbage::PipelineCache(cache) => device.destroy_pipeline_cache(cache, None),
            }
        }
    }
}

impl Drop for Device {
    fn drop(&mut self) {
        let _ = unsafe { self.raw.device_wait_idle() };
        let remaining = std::mem::take(
            &mut *self
                .graveyard
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner()),
        );
        remaining
            .into_iter()
            .for_each(|(_, garbage)| self.destroy(garbage));
        unsafe { self.raw.destroy_device(None) };
    }
}
