use aqua_graph::{QuadInstance, ShadowCast, ShadowInstance};

use crate::memo::ColorMemo;

pub const QUAD_STRIDE: usize = std::mem::size_of::<QuadGpu>();
pub const SHADOW_STRIDE: usize = std::mem::size_of::<ShadowGpu>();

const NO_CLIP: [f32; 4] = [-1.0e5, -1.0e5, 2.0e5, 2.0e5];

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct QuadGpu {
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub shape: [f32; 4],
    pub fill: [f32; 4],
    pub border: [f32; 4],
    pub gradient: [f32; 4],
    pub stop_a: [f32; 4],
    pub stop_b: [f32; 4],
    pub stop_offsets: [f32; 4],
    pub xform: [f32; 4],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ShadowGpu {
    pub bounds: [f32; 4],
    pub clip: [f32; 4],
    pub shape: [f32; 4],
    pub offset: [f32; 4],
    pub color: [f32; 4],
}

fn clip_of(clip: Option<aqua_core::geometry::Rect>) -> [f32; 4] {
    clip.map(|clip| [clip.x, clip.y, clip.w, clip.h])
        .unwrap_or(NO_CLIP)
}

fn oklab(color: aqua_core::Oklch) -> [f32; 4] {
    let hue = color.h.to_radians();
    [
        color.l,
        color.c * hue.cos(),
        color.c * hue.sin(),
        color.alpha.clamp(0.0, 1.0),
    ]
}

pub fn pack_quad(instance: &QuadInstance) -> QuadGpu {
    pack_quad_with(instance, &mut ColorMemo::default())
}

pub fn pack_quad_with(instance: &QuadInstance, memo: &mut ColorMemo) -> QuadGpu {
    let bounds = instance.bounds;
    let (flag, gradient, stop_a, stop_b, offsets) = match &instance.gradient {
        Some(gradient) => (
            1.0,
            [
                gradient.start[0],
                gradient.start[1],
                gradient.end[0],
                gradient.end[1],
            ],
            oklab(gradient.stops[0].color),
            oklab(gradient.stops[1].color),
            [gradient.stops[0].offset, gradient.stops[1].offset, 0.0, 0.0],
        ),
        None => (0.0, [0.0; 4], [0.0; 4], [0.0; 4], [0.0; 4]),
    };
    QuadGpu {
        bounds: [bounds.x, bounds.y, bounds.w, bounds.h],
        clip: clip_of(instance.clip),
        shape: [
            instance.radius,
            instance.border_width,
            instance.opacity,
            flag,
        ],
        fill: memo.premultiplied(instance.fill),
        border: memo.straight(instance.border_color),
        gradient,
        stop_a,
        stop_b,
        stop_offsets: offsets,
        xform: instance.linear.pack(),
    }
}

pub fn pack_shadow(instance: &ShadowInstance) -> ShadowGpu {
    pack_shadow_with(instance, &mut ColorMemo::default())
}

pub fn pack_shadow_with(instance: &ShadowInstance, memo: &mut ColorMemo) -> ShadowGpu {
    let bounds = instance.bounds;
    ShadowGpu {
        bounds: [bounds.x, bounds.y, bounds.w, bounds.h],
        clip: clip_of(instance.clip),
        shape: [
            instance.radius,
            instance.sigma,
            instance.spread,
            instance.opacity,
        ],
        offset: [
            instance.offset[0],
            instance.offset[1],
            match instance.cast {
                ShadowCast::Whole => 0.0,
                ShadowCast::Outside => 1.0,
            },
            0.0,
        ],
        color: memo.premultiplied(instance.color),
    }
}
