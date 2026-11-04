use super::{BlurParams, ClipShape, Rect, RenderError, Renderer};

#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct GpuiBlurUniforms {
    texel: [f32; 2],
    direction: [f32; 2],
    sigma: f32,
    step: f32,
    mode: f32,
    taps: f32,
    source_origin: [f32; 2],
    padding: [f32; 2],
}

pub(super) fn region(bounds: Rect, radius: f32, width: u32, height: u32) -> Option<[u32; 4]> {
    let half_w = (width / 2).max(1);
    let half_h = (height / 2).max(1);
    let dilation = 3.0 * radius;
    let x0 = (((bounds.x - dilation) * 0.5).floor().max(0.0) as u32).min(half_w);
    let y0 = (((bounds.y - dilation) * 0.5).floor().max(0.0) as u32).min(half_h);
    let x1 = ((((bounds.right() + dilation) * 0.5).ceil().max(0.0) as u32).min(half_w)).max(x0);
    let y1 = ((((bounds.bottom() + dilation) * 0.5).ceil().max(0.0) as u32).min(half_h)).max(y0);
    (x0 != x1 && y0 != y1).then_some([2 * x0, 2 * y0, 2 * x1, 2 * y1])
}

pub(super) fn draw(
    renderer: &mut Renderer,
    encoder: &mut wgpu::CommandEncoder,
    target: (&wgpu::TextureView, &wgpu::Texture, [u32; 2]),
    bounds: Rect,
    filter: (BlurParams, bool, &[ClipShape], f32),
) -> Result<(), RenderError> {
    let (params, replace, clips, opacity) = filter;
    let (target, target_texture, copy_size) = target;
    let sigma = params.radius_px * 0.5;
    if sigma <= 0.0 {
        return Ok(());
    }
    let ideal_taps = (3.0 * sigma).ceil();
    let taps = ideal_taps.clamp(1.0, 32.0);
    let step = (ideal_taps / taps).max(1.0);
    let device = renderer.device.clone();
    let queue = renderer.queue.clone();
    let (width, height) = ((renderer.width / 2).max(1), (renderer.height / 2).max(1));
    let dilation = 3.0 * params.radius_px;
    let Some([left, top, right, bottom]) =
        region(bounds, params.radius_px, renderer.width, renderer.height)
    else {
        return Ok(());
    };
    let (x0, y0, x1, y1) = (left / 2, top / 2, right / 2, bottom / 2);
    let copy = if replace {
        None
    } else {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("aqua-gpui-backdrop-copy"),
            size: wgpu::Extent3d {
                width: copy_size[0],
                height: copy_size[1],
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: renderer.format,
            usage: wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        });
        encoder.copy_texture_to_texture(
            wgpu::TexelCopyTextureInfo {
                texture: target_texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: left,
                    y: top,
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            texture.as_image_copy(),
            wgpu::Extent3d {
                width: right - left,
                height: bottom - top,
                depth_or_array_layers: 1,
            },
        );
        Some(texture)
    };
    let source_view = copy
        .as_ref()
        .map(|texture| texture.create_view(&wgpu::TextureViewDescriptor::default()))
        .unwrap_or_else(|| target.clone());
    let source_origin = if replace {
        [0.0, 0.0]
    } else {
        [left as f32, top as f32]
    };
    let first = renderer
        .pool
        .acquire(&device, width, height, renderer.format)?;
    let second = match renderer
        .pool
        .acquire(&device, width, height, renderer.format)
    {
        Ok(value) => value,
        Err(error) => {
            renderer.pool.release(first);
            return Err(error);
        }
    };
    let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("aqua-gpui-blur"),
        source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/gpui-blur.wgsl").into()),
    });
    let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("aqua-gpui-blur"),
        bind_group_layouts: &[Some(&renderer.blur_bind_layout)],
        immediate_size: 0,
    });
    let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
        label: Some("aqua-gpui-blur"),
        layout: Some(&layout),
        vertex: wgpu::VertexState {
            module: &shader,
            entry_point: Some("vs_main"),
            buffers: &[],
            compilation_options: Default::default(),
        },
        fragment: Some(wgpu::FragmentState {
            module: &shader,
            entry_point: Some("fs_main"),
            targets: &[Some(wgpu::ColorTargetState {
                format: renderer.format,
                blend: None,
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
    let a = renderer.pool.entries[first].view.clone();
    let b = renderer.pool.entries[second].view.clone();
    for (source, destination, direction, mode) in [
        (source_view, a.clone(), [0.0, 0.0], 0.0),
        (a.clone(), b.clone(), [1.0, 0.0], 1.0),
        (b.clone(), a.clone(), [0.0, 1.0], 1.0),
    ] {
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("aqua-gpui-blur-uniform"),
            size: std::mem::size_of::<GpuiBlurUniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(
            &uniform,
            0,
            bytemuck::bytes_of(&GpuiBlurUniforms {
                texel: [1.0 / width as f32, 1.0 / height as f32],
                direction,
                sigma,
                step,
                mode,
                taps,
                source_origin,
                padding: [0.0; 2],
            }),
        );
        let bind = renderer.bind_for(&device, &renderer.blur_bind_layout, &uniform, &source);
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aqua-gpui-blur-pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &destination,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        pass.set_pipeline(&pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.set_scissor_rect(x0, y0, x1 - x0, y1 - y0);
        pass.draw(0..3, 0..1);
    }
    if replace {
        let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("aqua-gpui-filter-clear"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: target,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
    }
    let composite = if replace {
        crate::scene::inflate(bounds, dilation)
    } else {
        bounds
    };
    let left = composite.x.max(0.0);
    let top = composite.y.max(0.0);
    let right = composite.right().min(renderer.width as f32).max(left);
    let bottom = composite.bottom().min(renderer.height as f32).max(top);
    let destination = Rect::from_edges(left, top, right, bottom);
    let uv = [
        left / (2.0 * width as f32),
        top / (2.0 * height as f32),
        (right - left) / (2.0 * width as f32),
        (bottom - top) / (2.0 * height as f32),
    ];
    let result = renderer.blit_clipped(
        &device,
        encoder,
        &a,
        target,
        renderer.format,
        destination,
        uv,
        false,
        opacity,
        [0.0, 0.0, -1.0, 0.0],
        0.0,
        0.0,
        1.0,
        clips,
    );
    renderer.pool.release(first);
    renderer.pool.release(second);
    result
}
