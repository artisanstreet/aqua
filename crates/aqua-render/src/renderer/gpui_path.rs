use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, LineCap, LineJoin, PathCommand};
use lyon::algorithms::measure::{PathMeasurements, SampleType};
use lyon::math::{Angle, point, vector};
use lyon::path::Path;
use lyon::path::builder::SvgPathBuilder;
use lyon::path::geom::ArcFlags;
use lyon::tessellation::{
    BuffersBuilder, FillOptions, FillTessellator, FillVertex, StrokeOptions, StrokeTessellator,
    StrokeVertex, VertexBuffers,
};

pub(super) fn tessellate(path: &GpuiPath, device_scale: f32) -> Result<Vec<[f32; 2]>, String> {
    let mut builder = Path::svg_builder();
    for command in &path.commands_local {
        match *command {
            PathCommand::Move(p) => {
                builder.move_to(point(p.x, p.y));
            }
            PathCommand::Line(p) => {
                builder.line_to(point(p.x, p.y));
            }
            PathCommand::Quadratic { control, to } => {
                builder.quadratic_bezier_to(point(control.x, control.y), point(to.x, to.y));
            }
            PathCommand::Cubic { first, second, to } => {
                builder.cubic_bezier_to(
                    point(first.x, first.y),
                    point(second.x, second.y),
                    point(to.x, to.y),
                );
            }
            PathCommand::Arc {
                radii,
                rotation_radians,
                large,
                sweep,
                to,
            } => builder.arc_to(
                vector(radii.x, radii.y),
                Angle::radians(rotation_radians),
                ArcFlags {
                    large_arc: large,
                    sweep,
                },
                point(to.x, to.y),
            ),
            PathCommand::Close => builder.close(),
        }
    }
    let source = builder.build();
    let mut buffers: VertexBuffers<lyon::math::Point, u16> = VertexBuffers::new();
    match &path.style {
        GpuiPathStyle::Fill { rule } => {
            let rule = match rule {
                FillRule::NonZero => lyon::tessellation::FillRule::NonZero,
                FillRule::EvenOdd => lyon::tessellation::FillRule::EvenOdd,
            };
            FillTessellator::new()
                .tessellate_path(
                    &source,
                    &FillOptions::default().with_fill_rule(rule),
                    &mut BuffersBuilder::new(&mut buffers, |vertex: FillVertex| vertex.position()),
                )
                .map_err(|error| error.to_string())?;
        }
        GpuiPathStyle::Stroke {
            width,
            cap,
            join,
            dash,
            dash_offset,
        } => {
            if !width.is_finite()
                || *width <= 0.0
                || !dash_offset.is_finite()
                || dash.iter().any(|value| !value.is_finite() || *value <= 0.0)
            {
                return Err("invalid GPUI path stroke".into());
            }
            let cap = match cap {
                LineCap::Butt => lyon::tessellation::LineCap::Butt,
                LineCap::Round => lyon::tessellation::LineCap::Round,
                LineCap::Square => lyon::tessellation::LineCap::Square,
            };
            let join = match join {
                LineJoin::Miter => lyon::tessellation::LineJoin::Miter,
                LineJoin::Round => lyon::tessellation::LineJoin::Round,
                LineJoin::Bevel => lyon::tessellation::LineJoin::Bevel,
            };
            let options = StrokeOptions::default()
                .with_line_width(*width)
                .with_start_cap(cap)
                .with_end_cap(cap)
                .with_line_join(join);
            let dashed;
            let stroke_path = if dash.is_empty() {
                &source
            } else {
                let mut pattern = dash.clone();
                if pattern.len() % 2 != 0 {
                    pattern.extend_from_slice(dash);
                }
                let period: f32 = pattern.iter().sum();
                let mut phase = dash_offset.rem_euclid(period);
                let mut index = 0;
                while phase >= pattern[index] {
                    phase -= pattern[index];
                    index = (index + 1) % pattern.len();
                }
                let measurements = PathMeasurements::from_path(&source, 0.01);
                let mut sampler = measurements.create_sampler(&source, SampleType::Normalized);
                let total = sampler.length();
                let mut output = Path::builder();
                let mut pos = 0.0;
                while pos < total {
                    let next = (pos + pattern[index] - phase).min(total);
                    if next <= pos {
                        return Err("invalid GPUI path dash progression".into());
                    }
                    if index % 2 == 0 {
                        sampler.split_range(pos / total..next / total, &mut output);
                    }
                    pos = next;
                    phase = 0.0;
                    index = (index + 1) % pattern.len();
                }
                dashed = output.build();
                &dashed
            };
            StrokeTessellator::new()
                .tessellate_path(
                    stroke_path,
                    &options,
                    &mut BuffersBuilder::new(&mut buffers, |vertex: StrokeVertex| {
                        vertex.position()
                    }),
                )
                .map_err(|error| error.to_string())?;
        }
    }
    Ok(buffers
        .indices
        .into_iter()
        .map(|index| {
            let point = buffers.vertices[index as usize];
            let point = path.device_position(aqua_core::Point::new(point.x, point.y), device_scale);
            [point.x, point.y]
        })
        .collect())
}

struct Pipelines {
    format: wgpu::TextureFormat,
    parameters: wgpu::BindGroupLayout,
    texture_layout: wgpu::BindGroupLayout,
    raster: wgpu::RenderPipeline,
    sprite: wgpu::RenderPipeline,
}

struct Targets {
    size: [u32; 2],
    format: wgpu::TextureFormat,
    resolved_view: wgpu::TextureView,
    multisampled_view: wgpu::TextureView,
    texture_bind: wgpu::BindGroup,
}

#[derive(Default)]
pub(super) struct Cache {
    pipelines: Option<Pipelines>,
    targets: Option<Targets>,
}

