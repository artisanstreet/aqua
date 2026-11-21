use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::ffi::CString;
use std::sync::Arc;

use ash::vk;

use super::binding::Layouts;
use super::Error;
use crate::hal::{Checked, Device, Garbage};
use crate::pack::{QUAD_STRIDE, SHADOW_STRIDE, SOLID_STRIDE};
use crate::sprite::SPRITE_STRIDE;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Kind {
    Quad,
    Shadow,
    Glyph,
    Slug,
    Image,
    ImageReplace,
    Layer,
    Copy,
    BlurDown,
    BlurUp,
    ColorMatrix,
    Solid,
    Srgba,
}

impl Kind {
    pub const ALL: [Kind; 13] = [
        Kind::Quad,
        Kind::Shadow,
        Kind::Glyph,
        Kind::Slug,
        Kind::Image,
        Kind::ImageReplace,
        Kind::Layer,
        Kind::Copy,
        Kind::BlurDown,
        Kind::BlurUp,
        Kind::ColorMatrix,
        Kind::Solid,
        Kind::Srgba,
    ];
}

const FIRST_FRAME: [Kind; 5] = [
    Kind::Quad,
    Kind::Shadow,
    Kind::Glyph,
    Kind::Slug,
    Kind::Image,
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Layout {
    None,
    Floats { count: u32, stride: usize },
    Solid,
    Srgba,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Blending {
    Opaque,
    Over,
    Dual,
}

struct Shaders {
    vertex: &'static str,
    pixel: &'static str,
    layout: Layout,
    blend: Blending,
}

fn shaders(kind: Kind) -> Shaders {
    let instanced = |vertex, pixel, layout| Shaders {
        vertex,
        pixel,
        layout,
        blend: Blending::Over,
    };
    let floats = |count, stride| Layout::Floats { count, stride };
    let fullscreen = |pixel, blend| Shaders {
        vertex: "vs_fullscreen",
        pixel,
        layout: Layout::None,
        blend,
    };
    match kind {
        Kind::Srgba => instanced("vs_srgba", "ps_solid", Layout::Srgba),
        Kind::Solid => instanced("vs_solid", "ps_solid", Layout::Solid),
        Kind::Quad => instanced("vs_quad", "ps_quad", floats(10, QUAD_STRIDE)),
        Kind::Shadow => instanced("vs_shadow", "ps_shadow", floats(5, SHADOW_STRIDE)),
        Kind::Glyph => instanced("vs_glyph", "ps_glyph", floats(4, SPRITE_STRIDE)),
        Kind::Slug => instanced("vs_slug", "ps_slug", floats(5, SPRITE_STRIDE)),
        Kind::Image => instanced("vs_image", "ps_image", floats(5, SPRITE_STRIDE)),
        Kind::ImageReplace => Shaders {
            blend: Blending::Dual,
            ..instanced("vs_image", "ps_image_replace", floats(5, SPRITE_STRIDE))
        },
        Kind::Layer => fullscreen("ps_layer", Blending::Over),
        Kind::Copy => fullscreen("ps_copy", Blending::Opaque),
        Kind::BlurDown => fullscreen("ps_blur_down", Blending::Opaque),
        Kind::BlurUp => fullscreen("ps_blur_up", Blending::Opaque),
        Kind::ColorMatrix => fullscreen("ps_color_matrix", Blending::Opaque),
    }
}

fn instance_binding(binding: u32, stride: usize) -> vk::VertexInputBindingDescription {
    vk::VertexInputBindingDescription {
        binding,
        stride: stride as u32,
        input_rate: vk::VertexInputRate::INSTANCE,
    }
}

fn attribute(
    location: u32,
    binding: u32,
    format: vk::Format,
    offset: u32,
) -> vk::VertexInputAttributeDescription {
    vk::VertexInputAttributeDescription {
        location,
        binding,
        format,
        offset,
    }
}

type VertexInput = (
    Vec<vk::VertexInputBindingDescription>,
    Vec<vk::VertexInputAttributeDescription>,
);

fn vertex_input(layout: Layout) -> VertexInput {
    let float4 = vk::Format::R32G32B32A32_SFLOAT;
    match layout {
        Layout::None => (Vec::new(), Vec::new()),
        Layout::Floats { count, stride } => (
            vec![instance_binding(0, stride)],
            (0..count)
                .map(|location| attribute(location, 0, float4, location * 16))
                .collect(),
        ),
        Layout::Solid => (
            vec![instance_binding(0, SOLID_STRIDE)],
            vec![
                attribute(0, 0, float4, 0),
                attribute(1, 0, float4, 16),
                attribute(2, 0, vk::Format::R32_UINT, 32),
            ],
        ),
        Layout::Srgba => (
            vec![instance_binding(0, 16), instance_binding(1, 4)],
            vec![
                attribute(0, 0, float4, 0),
                attribute(1, 1, vk::Format::R8G8B8A8_UNORM, 0),
            ],
        ),
    }
}

fn blend_attachment(blending: Blending) -> vk::PipelineColorBlendAttachmentState {
    let (destination, destination_alpha) = match blending {
        Blending::Dual => (
            vk::BlendFactor::ONE_MINUS_SRC1_COLOR,
            vk::BlendFactor::ONE_MINUS_SRC1_ALPHA,
        ),
        Blending::Opaque | Blending::Over => (
            vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
            vk::BlendFactor::ONE_MINUS_SRC_ALPHA,
        ),
    };
    vk::PipelineColorBlendAttachmentState {
        blend_enable: (blending != Blending::Opaque).into(),
        src_color_blend_factor: vk::BlendFactor::ONE,
        dst_color_blend_factor: destination,
        color_blend_op: vk::BlendOp::ADD,
        src_alpha_blend_factor: vk::BlendFactor::ONE,
        dst_alpha_blend_factor: destination_alpha,
        alpha_blend_op: vk::BlendOp::ADD,
        color_write_mask: vk::ColorComponentFlags::RGBA,
    }
}

fn words(name: &'static str) -> Result<Vec<u32>, Error> {
    let blob = aqua_shaders::find_spirv(name).ok_or(Error::MissingShader(name))?;
    let words = blob
        .bytes
        .as_chunks::<4>()
        .0
        .iter()
        .map(|chunk| u32::from_le_bytes(*chunk))
        .collect::<Vec<_>>();
    assert!(
        blob.bytes.len() % 4 == 0 && !words.is_empty(),
        "SPIR-V blob {name} is not a whole number of words"
    );
    Ok(words)
}

const CACHE_HEADER: usize = 16 + vk::UUID_SIZE;

fn compatible_cache<'a>(device: &Device, library: &'a [u8]) -> &'a [u8] {
    let info = device.info();
    let word = |at: usize| {
        library
            .get(at..at + 4)
            .map(|bytes| u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    };
    let matches = library.len() >= CACHE_HEADER
        && word(4) == Some(vk::PipelineCacheHeaderVersion::ONE.as_raw() as u32)
        && word(8) == Some(info.vendor_id)
        && word(12) == Some(info.device_id)
        && library[16..CACHE_HEADER] == info.cache_uuid;
    match matches {
        true => library,
        false => &[],
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct PipelineStats {
    pub compiled: u32,
    pub late: u32,
}

#[derive(Clone, Copy, Default)]
struct Usage {
    used: bool,
    first_frame: bool,
}

pub struct Pipelines {
    device: Arc<Device>,
    cache: vk::PipelineCache,
    modules: HashMap<&'static str, vk::ShaderModule>,
    built: BTreeMap<(Kind, i32), vk::Pipeline>,
    compute: Option<vk::Pipeline>,
    usage: BTreeMap<Kind, Usage>,
    stats: PipelineStats,
    warmed: bool,
    startup_open: bool,
    late: Vec<Kind>,
    hint: Option<BTreeSet<String>>,
}

fn label(kind: Kind) -> String {
    format!("{kind:?}")
}

fn first_frame_label(kind: Kind) -> String {
    format!("+{kind:?}")
}

fn is_own_label(name: &str) -> bool {
    let bare = name.strip_prefix('+').unwrap_or(name);
    Kind::ALL.iter().any(|kind| label(*kind) == bare)
}

impl Pipelines {
    pub fn new(device: &Arc<Device>, cache: Option<&[u8]>) -> Result<Self, Error> {
        let (hint, library) = super::hint::decode(cache);
        let initial = compatible_cache(device, library.unwrap_or_default());
        let info = vk::PipelineCacheCreateInfo::default().initial_data(initial);
        let cache = unsafe { device.raw.create_pipeline_cache(&info, None) }
            .checked("vkCreatePipelineCache")?;
        Ok(Self {
            device: Arc::clone(device),
            cache,
            modules: HashMap::new(),
            built: BTreeMap::new(),
            compute: None,
            usage: BTreeMap::new(),
            stats: PipelineStats::default(),
            warmed: false,
            startup_open: true,
            late: Vec::new(),
            hint: hint.filter(|names| names.iter().all(|name| is_own_label(name))),
        })
    }

    fn module(&mut self, name: &'static str) -> Result<vk::ShaderModule, Error> {
        if let Some(module) = self.modules.get(name) {
            return Ok(*module);
        }
        let code = words(name)?;
        let info = vk::ShaderModuleCreateInfo::default().code(&code);
        let module = unsafe { self.device.raw.create_shader_module(&info, None) }
            .checked("vkCreateShaderModule")?;
        self.modules.insert(name, module);
        Ok(module)
    }

    fn build(
        &mut self,
        kind: Kind,
        format: vk::Format,
        layouts: &Layouts,
    ) -> Result<vk::Pipeline, Error> {
        let shaders = shaders(kind);
        let vertex = self.module(shaders.vertex)?;
        let pixel = self.module(shaders.pixel)?;
        let vertex_entry = CString::new(shaders.vertex).expect("entry names have no NUL");
        let pixel_entry = CString::new(shaders.pixel).expect("entry names have no NUL");
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vertex)
                .name(&vertex_entry),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(pixel)
                .name(&pixel_entry),
        ];
        let (bindings, attributes) = vertex_input(shaders.layout);
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default()
            .vertex_binding_descriptions(&bindings)
            .vertex_attribute_descriptions(&attributes);
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default()
            .topology(vk::PrimitiveTopology::TRIANGLE_STRIP);
        let viewport = vk::PipelineViewportStateCreateInfo::default()
            .viewport_count(1)
            .scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::CLOCKWISE)
            .line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default()
            .rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let attachments = [blend_attachment(shaders.blend)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachments);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let formats = [format];
        let mut rendering =
            vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layouts.graphics)
            .push_next(&mut rendering);
        let pipeline = unsafe {
            self.device
                .raw
                .create_graphics_pipelines(self.cache, &[info], None)
        }
        .map_err(|(_, result)| result)
        .checked("vkCreateGraphicsPipelines")?[0];
        self.stats.compiled += 1;
        Ok(pipeline)
    }

    pub fn get(
        &mut self,
        kind: Kind,
        format: vk::Format,
        layouts: &Layouts,
    ) -> Result<vk::Pipeline, Error> {
        let pipeline = match self.built.get(&(kind, format.as_raw())) {
            Some(pipeline) => *pipeline,
            None => {
                let pipeline = self.build(kind, format, layouts)?;
                if self.warmed {
                    self.stats.late += 1;
                    self.late.push(kind);
                }
                self.built.insert((kind, format.as_raw()), pipeline);
                pipeline
            }
        };
        let startup_open = self.startup_open;
        let usage = self.usage.entry(kind).or_default();
        usage.used = true;
        usage.first_frame |= startup_open;
        Ok(pipeline)
    }

    pub fn compute(&mut self, layouts: &Layouts) -> Result<vk::Pipeline, Error> {
        if let Some(pipeline) = self.compute {
            return Ok(pipeline);
        }
        let module = self.module("cs_row_box_blur")?;
        let entry = c"cs_row_box_blur";
        let stage = vk::PipelineShaderStageCreateInfo::default()
            .stage(vk::ShaderStageFlags::COMPUTE)
            .module(module)
            .name(entry);
        let info = vk::ComputePipelineCreateInfo::default()
            .stage(stage)
            .layout(layouts.compute);
        let pipeline = unsafe {
            self.device
                .raw
                .create_compute_pipelines(self.cache, &[info], None)
        }
        .map_err(|(_, result)| result)
        .checked("vkCreateComputePipelines")?[0];
        self.stats.compiled += 1;
        self.compute = Some(pipeline);
        Ok(pipeline)
    }

    pub fn warm(&mut self, format: vk::Format, layouts: &Layouts) -> Result<(), Error> {
        let wanted: Vec<Kind> = Kind::ALL
            .into_iter()
            .filter(|kind| match &self.hint {
                None => FIRST_FRAME.contains(kind),
                Some(hint) => {
                    hint.contains(&label(*kind)) || hint.contains(&first_frame_label(*kind))
                }
            })
            .filter(|kind| !self.built.contains_key(&(*kind, format.as_raw())))
            .collect();
        for kind in wanted {
            let pipeline = self.build(kind, format, layouts)?;
            self.built.insert((kind, format.as_raw()), pipeline);
        }
        self.warmed = true;
        Ok(())
    }

    pub fn close_startup(&mut self) {
        self.startup_open = false;
    }

    pub fn late(&self) -> &[Kind] {
        &self.late
    }

    pub fn stats(&self) -> PipelineStats {
        self.stats
    }

    fn used_labels(&self) -> BTreeSet<String> {
        let mut merged: BTreeMap<String, bool> = BTreeMap::new();
        let mut note = |name: String, first: bool| {
            *merged.entry(name).or_insert(false) |= first;
        };
        self.hint
            .iter()
            .flatten()
            .for_each(|name| match name.strip_prefix('+') {
                Some(bare) => note(bare.to_owned(), true),
                None => note(name.clone(), false),
            });
        self.usage
            .iter()
            .filter(|(_, usage)| usage.used)
            .for_each(|(kind, usage)| note(label(*kind), usage.first_frame));
        merged
            .into_iter()
            .map(|(name, first)| match first {
                true => format!("+{name}"),
                false => name,
            })
            .collect()
    }

    pub fn serialize(&self) -> Result<Vec<u8>, Error> {
        let library = unsafe { self.device.raw.get_pipeline_cache_data(self.cache) }
            .checked("vkGetPipelineCacheData")?;
        Ok(super::hint::encode(&self.used_labels(), &library))
    }
}

impl Drop for Pipelines {
    fn drop(&mut self) {
        let device = &self.device;
        self.built
            .values()
            .chain(self.compute.iter())
            .for_each(|pipeline| device.retire(Garbage::Pipeline(*pipeline)));
        self.modules
            .values()
            .for_each(|module| device.retire(Garbage::ShaderModule(*module)));
        device.retire(Garbage::PipelineCache(self.cache));
    }
}
