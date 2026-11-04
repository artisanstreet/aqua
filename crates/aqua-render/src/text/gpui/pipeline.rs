use bytemuck::{Pod, Zeroable};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct GlyphInstance {
    pub bounds: [f32; 4],
    pub uv_bounds: [f32; 4],
    pub hsla: [f32; 4],
}

pub(crate) struct Pipelines {
    pub mask_layout: wgpu::BindGroupLayout,
    pub viewport_layout: wgpu::BindGroupLayout,
    pub grayscale: wgpu::RenderPipeline,
    pub polychrome: wgpu::RenderPipeline,
    pub subpixel: wgpu::RenderPipeline,
}

impl Pipelines {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Result<Self, String> {
        if !device
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING)
        {
            return Err("GPUI subpixel text requires dual-source blending".into());
        }
        if !device.features().contains(wgpu::Features::CLIP_DISTANCES) {
            return Err("GPUI glyph clipping requires clip distances".into());
        }
        if !matches!(
            format,
            wgpu::TextureFormat::Bgra8Unorm | wgpu::TextureFormat::Rgba8Unorm
        ) {
            return Err("GPUI encoded glyphs require an 8-bit UNORM render target".into());
        }
        let mask_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpui-glyph-mask-layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let viewport_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpui-glyph-viewport-layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::VERTEX,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpui-glyph-pipeline-layout"),
            bind_group_layouts: &[Some(&mask_layout), Some(&viewport_layout)],
            immediate_size: 0,
        });
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpui-glyph-mask"),
            source: wgpu::ShaderSource::Wgsl(super::MASK_SHADER.into()),
        });
        let make = |entry: &str, color: wgpu::BlendComponent, alpha: wgpu::BlendComponent| {
            let attributes =
                wgpu::vertex_attr_array![0 => Float32x4, 1 => Float32x4, 2 => Float32x4];
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("gpui-glyph-pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vertex"),
                    buffers: &[Some(wgpu::VertexBufferLayout {
                        array_stride: std::mem::size_of::<GlyphInstance>() as u64,
                        step_mode: wgpu::VertexStepMode::Instance,
                        attributes: &attributes,
                    })],
                    compilation_options: Default::default(),
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some(entry),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState { color, alpha }),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                    compilation_options: Default::default(),
                }),
                primitive: wgpu::PrimitiveState {
                    topology: wgpu::PrimitiveTopology::TriangleStrip,
                    ..Default::default()
                },
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };
        let blend = |src_factor, dst_factor| wgpu::BlendComponent {
            src_factor,
            dst_factor,
            operation: wgpu::BlendOperation::Add,
        };
        let grayscale = make(
            "grayscale",
            blend(
                wgpu::BlendFactor::SrcAlpha,
                wgpu::BlendFactor::OneMinusSrcAlpha,
            ),
            blend(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrcAlpha),
        );
        let polychrome = make(
            "polychrome",
            blend(
                wgpu::BlendFactor::SrcAlpha,
                wgpu::BlendFactor::OneMinusSrcAlpha,
            ),
            blend(wgpu::BlendFactor::One, wgpu::BlendFactor::OneMinusSrcAlpha),
        );
        let subpixel = make(
            "subpixel",
            blend(wgpu::BlendFactor::Src1, wgpu::BlendFactor::OneMinusSrc1),
            blend(
                wgpu::BlendFactor::Src1Alpha,
                wgpu::BlendFactor::OneMinusSrc1Alpha,
            ),
        );
        Ok(Self {
            mask_layout,
            viewport_layout,
            grayscale,
            polychrome,
            subpixel,
        })
    }
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Uniforms {
    gamma: [f32; 4],
    grayscale_contrast: f32,
    subpixel_contrast: f32,
    bgr: u32,
    unused: u32,
}

pub(crate) struct MaskDraw<'a> {
    pub device: &'a wgpu::Device,
    pub encoder: &'a mut wgpu::CommandEncoder,
    pub target: &'a wgpu::TextureView,
    pub atlas: &'a wgpu::TextureView,
    pub size: [u32; 2],
    pub clip: [u32; 4],
    pub params: super::RenderingParameters,
    pub instances: &'a [GlyphInstance],
    pub subpixel: bool,
}

impl Pipelines {
    pub fn encode(&self, draw: MaskDraw<'_>) -> Result<(), String> {
        self.encode_impl(draw, false)
    }

    pub fn encode_color(&self, draw: MaskDraw<'_>) -> Result<(), String> {
        self.encode_impl(draw, true)
    }

    fn encode_impl(&self, draw: MaskDraw<'_>, color: bool) -> Result<(), String> {
        use wgpu::util::DeviceExt;
        draw.params.validate().map_err(str::to_owned)?;
        if draw.instances.is_empty() || draw.clip[2] == 0 || draw.clip[3] == 0 {
            return Ok(());
        }
        if draw.size.contains(&0)
            || draw.clip[0].saturating_add(draw.clip[2]) > draw.size[0]
            || draw.clip[1].saturating_add(draw.clip[3]) > draw.size[1]
        {
            return Err("invalid glyph viewport or clip".into());
        }
        let uniforms = Uniforms {
            gamma: draw.params.gamma_ratios,
            grayscale_contrast: draw.params.grayscale_enhanced_contrast,
            subpixel_contrast: draw.params.subpixel_enhanced_contrast,
            bgr: u32::from(draw.params.is_bgr),
            unused: 0,
        };
        let uniform = draw
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpui-mask-parameters"),
                contents: bytemuck::bytes_of(&uniforms),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let viewport = [
            draw.size[0] as f32,
            draw.size[1] as f32,
            0.0,
            0.0,
            draw.clip[0] as f32,
            draw.clip[1] as f32,
            draw.clip[2] as f32,
            draw.clip[3] as f32,
        ];
        let viewport_buffer = draw
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpui-mask-viewport"),
                contents: bytemuck::cast_slice(&viewport),
                usage: wgpu::BufferUsages::UNIFORM,
            });
        let instances = draw
            .device
            .create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpui-mask-instances"),
                contents: bytemuck::cast_slice(draw.instances),
                usage: wgpu::BufferUsages::VERTEX,
            });
        let sampler = draw.device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gpui-mask-sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        let mask_bind = draw.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpui-mask-bind"),
            layout: &self.mask_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(draw.atlas),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: uniform.as_entire_binding(),
                },
            ],
        });
        let viewport_bind = draw.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpui-mask-viewport-bind"),
            layout: &self.viewport_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: viewport_buffer.as_entire_binding(),
            }],
        });
        let mut pass = draw.encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpui-mask-draw"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: draw.target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(if color {
            &self.polychrome
        } else if draw.subpixel {
            &self.subpixel
        } else {
            &self.grayscale
        });
        pass.set_bind_group(0, &mask_bind, &[]);
        pass.set_bind_group(1, &viewport_bind, &[]);
        pass.set_vertex_buffer(0, instances.slice(..));
        pass.set_scissor_rect(draw.clip[0], draw.clip[1], draw.clip[2], draw.clip[3]);
        pass.draw(0..4, 0..draw.instances.len() as u32);
        Ok(())
    }
}