fn build_pipelines(renderer: &super::Renderer) -> Pipelines {
    let device = &renderer.device;
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aqua-gpui-path"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}",
                include_str!("../shaders/rect.wgsl"),
                include_str!("../shaders/gpui-path.wgsl")
            )
            .into(),
        ),
    });
    let parameters = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("aqua-gpui-path-parameters"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        }],
    });
    let texture_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("aqua-gpui-path-texture"),
        entries: &[wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::FRAGMENT,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: false },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
            count: None,
        }],
    });
    let raster_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aqua-gpui-path-raster-layout"),
        bind_group_layouts: &[Some(&renderer.rect_bind_layout), Some(&parameters)],
        immediate_size: 0,
    });
    let sprite_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aqua-gpui-path-layout"),
        bind_group_layouts: &[
            Some(&renderer.rect_bind_layout),
            Some(&parameters),
            Some(&texture_layout),
        ],
        immediate_size: 0,
    });
    let blend = |alpha_destination| {
        Some(wgpu::BlendState {
            color: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
                operation: wgpu::BlendOperation::Add,
            },
            alpha: wgpu::BlendComponent {
                src_factor: wgpu::BlendFactor::One,
                dst_factor: alpha_destination,
                operation: wgpu::BlendOperation::Add,
            },
        })
    };
    let attributes = wgpu::vertex_attr_array![0 => Float32x2];
    let raster = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aqua-gpui-path-raster"),
        layout: Some(&raster_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("path_vertex"),
            buffers: &[Some(wgpu::VertexBufferLayout {
                array_stride: 8,
                step_mode: wgpu::VertexStepMode::Vertex,
                attributes: &attributes,
            })],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("path_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.format,
                blend: blend(wgpu::BlendFactor::OneMinusSrcAlpha),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState {
            count: 4,
            ..Default::default()
        },
        multiview_mask: None,
        cache: None,
    });
    let sprite = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aqua-gpui-path-sprite"),
        layout: Some(&sprite_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("path_sprite_vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("path_sprite_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.format,
                blend: blend(wgpu::BlendFactor::One),
                write_mask: wgpu::ColorWrites::ALL,
            })],
            compilation_options: Default::default(),
        }),
        primitive: wgpu::PrimitiveState::default(),
        depth_stencil: None,
        multisample: wgpu::MultisampleState::default(),
        multiview_mask: None,
        cache: None,
    });
    Pipelines {
        format: renderer.format,
        parameters,
        texture_layout,
        raster,
        sprite,
    }
}

fn build_targets(renderer: &super::Renderer, pipelines: &Pipelines) -> Targets {
    let device = &renderer.device;
    let size = wgpu::Extent3d {
        width: renderer.width,
        height: renderer.height,
        depth_or_array_layers: 1,
    };
    let texture = |sample_count, usage| {
        device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-gpui-path-target"),
            size,
            mip_level_count: 1,
            sample_count,
            dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage,
            view_formats: &[],
        })
    };
    let resolved = texture(
        1,
        wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
    );
    let multisampled = texture(4, wgpu::TextureUsages::RENDER_ATTACHMENT);
    let resolved_view = resolved.create_view(&Default::default());
    let multisampled_view = multisampled.create_view(&Default::default());
    let texture_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("aqua-gpui-path-texture"),
        layout: &pipelines.texture_layout,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: wgpu::BindingResource::TextureView(&resolved_view),
        }],
    });
    Targets {
        size: [renderer.width, renderer.height],
        format: renderer.format,
        resolved_view,
        multisampled_view,
        texture_bind,
    }
}

pub(super) fn draw(
    renderer: &mut super::Renderer,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    path: &super::PathOp,
) {
    use wgpu::util::DeviceExt;
    let mut cache = std::mem::take(&mut renderer.gpui_path_cache);
    if cache
        .pipelines
        .as_ref()
        .is_none_or(|pipelines| pipelines.format != renderer.format)
    {
        cache.pipelines = Some(build_pipelines(renderer));
        cache.targets = None;
    }
    let pipelines = cache.pipelines.as_ref().expect("pipelines built");
    if cache.targets.as_ref().is_none_or(|targets| {
        targets.size != [renderer.width, renderer.height] || targets.format != renderer.format
    }) {
        cache.targets = Some(build_targets(renderer, pipelines));
    }
    let targets = cache.targets.as_ref().expect("targets built");
    let device = &renderer.device;
    let vertices = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("aqua-gpui-path-vertices"),
        contents: bytemuck::cast_slice(&path.vertices),
        usage: wgpu::BufferUsages::VERTEX,
    });
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("aqua-gpui-path-instance"),
        contents: bytemuck::bytes_of(&path.instance),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let parameter_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("aqua-gpui-path-parameters"),
        layout: &pipelines.parameters,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: uniform.as_entire_binding(),
        }],
    });
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aqua-gpui-path-raster"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &targets.multisampled_view,
                depth_slice: None,
                resolve_target: Some(&targets.resolved_view),
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Discard,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipelines.raster);
        pass.set_bind_group(0, &renderer.rect_uniform_bind, &[]);
        pass.set_bind_group(1, &parameter_bind, &[]);
        pass.set_vertex_buffer(0, vertices.slice(..));
        pass.draw(0..path.vertices.len() as u32, 0..1);
    }
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aqua-gpui-path-composite"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
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
        pass.set_pipeline(&pipelines.sprite);
        pass.set_bind_group(0, &renderer.rect_uniform_bind, &[]);
        pass.set_bind_group(1, &parameter_bind, &[]);
        pass.set_bind_group(2, &targets.texture_bind, &[]);
        pass.draw(0..6, 0..1);
    }
    renderer.gpui_path_cache = cache;
}
