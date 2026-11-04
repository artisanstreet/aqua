use super::{Rect, RenderError, Renderer, SvgOp};
use crate::text::gpui::RenderingParameters;
use crate::vector::VectorError;
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Parameters {
    bounds: [f32; 4],
    hsla: [f32; 4],
    gamma: [f32; 4],
    misc: [f32; 4],
    matrix: [f32; 4],
    center_translation: [f32; 4],
}

pub(super) fn draw(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    svg: &SvgOp,
) -> Result<(), RenderError> {
    let doc = roxmltree::Document::parse(&svg.source)
        .map_err(|error| RenderError::Vector(VectorError::InvalidSvg(error.to_string())))?;
    if doc
        .descendants()
        .flat_map(|node| node.attributes())
        .any(|attribute| {
            attribute.name() == "href"
                && !attribute.value().starts_with('#')
                && !attribute.value().starts_with("data:")
        })
    {
        return Err(RenderError::Vector(VectorError::ExternalResource));
    }
    let tree = resvg::usvg::Tree::from_str(&svg.source, &resvg::usvg::Options::default())
        .map_err(|error| RenderError::Vector(VectorError::InvalidSvg(error.to_string())))?;
    let snap = aqua_core::geometry::snap_device_pixel;
    let scale = renderer.scale;
    let original = svg.untransformed_bounds;
    if original.w <= 0.0 || original.h <= 0.0 || svg.element_scale <= 0.0 {
        return Ok(());
    }
    let (texture_width, texture_height, pixels, destination, format) = if svg.tint.is_some() {
        let width = (snap(original.right(), scale) - snap(original.x, scale)).max(0.0);
        let mut raster_width = (width * 2.0).ceil();
        let mut raster_height = raster_width * tree.size().height() / tree.size().width();
        let cap = (8192.0 / raster_width).min(8192.0 / raster_height).min(1.0);
        raster_width *= cap;
        raster_height *= cap;
        if raster_width < 1.0 || raster_height < 1.0 {
            return Ok(());
        }
        let (width, height) = (raster_width as u32, raster_height as u32);
        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or(RenderError::Vector(VectorError::TooLarge))?;
        let transform = tiny_skia::Transform::from_scale(
            raster_width / tree.size().width(),
            raster_height / tree.size().height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let pixels = pixmap.pixels().iter().map(|pixel| pixel.alpha()).collect();
        let cx = (snap(svg.bounds.x, scale) + snap(svg.bounds.right(), scale)) * 0.5;
        let cy = (snap(svg.bounds.y, scale) + snap(svg.bounds.bottom(), scale)) * 0.5;
        let w = width as f32 * 0.5 * svg.element_scale;
        let h = height as f32 * 0.5 * svg.element_scale;
        let x = snap(cx - w * 0.5, 1.0);
        let y = snap(cy - h * 0.5, 1.0);
        (
            width,
            height,
            pixels,
            Rect::from_edges(x, y, x + w.ceil(), y + h.ceil()),
            wgpu::TextureFormat::R8Unorm,
        )
    } else {
        let ratio = tree.size().width() / tree.size().height();
        let (display_w, display_h) = if original.w / original.h > ratio {
            (original.h * ratio, original.h)
        } else {
            (original.w, original.w / ratio)
        };
        let physical_scale = (scale.max(0.01) * 2.0).max(1.0);
        let mut width = (display_w * physical_scale).ceil().max(1.0);
        let mut height = (display_h * physical_scale).ceil().max(1.0);
        let cap = (1024.0_f32 / width)
            .min(1024.0 / height)
            .min((1_048_576.0 / (width * height)).sqrt())
            .min(1.0);
        width = (width * cap).round().max(1.0);
        height = (height * cap).round().max(1.0);
        let (width, height) = (width as u32, height as u32);
        let mut pixmap = tiny_skia::Pixmap::new(width, height)
            .ok_or(RenderError::Vector(VectorError::TooLarge))?;
        let transform = tiny_skia::Transform::from_scale(
            width as f32 / tree.size().width(),
            height as f32 / tree.size().height(),
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());
        let mut pixels = pixmap.take();
        for pixel in pixels.as_chunks_mut::<4>().0 {
            if pixel[3] != 0 {
                let alpha = pixel[3] as f32 / 255.0;
                pixel[0] = (pixel[0] as f32 / alpha) as u8;
                pixel[1] = (pixel[1] as f32 / alpha) as u8;
                pixel[2] = (pixel[2] as f32 / alpha) as u8;
            }
        }
        let display_w = display_w * svg.element_scale;
        let display_h = display_h * svg.element_scale;
        let x = svg.bounds.x + (svg.bounds.w - display_w) * 0.5;
        let y = svg.bounds.y + (svg.bounds.h - display_h) * 0.5;
        let left = snap(x, scale);
        let top = snap(y, scale);
        let right = snap(x + display_w, scale).max(left);
        let bottom = snap(y + display_h, scale).max(top);
        (
            width,
            height,
            pixels,
            Rect::from_edges(left, top, right, bottom),
            wgpu::TextureFormat::Rgba8Unorm,
        )
    };
    let bpp = if svg.tint.is_some() { 1 } else { 4 };
    let stride = (texture_width * bpp).div_ceil(256) * 256;
    let mut padded = vec![0; stride as usize * texture_height as usize];
    let row_len = (texture_width * bpp) as usize;
    for (y, row) in pixels.chunks_exact(row_len).enumerate() {
        padded[y * stride as usize..y * stride as usize + row_len].copy_from_slice(row);
    }
    let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("aqua-gpui-svg"),
        size: wgpu::Extent3d {
            width: texture_width,
            height: texture_height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    renderer.queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &padded,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(stride),
            rows_per_image: Some(texture_height),
        },
        wgpu::Extent3d {
            width: texture_width,
            height: texture_height,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&Default::default());
    let Some(tint) = svg.tint else {
        let device = renderer.device.clone();
        return renderer.blit_clipped(
            &device,
            encoder,
            &view,
            target,
            renderer.format,
            destination,
            [0.0, 0.0, 1.0, 1.0],
            false,
            svg.opacity,
            [0.0, 0.0, -1.0, 0.0],
            0.0,
            0.0,
            0.0,
            &svg.clips,
        );
    };
    let device = &renderer.device;
    let parameters = RenderingParameters::cosmic_from_env();
    let parameters = if renderer.text_raster_mode == aqua_core::scene::TextRasterMode::Outline {
        parameters.exact_coverage()
    } else {
        parameters
    };
    let transform = svg.sprite_transform;
    let (sin, cos) = transform.rotation_radians.sin_cos();
    let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("aqua-gpui-svg-parameters"),
        contents: bytemuck::bytes_of(&Parameters {
            bounds: [destination.x, destination.y, destination.w, destination.h],
            hsla: [tint.h, tint.s, tint.l, tint.a],
            gamma: parameters.gamma_ratios,
            misc: [
                parameters.grayscale_enhanced_contrast,
                svg.opacity,
                svg.clip_index as f32,
                0.0,
            ],
            matrix: [
                cos * transform.scale.x,
                -sin * transform.scale.y,
                sin * transform.scale.x,
                cos * transform.scale.y,
            ],
            center_translation: [
                (original.x + original.w * 0.5) * scale,
                (original.y + original.h * 0.5) * scale,
                transform.translation.x * scale,
                transform.translation.y * scale,
            ],
        }),
        usage: wgpu::BufferUsages::UNIFORM,
    });
    let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("aqua-gpui-svg-bind"),
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
                visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            },
        ],
    });
    let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("aqua-gpui-svg-bind"),
        layout: &layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&renderer.sampler),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform.as_entire_binding(),
            },
        ],
    });
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aqua-gpui-svg"),
        source: wgpu::ShaderSource::Wgsl(
            format!(
                "{}\n{}",
                include_str!("../shaders/rect.wgsl"),
                include_str!("../shaders/gpui-svg.wgsl")
            )
            .into(),
        ),
    });
    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aqua-gpui-svg-layout"),
        bind_group_layouts: &[Some(&renderer.rect_bind_layout), Some(&layout)],
        immediate_size: 0,
    });
    let blend = wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::SrcAlpha,
        dst_factor: wgpu::BlendFactor::OneMinusSrcAlpha,
        operation: wgpu::BlendOperation::Add,
    };
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aqua-gpui-svg"),
        layout: Some(&pipeline_layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("svg_vertex"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("svg_fragment"),
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.format,
                blend: Some(wgpu::BlendState {
                    color: blend,
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        ..blend
                    },
                }),
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
    });
    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("aqua-gpui-svg"),
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
    pass.set_pipeline(&pipeline);
    pass.set_bind_group(0, &renderer.rect_uniform_bind, &[]);
    pass.set_bind_group(1, &bind, &[]);
    pass.draw(0..4, 0..1);
    Ok(())
}
