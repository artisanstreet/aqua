use std::sync::Arc;

use ash::vk;

use super::frames::Frame;
use crate::hal::{Binding, Checked, Device, Error, Garbage};

pub const UNIFORM: u32 = 0;
pub const SOURCE: u32 = 16;
pub const SECOND_SOURCE: u32 = 17;
pub const SAMPLER: u32 = 32;
pub const OUTPUT: u32 = 48;
pub const CUSTOM_INPUTS: usize = 8;

#[derive(Clone, Copy)]
struct Pair {
    set: vk::DescriptorSetLayout,
    pipeline: vk::PipelineLayout,
}

impl Pair {
    const NULL: Pair = Pair {
        set: vk::DescriptorSetLayout::null(),
        pipeline: vk::PipelineLayout::null(),
    };
}

pub struct Layouts {
    device: Arc<Device>,
    sampler: vk::Sampler,
    pairs: [Pair; 4],
}

#[derive(Clone, Copy)]
enum Which {
    Graphics = 0,
    Compute = 1,
    Custom = 2,
    CustomCompute = 3,
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

fn pair(device: &Device, bindings: &[vk::DescriptorSetLayoutBinding<'_>]) -> Result<Pair, Error> {
    let set = set_layout(device, bindings)?;
    match pipeline_layout(device, set) {
        Ok(pipeline) => Ok(Pair { set, pipeline }),
        Err(error) => {
            device.retire(Garbage::SetLayout(set));
            Err(error)
        }
    }
}

fn custom_inputs(
    stages: vk::ShaderStageFlags,
) -> [vk::DescriptorSetLayoutBinding<'static>; CUSTOM_INPUTS] {
    std::array::from_fn(|slot| {
        binding(
            SOURCE + slot as u32,
            vk::DescriptorType::SAMPLED_IMAGE,
            stages,
        )
    })
}

fn image_infos(
    views: &[vk::ImageView; CUSTOM_INPUTS],
) -> [[vk::DescriptorImageInfo; 1]; CUSTOM_INPUTS] {
    views.map(|view| {
        [vk::DescriptorImageInfo::default()
            .image_view(view)
            .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)]
    })
}

fn input_writes(
    infos: &[[vk::DescriptorImageInfo; 1]; CUSTOM_INPUTS],
) -> impl Iterator<Item = vk::WriteDescriptorSet<'_>> {
    infos.iter().enumerate().map(|(slot, info)| {
        vk::WriteDescriptorSet::default()
            .dst_binding(SOURCE + slot as u32)
            .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
            .image_info(info)
    })
}

fn uniform_write<'a>(buffers: &'a [vk::DescriptorBufferInfo; 1]) -> vk::WriteDescriptorSet<'a> {
    vk::WriteDescriptorSet::default()
        .dst_binding(UNIFORM)
        .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
        .buffer_info(buffers)
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
        let fragment = vk::ShaderStageFlags::FRAGMENT;
        let custom_bindings: Vec<_> = [binding(
            UNIFORM,
            vk::DescriptorType::UNIFORM_BUFFER,
            fragment,
        )]
        .into_iter()
        .chain(custom_inputs(fragment))
        .chain([
            binding(SAMPLER, vk::DescriptorType::SAMPLER, fragment).immutable_samplers(&samplers)
        ])
        .collect();
        let custom_compute_bindings: Vec<_> = [binding(
            UNIFORM,
            vk::DescriptorType::UNIFORM_BUFFER,
            compute,
        )]
        .into_iter()
        .chain(custom_inputs(compute))
        .chain([binding(OUTPUT, vk::DescriptorType::STORAGE_IMAGE, compute)])
        .collect();
        let mut layouts = Self {
            device: Arc::clone(device),
            sampler,
            pairs: [Pair::NULL; 4],
        };
        let all: [&[vk::DescriptorSetLayoutBinding<'_>]; 4] = [
            &graphics_bindings,
            &compute_bindings,
            &custom_bindings,
            &custom_compute_bindings,
        ];
        for (slot, bindings) in all.into_iter().enumerate() {
            layouts.pairs[slot] = pair(device, bindings)?;
        }
        Ok(layouts)
    }

    fn pair(&self, which: Which) -> Pair {
        self.pairs[which as usize]
    }

    pub fn graphics(&self) -> vk::PipelineLayout {
        self.pair(Which::Graphics).pipeline
    }

    pub fn compute(&self) -> vk::PipelineLayout {
        self.pair(Which::Compute).pipeline
    }

    pub fn custom(&self) -> vk::PipelineLayout {
        self.pair(Which::Custom).pipeline
    }

    pub fn custom_compute(&self) -> vk::PipelineLayout {
        self.pair(Which::CustomCompute).pipeline
    }

    fn bind(
        &self,
        frame: &mut Frame<'_>,
        which: Which,
        writes: &mut [vk::WriteDescriptorSet<'_>],
    ) -> Result<(), Error> {
        let point = match which {
            Which::Compute | Which::CustomCompute => vk::PipelineBindPoint::COMPUTE,
            Which::Graphics | Which::Custom => vk::PipelineBindPoint::GRAPHICS,
        };
        let Pair {
            set: set_layout,
            pipeline: layout,
        } = self.pair(which);
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
        self.bind(frame, Which::Graphics, &mut writes)
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
        self.bind(frame, Which::Compute, &mut writes)
    }

    pub fn bind_custom(
        &self,
        frame: &mut Frame<'_>,
        uniform: &Uniform,
        inputs: &[vk::ImageView; CUSTOM_INPUTS],
    ) -> Result<(), Error> {
        let buffers = [vk::DescriptorBufferInfo::default()
            .buffer(frame.ring())
            .offset(uniform.offset)
            .range(uniform.range)];
        let infos = image_infos(inputs);
        let mut writes: Vec<_> = [uniform_write(&buffers)]
            .into_iter()
            .chain(input_writes(&infos))
            .collect();
        self.bind(frame, Which::Custom, &mut writes)
    }

    pub fn bind_custom_compute(
        &self,
        frame: &mut Frame<'_>,
        uniform: &Uniform,
        inputs: &[vk::ImageView; CUSTOM_INPUTS],
        output: vk::ImageView,
    ) -> Result<(), Error> {
        let buffers = [vk::DescriptorBufferInfo::default()
            .buffer(frame.ring())
            .offset(uniform.offset)
            .range(uniform.range)];
        let infos = image_infos(inputs);
        let outputs = [vk::DescriptorImageInfo::default()
            .image_view(output)
            .image_layout(vk::ImageLayout::GENERAL)];
        let mut writes: Vec<_> = [uniform_write(&buffers)]
            .into_iter()
            .chain(input_writes(&infos))
            .chain([vk::WriteDescriptorSet::default()
                .dst_binding(OUTPUT)
                .descriptor_type(vk::DescriptorType::STORAGE_IMAGE)
                .image_info(&outputs)])
            .collect();
        self.bind(frame, Which::CustomCompute, &mut writes)
    }
}

impl Drop for Layouts {
    fn drop(&mut self) {
        let device = &self.device;
        self.pairs
            .iter()
            .map(|pair| pair.pipeline)
            .filter(|layout| *layout != vk::PipelineLayout::null())
            .for_each(|layout| device.retire(Garbage::PipelineLayout(layout)));
        self.pairs
            .iter()
            .map(|pair| pair.set)
            .filter(|layout| *layout != vk::DescriptorSetLayout::null())
            .for_each(|layout| device.retire(Garbage::SetLayout(layout)));
        device.retire(Garbage::Sampler(self.sampler));
    }
}
