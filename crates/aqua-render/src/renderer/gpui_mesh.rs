use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use aqua_core::gpui_mesh::{GpuiMesh, GpuiMeshStyle};
use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Vertex {
    position: [f32; 3],
    normal: [i16; 4],
    color: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy, PartialEq, Pod, Zeroable)]
struct Uniforms {
    view: [[f32; 4]; 4],
    projection: [[f32; 4]; 4],
    base_color: [f32; 4],
    edge_color: [f32; 4],
    params: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Composite {
    bounds: [f32; 4],
    clip: [f32; 4],
    corners: [f32; 4],
    params: [f32; 4],
    viewport: [f32; 4],
}

struct Geometry {
    buffer: wgpu::Buffer,
    vertices: u32,
    last_painted: u64,
}

struct Target {
    size: [u32; 2],
    view: wgpu::TextureView,
    rendered: Option<(u64, Uniforms)>,
}

struct Scratch {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    last_used: u64,
}

pub(super) struct CompositeDraw<'a> {
    pub target: &'a wgpu::TextureView,
    pub index: usize,
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub corners: [f32; 4],
    pub opacity: f32,
    pub viewport: [u32; 2],
    pub clips: &'a [super::ClipShape],
}

pub(super) struct GpuiMeshRenderer {
    mesh_pipeline: wgpu::RenderPipeline,
    composite_pipeline: wgpu::RenderPipeline,
    mesh_layout: wgpu::BindGroupLayout,
    composite_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    geometry: HashMap<u64, Geometry>,
    targets: Vec<Target>,
    scratch: HashMap<[u32; 2], Scratch>,
    frame: u64,
    last_prepare: Option<Instant>,
    drew: bool,
}

