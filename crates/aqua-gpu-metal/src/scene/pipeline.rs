use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use objc2::rc::Retained;
use objc2::runtime::ProtocolObject;
use objc2_foundation::NSString;
use objc2_metal::{
    MTLBlendFactor, MTLBlendOperation, MTLColorWriteMask, MTLComputePipelineState, MTLDevice,
    MTLPixelFormat, MTLRenderPipelineDescriptor, MTLRenderPipelineState, MTLVertexDescriptor,
    MTLVertexFormat, MTLVertexStepFunction,
};

use super::map::{INSTANCES, SECOND_INSTANCES};
use super::Error;
use crate::hal::{describe, Device, Error as DeviceError};
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
    Floats { count: usize, stride: usize },
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

struct Attribute {
    location: usize,
    stream: u32,
    format: MTLVertexFormat,
    offset: usize,
}

fn attributes(layout: Layout) -> (Vec<Attribute>, Vec<(u32, usize)>) {
    let float4 = |location: usize, stream, offset| Attribute {
        location,
        stream,
        format: MTLVertexFormat::Float4,
        offset,
    };
    match layout {
        Layout::None => (Vec::new(), Vec::new()),
        Layout::Floats { count, stride } => (
            (0..count)
                .map(|location| float4(location, INSTANCES, location * 16))
                .collect(),
            vec![(INSTANCES, stride)],
        ),
        Layout::Solid => (
            vec![
                float4(0, INSTANCES, 0),
                float4(1, INSTANCES, 16),
                Attribute {
                    location: 2,
                    stream: INSTANCES,
                    format: MTLVertexFormat::UInt,
                    offset: 32,
                },
            ],
            vec![(INSTANCES, SOLID_STRIDE)],
        ),
        Layout::Srgba => (
            vec![
                float4(0, INSTANCES, 0),
                Attribute {
                    location: 1,
                    stream: SECOND_INSTANCES,
                    format: MTLVertexFormat::UChar4Normalized,
                    offset: 0,
                },
            ],
            vec![(INSTANCES, 16), (SECOND_INSTANCES, 4)],
        ),
    }
}

fn vertex_descriptor(layout: Layout) -> Option<Retained<MTLVertexDescriptor>> {
    let (attributes, streams) = attributes(layout);
    if attributes.is_empty() {
        return None;
    }
    let descriptor = MTLVertexDescriptor::vertexDescriptor();
    for attribute in attributes {
        let slot = unsafe {
            descriptor
                .attributes()
                .objectAtIndexedSubscript(attribute.location)
        };
        slot.setFormat(attribute.format);
        unsafe {
            slot.setOffset(attribute.offset);
            slot.setBufferIndex(attribute.stream as usize);
        }
    }
    for (stream, stride) in streams {
        let layout = unsafe {
            descriptor
                .layouts()
                .objectAtIndexedSubscript(stream as usize)
        };
        layout.setStepFunction(MTLVertexStepFunction::PerInstance);
        unsafe {
            layout.setStride(stride);
            layout.setStepRate(1);
        }
    }
    Some(descriptor)
}

fn blend_factors(blending: Blending) -> (MTLBlendFactor, MTLBlendFactor) {
    match blending {
        Blending::Dual => (
            MTLBlendFactor::OneMinusSource1Color,
            MTLBlendFactor::OneMinusSource1Alpha,
        ),
        Blending::Opaque | Blending::Over => (
            MTLBlendFactor::OneMinusSourceAlpha,
            MTLBlendFactor::OneMinusSourceAlpha,
        ),
    }
}

pub type RenderPipeline = Retained<ProtocolObject<dyn MTLRenderPipelineState>>;
pub type ComputePipeline = Retained<ProtocolObject<dyn MTLComputePipelineState>>;

