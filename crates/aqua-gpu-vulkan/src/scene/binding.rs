use std::sync::Arc;

use ash::vk;

use super::frames::Frame;
use crate::hal::{Binding, Checked, Device, Error, Garbage};

pub const UNIFORM: u32 = 0;
pub const SOURCE: u32 = 16;
pub const SECOND_SOURCE: u32 = 17;
pub const SAMPLER: u32 = 32;
pub const OUTPUT: u32 = 48;

pub struct Layouts {
    device: Arc<Device>,
    sampler: vk::Sampler,
    graphics_set: vk::DescriptorSetLayout,
    compute_set: vk::DescriptorSetLayout,
    pub graphics: vk::PipelineLayout,
    pub compute: vk::PipelineLayout,
}

pub struct Uniform {
    pub offset: u64,
    pub range: u64,
}

fn set_layout(
    device: &Device,
    bindings: &[vk::DescriptorSetLayoutBinding<'_>],
) -> Result<vk::DescriptorSetLayout, Error> {
    let flags = match device.info().binding {
        Binding::Push => vk::DescriptorSetLayoutCreateFlags::PUSH_DESCRIPTOR_KHR,
        Binding::Pool => vk::DescriptorSetLayoutCreateFlags::empty(),
    };
    let info = vk::DescriptorSetLayoutCreateInfo::default()
        .flags(flags)
        .bindings(bindings);
    unsafe { device.raw.create_descriptor_set_layout(&info, None) }
        .checked("vkCreateDescriptorSetLayout")
}

fn pipeline_layout(
    device: &Device,
    set: vk::DescriptorSetLayout,
) -> Result<vk::PipelineLayout, Error> {
    let sets = [set];
    let info = vk::PipelineLayoutCreateInfo::default().set_layouts(&sets);
    unsafe { device.raw.create_pipeline_layout(&info, None) }.checked("vkCreatePipelineLayout")
}

fn binding(
    index: u32,
    kind: vk::DescriptorType,
    stages: vk::ShaderStageFlags,
) -> vk::DescriptorSetLayoutBinding<'static> {
    vk::DescriptorSetLayoutBinding::default()
        .binding(index)
        .descriptor_type(kind)
        .descriptor_count(1)
        .stage_flags(stages)
}