impl GpuiMeshRenderer {
    pub(super) fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let mesh_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpui-mesh-uniforms"),
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
        let composite_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("gpui-mesh-composite"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
            ],
        });
        let mesh_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpui-mesh"),
            source: wgpu::ShaderSource::Wgsl(include_str!("../shaders/gpui-mesh.wgsl").into()),
        });
        let composite_shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("gpui-mesh-composite"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("../shaders/gpui-mesh-composite.wgsl").into(),
            ),
        });
        let mesh_pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("gpui-mesh-layout"),
            bind_group_layouts: &[Some(&mesh_layout)],
            immediate_size: 0,
        });
        let composite_pipeline_layout =
            device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("gpui-mesh-composite-layout"),
                bind_group_layouts: &[Some(&composite_layout)],
                immediate_size: 0,
            });
        let mesh_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpui-mesh"),
            layout: Some(&mesh_pipeline_layout),
            vertex: wgpu::VertexState { module: &mesh_shader, entry_point: Some("mesh_vertex"), buffers: &[Some(wgpu::VertexBufferLayout { array_stride: std::mem::size_of::<Vertex>() as u64, step_mode: wgpu::VertexStepMode::Vertex, attributes: &wgpu::vertex_attr_array![0 => Float32x3, 1 => Snorm16x4, 2 => Unorm8x4] })], compilation_options: Default::default() },
            fragment: Some(wgpu::FragmentState { module: &mesh_shader, entry_point: Some("mesh_fragment"), targets: &[Some(wgpu::ColorTargetState { format: wgpu::TextureFormat::Rgba8Unorm, blend: None, write_mask: wgpu::ColorWrites::ALL })], compilation_options: Default::default() }),
            primitive: wgpu::PrimitiveState { cull_mode: None, ..Default::default() },
            depth_stencil: Some(wgpu::DepthStencilState { format: wgpu::TextureFormat::Depth32Float, depth_write_enabled: Some(true), depth_compare: Some(wgpu::CompareFunction::Less), stencil: Default::default(), bias: Default::default() }),
            multisample: wgpu::MultisampleState { count: 4, ..Default::default() },
            multiview_mask: None,
            cache: None,
        });
        let composite_pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("gpui-mesh-composite"),
            layout: Some(&composite_pipeline_layout),
            vertex: wgpu::VertexState {
                module: &composite_shader,
                entry_point: Some("composite_vertex"),
                buffers: &[],
                compilation_options: Default::default(),
            },
            fragment: Some(wgpu::FragmentState {
                module: &composite_shader,
                entry_point: Some("composite_fragment"),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
                compilation_options: Default::default(),
            }),
            primitive: wgpu::PrimitiveState {
                topology: wgpu::PrimitiveTopology::TriangleStrip,
                ..Default::default()
            },
            depth_stencil: None,
            multisample: Default::default(),
            multiview_mask: None,
            cache: None,
        });
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("gpui-mesh-sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Linear,
            ..Default::default()
        });
        Self {
            mesh_pipeline,
            composite_pipeline,
            mesh_layout,
            composite_layout,
            sampler,
            geometry: HashMap::new(),
            targets: Vec::new(),
            scratch: HashMap::new(),
            frame: 0,
            last_prepare: None,
            drew: false,
        }
    }

    pub(super) fn begin_frame(&mut self, count: usize) {
        self.frame = self.frame.wrapping_add(1);
        self.drew = false;
        self.targets.truncate(count);
        if count == 0 {
            self.geometry.clear();
            self.scratch.clear();
        }
    }

    pub(super) fn prepare(
        &mut self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        index: usize,
        bounds: [f32; 4],
        mesh: &Arc<GpuiMesh>,
        style: GpuiMeshStyle,
    ) -> Result<(), String> {
        let side = |value: f32| value.ceil().clamp(1.0, 4096.0) as u32;
        if !bounds.iter().all(|value| value.is_finite()) || index > self.targets.len() {
            return Err("invalid GPUI mesh target".into());
        }
        let size = [side(bounds[2]), side(bounds[3])];
        if self
            .targets
            .get(index)
            .is_none_or(|target| target.size != size)
        {
            let texture = device.create_texture(&wgpu::TextureDescriptor {
                label: Some("gpui-mesh-resolve"),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                    | wgpu::TextureUsages::TEXTURE_BINDING,
                view_formats: &[],
            });
            let target = Target {
                size,
                view: texture.create_view(&wgpu::TextureViewDescriptor::default()),
                rendered: None,
            };
            if index == self.targets.len() {
                self.targets.push(target);
            } else {
                self.targets[index] = target;
            }
        }
        if !self.geometry.contains_key(&mesh.id()) {
            if std::mem::size_of_val(mesh.vertices()) > 256 * 1024 * 1024 {
                return Err("GPUI mesh exceeds vertex budget".into());
            }
            let vertices: Vec<_> = mesh
                .vertices()
                .iter()
                .map(|vertex| Vertex {
                    position: vertex.position,
                    normal: vertex.normal,
                    color: vertex.color,
                })
                .collect();
            let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("gpui-mesh-vertices"),
                contents: bytemuck::cast_slice(&vertices),
                usage: wgpu::BufferUsages::VERTEX,
            });
            self.geometry.insert(
                mesh.id(),
                Geometry {
                    buffer,
                    vertices: vertices.len() as u32,
                    last_painted: self.frame,
                },
            );
        }
        let geometry = self
            .geometry
            .get_mut(&mesh.id())
            .ok_or("GPUI mesh geometry missing")?;
        geometry.last_painted = self.frame;
        let uniforms = Uniforms {
            view: style.view,
            projection: style.projection,
            base_color: [
                style.base_color[0],
                style.base_color[1],
                style.base_color[2],
                1.0,
            ],
            edge_color: style.edge_color,
            params: [0.6, 0.0, 0.0, 0.0],
        };
        if self.targets[index].rendered == Some((mesh.id(), uniforms)) {
            return Ok(());
        }
        self.drew = true;
        let scratch = self.scratch.entry(size).or_insert_with(|| {
            let texture = |format, usage| {
                device
                    .create_texture(&wgpu::TextureDescriptor {
                        label: Some("gpui-mesh-scratch"),
                        size: wgpu::Extent3d {
                            width: size[0],
                            height: size[1],
                            depth_or_array_layers: 1,
                        },
                        mip_level_count: 1,
                        sample_count: 4,
                        dimension: wgpu::TextureDimension::D2,
                        format,
                        usage,
                        view_formats: &[],
                    })
                    .create_view(&wgpu::TextureViewDescriptor::default())
            };
            Scratch {
                color: texture(
                    wgpu::TextureFormat::Rgba8Unorm,
                    wgpu::TextureUsages::RENDER_ATTACHMENT,
                ),
                depth: texture(
                    wgpu::TextureFormat::Depth32Float,
                    wgpu::TextureUsages::RENDER_ATTACHMENT,
                ),
                last_used: self.frame,
            }
        });
        scratch.last_used = self.frame;
        let uniform = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpui-mesh-uniforms"),
            contents: bytemuck::bytes_of(&uniforms),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpui-mesh-bind"),
            layout: &self.mesh_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("gpui-mesh-raster"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &scratch.color,
                    depth_slice: None,
                    resolve_target: Some(&self.targets[index].view),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &scratch.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Discard,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(&self.mesh_pipeline);
            pass.set_bind_group(0, &bind, &[]);
            pass.set_vertex_buffer(0, geometry.buffer.slice(..));
            pass.draw(0..geometry.vertices, 0..1);
        }
        self.targets[index].rendered = Some((mesh.id(), uniforms));
        Ok(())
    }

    pub(super) fn finish_prepare(&mut self) {
        let now = Instant::now();
        if !self.drew {
            if self
                .last_prepare
                .is_some_and(|last| now.duration_since(last) >= Duration::from_secs(1))
            {
                self.scratch.clear();
            }
            self.geometry
                .retain(|_, geometry| self.frame.wrapping_sub(geometry.last_painted) <= 120);
            return;
        }
        if self
            .last_prepare
            .is_some_and(|last| now.duration_since(last) < Duration::from_secs(1))
        {
            self.scratch
                .retain(|_, target| target.last_used == self.frame);
        } else {
            self.scratch.clear();
        }
        self.last_prepare = Some(now);
        self.geometry
            .retain(|_, geometry| self.frame.wrapping_sub(geometry.last_painted) <= 120);
    }

    pub(super) fn composite(
        &self,
        device: &wgpu::Device,
        encoder: &mut wgpu::CommandEncoder,
        draw: CompositeDraw<'_>,
    ) -> Result<(), String> {
        let CompositeDraw {
            target,
            index,
            bounds,
            clip,
            corners,
            opacity,
            viewport,
            clips,
        } = draw;
        let resolved = self.targets.get(index).ok_or("GPUI mesh target missing")?;
        let uniform = Composite {
            bounds,
            clip,
            corners,
            params: [opacity, clips.len() as f32, 0.0, 0.0],
            viewport: [viewport[0] as f32, viewport[1] as f32, 0.0, 0.0],
        };
        let buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpui-mesh-composite-uniforms"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM,
        });
        let mut clip_data: Vec<[f32; 12]> = clips
            .iter()
            .map(|(bounds, radii, mode)| {
                [
                    bounds.x,
                    bounds.y,
                    bounds.w,
                    bounds.h,
                    radii.top_left,
                    radii.top_right,
                    radii.bottom_right,
                    radii.bottom_left,
                    match mode {
                        super::ClipMode::Legacy => 0.0,
                        super::ClipMode::Rounded => 1.0,
                        super::ClipMode::ContentMask => 2.0,
                    },
                    0.0,
                    0.0,
                    0.0,
                ]
            })
            .collect();
        if clip_data.is_empty() {
            clip_data.push([0.0; 12]);
        }
        if std::mem::size_of_val(clip_data.as_slice()) as u64
            > device.limits().max_storage_buffer_binding_size
        {
            return Err("GPUI mesh clip stack exceeds device limit".into());
        }
        let clip_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("gpui-mesh-clips"),
            contents: bytemuck::cast_slice(&clip_data),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gpui-mesh-composite-bind"),
            layout: &self.composite_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: buffer.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&resolved.view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
                wgpu::BindGroupEntry {
                    binding: 3,
                    resource: clip_buffer.as_entire_binding(),
                },
            ],
        });
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("gpui-mesh-composite"),
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
        pass.set_pipeline(&self.composite_pipeline);
        pass.set_bind_group(0, &bind, &[]);
        pass.draw(0..4, 0..1);
        Ok(())
    }
}