fn render_pipeline(
    device: &Device,
    kind: Kind,
    format: MTLPixelFormat,
) -> Result<RenderPipeline, Error> {
    let shaders = shaders(kind);
    let descriptor = MTLRenderPipelineDescriptor::new();
    descriptor.setLabel(Some(&NSString::from_str(&label(kind))));
    let vertex = device.function(shaders.vertex)?;
    let pixel = device.function(shaders.pixel)?;
    descriptor.setVertexFunction(Some(&vertex));
    descriptor.setFragmentFunction(Some(&pixel));
    descriptor.setVertexDescriptor(vertex_descriptor(shaders.layout).as_deref());
    let attachment = unsafe { descriptor.colorAttachments().objectAtIndexedSubscript(0) };
    attachment.setPixelFormat(format);
    attachment.setWriteMask(MTLColorWriteMask::All);
    attachment.setBlendingEnabled(shaders.blend != Blending::Opaque);
    let (destination, destination_alpha) = blend_factors(shaders.blend);
    attachment.setSourceRGBBlendFactor(MTLBlendFactor::One);
    attachment.setDestinationRGBBlendFactor(destination);
    attachment.setRgbBlendOperation(MTLBlendOperation::Add);
    attachment.setSourceAlphaBlendFactor(MTLBlendFactor::One);
    attachment.setDestinationAlphaBlendFactor(destination_alpha);
    attachment.setAlphaBlendOperation(MTLBlendOperation::Add);
    device
        .raw
        .newRenderPipelineStateWithDescriptor_error(&descriptor)
        .map_err(|error| {
            Error::Device(DeviceError::Pipeline {
                pipeline: format!("{kind:?} for pixel format {}", format.0),
                reason: describe(&error),
            })
        })
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
    built: BTreeMap<(Kind, usize), RenderPipeline>,
    compute: Option<ComputePipeline>,
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
    pub fn new(device: &Arc<Device>, cache: Option<&[u8]>) -> Self {
        let (hint, _library) = super::hint::decode(cache);
        Self {
            device: Arc::clone(device),
            built: BTreeMap::new(),
            compute: None,
            usage: BTreeMap::new(),
            stats: PipelineStats::default(),
            warmed: false,
            startup_open: true,
            late: Vec::new(),
            hint: hint.filter(|names| names.iter().all(|name| is_own_label(name))),
        }
    }

    fn build(&mut self, kind: Kind, format: MTLPixelFormat) -> Result<RenderPipeline, Error> {
        let pipeline = render_pipeline(&self.device, kind, format)?;
        self.stats.compiled += 1;
        Ok(pipeline)
    }

    pub fn get(&mut self, kind: Kind, format: MTLPixelFormat) -> Result<RenderPipeline, Error> {
        let pipeline = match self.built.get(&(kind, format.0)) {
            Some(pipeline) => pipeline.clone(),
            None => {
                let pipeline = self.build(kind, format)?;
                if self.warmed {
                    self.stats.late += 1;
                    self.late.push(kind);
                }
                self.built.insert((kind, format.0), pipeline.clone());
                pipeline
            }
        };
        let startup_open = self.startup_open;
        let usage = self.usage.entry(kind).or_default();
        usage.used = true;
        usage.first_frame |= startup_open;
        Ok(pipeline)
    }

    pub fn compute(&mut self) -> Result<ComputePipeline, Error> {
        if let Some(pipeline) = &self.compute {
            return Ok(pipeline.clone());
        }
        let function = self.device.function("cs_row_box_blur")?;
        let pipeline = self
            .device
            .raw
            .newComputePipelineStateWithFunction_error(&function)
            .map_err(|error| {
                Error::Device(DeviceError::Pipeline {
                    pipeline: "cs_row_box_blur".to_owned(),
                    reason: describe(&error),
                })
            })?;
        self.stats.compiled += 1;
        self.compute = Some(pipeline.clone());
        Ok(pipeline)
    }

    pub fn warm(&mut self, format: MTLPixelFormat) -> Result<(), Error> {
        let wanted: Vec<Kind> = Kind::ALL
            .into_iter()
            .filter(|kind| match &self.hint {
                None => FIRST_FRAME.contains(kind),
                Some(hint) => {
                    hint.contains(&label(*kind)) || hint.contains(&first_frame_label(*kind))
                }
            })
            .filter(|kind| !self.built.contains_key(&(*kind, format.0)))
            .collect();
        for kind in wanted {
            let pipeline = self.build(kind, format)?;
            self.built.insert((kind, format.0), pipeline);
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

    pub fn serialize(&self) -> Vec<u8> {
        super::hint::encode(&self.used_labels(), &[])
    }
}