impl Layouts {
    pub fn new(device: &Arc<Device>) -> Result<Self, Error> {
        let raw = &device.raw;
        let sampler = unsafe {
            raw.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .max_lod(vk::LOD_CLAMP_NONE),
                None,
            )
        }
        .checked("vkCreateSampler")?;
        let samplers = [sampler];
        let drawing = vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT;
        let graphics_bindings = [
            binding(UNIFORM, vk::DescriptorType::UNIFORM_BUFFER, drawing),
            binding(
                SOURCE,
                vk::DescriptorType::SAMPLED_IMAGE,
                vk::ShaderStageFlags::FRAGMENT,
            ),
            binding(
                SECOND_SOURCE,
                vk::DescriptorType::SAMPLED_IMAGE,
                vk::ShaderStageFlags::FRAGMENT,
            ),
            binding(
                SAMPLER,
                vk::DescriptorType::SAMPLER,
                vk::ShaderStageFlags::FRAGMENT,
            )
            .immutable_samplers(&samplers),
        ];
        let compute = vk::ShaderStageFlags::COMPUTE;
        let compute_bindings = [
            binding(UNIFORM, vk::DescriptorType::UNIFORM_BUFFER, compute),
            binding(SOURCE, vk::DescriptorType::SAMPLED_IMAGE, compute),
            binding(OUTPUT, vk::DescriptorType::STORAGE_IMAGE, compute),
        ];
        let mut layouts = Self {
            device: Arc::clone(device),
            sampler,
            graphics_set: vk::DescriptorSetLayout::null(),
            compute_set: vk::DescriptorSetLayout::null(),
            graphics: vk::PipelineLayout::null(),
            compute: vk::PipelineLayout::null(),
        };
        layouts.graphics_set = set_layout(device, &graphics_bindings)?;
        layouts.compute_set = set_layout(device, &compute_bindings)?;
        layouts.graphics = pipeline_layout(device, layouts.graphics_set)?;
        layouts.compute = pipeline_layout(device, layouts.compute_set)?;
        Ok(layouts)
    }

    fn bind(
        &self,
        frame: &mut Frame<'_>,
        point: vk::PipelineBindPoint,
        writes: &mut [vk::WriteDescriptorSet<'_>],
    ) -> Result<(), Error> {
        let (layout, set_layout) = match point {
            vk::PipelineBindPoint::COMPUTE => (self.compute, self.compute_set),
            _ => (self.graphics, self.graphics_set),
        };
        match &self.device.push {
            Some(push) => {
                unsafe { push.cmd_push_descriptor_set(frame.command(), point, layout, 0, writes) };
                Ok(())
            }
            None => {
                let set = frame.descriptor_set(set_layout)?;
                writes.iter_mut().for_each(|write| write.dst_set = set);
                unsafe {
                    self.device.raw.update_descriptor_sets(writes, &[]);
                    self.device.raw.cmd_bind_descriptor_sets(
                        frame.command(),
                        point,
                        layout,
                        0,
                        &[set],
                        &[],
                    );
                }
                Ok(())
            }
        }
    }

    pub fn bind_draw(
        &self,
        frame: &mut Frame<'_>,
        uniform: &Uniform,
        sources: [vk::ImageView; 2],
    ) -> Result<(), Error> {
        let buffers = [vk::DescriptorBufferInfo::default()
            .buffer(frame.ring())
            .offset(uniform.offset)
            .range(uniform.range)];
        let images = sources.map(|view| {
            [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
        });
        let mut writes = [
            vk::WriteDescriptorSet::default()
                .dst_binding(UNIFORM)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffers),
            vk::WriteDescriptorSet::default()
                .dst_binding(SOURCE)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&images[0]),
            vk::WriteDescriptorSet::default()
                .dst_binding(SECOND_SOURCE)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&images[1]),
        ];
        self.bind(frame, vk::PipelineBindPoint::GRAPHICS, &mut writes)
    }

    pub fn bind_compute(
        &self,
        frame: &mut Frame<'_>,
        uniform: &Uniform,
        input: vk::ImageView,
        output: vk::ImageView,
    ) -> Result<(), Error> {
        let buffers = [vk::DescriptorBufferInfo::default()
            .buffer(frame.ring())
            .offset(uniform.offset)
            .range(uniform.range)];
        let inputs = [vk::DescriptorImageInfo::default()
            .image_view(input)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
        let outputs = [vk::DescriptorImageInfo::default()
            .image_view(output)
            .image_layout(vk::ImageLayout::GENERAL)];
        let mut writes = [
            vk::WriteDescriptorSet::default()
                .dst_binding(UNIFORM)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .buffer_info(&buffers),
            vk::WriteDescriptorSet::default()
                .dst_binding(SOURCE)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&inputs),
            vk::WriteDescriptorSet::default()
                .dst_binding(OUTPUT)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&outputs),
        ];
        self.bind(frame, vk::PipelineBindPoint::COMPUTE, &mut writes)
    }
}

impl Drop for Layouts {
    fn drop(&mut self) {
        let device = &self.device;
        [self.graphics, self.compute]
            .into_iter()
            .filter(|layout| *layout != vk::PipelineLayout::null())
            .for_each(|layout| device.retire(Garbage::PipelineLayout(layout)));
        [self.graphics_set, self.compute_set]
            .into_iter()
            .filter(|layout| *layout != vk::DescriptorSetLayout::null())
            .for_each(|layout| device.retire(Garbage::SetLayout(layout)));
        device.retire(Garbage::Sampler(self.sampler));
    }
}
