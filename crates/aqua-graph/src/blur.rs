use aqua_core::geometry::Rect;

use crate::graph::RenderGraph;
use crate::plan::{
    Blend, Extent, FilterParams, ImageInstance, LoadOp, Pass, PassKind, ShaderId, TextureFormat,
    TextureRef,
};

pub const MAX_LEVELS: u32 = 6;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurRequest {
    pub source: TextureRef,
    pub region: Rect,
    pub placement: Rect,
    pub radius: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BlurOutput {
    pub texture: TextureRef,
    pub bounds: Rect,
    pub texels: Rect,
}

fn pixel_region(region: Rect) -> (u32, u32, Extent) {
    let left = region.x.floor().max(0.0) as u32;
    let top = region.y.floor().max(0.0) as u32;
    let right = region.right().ceil().max(left as f32 + 1.0) as u32;
    let bottom = region.bottom().ceil().max(top as f32 + 1.0) as u32;
    (left, top, Extent::new(right - left, bottom - top))
}

pub const MIX_START: f32 = 0.75;
pub const HALF_RESOLUTION_LEVELS: u32 = 3;
pub const HALF_RESOLUTION_OFFSET_SCALE: f32 = 1.03;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BlurDepth {
    Single { levels: u32 },
    Mixed { lower: u32, weight: f32 },
}

pub fn depth_for(radius: f32, size: Extent) -> BlurDepth {
    let smallest = size.width.min(size.height).max(2);
    let cap = MAX_LEVELS.min(smallest.ilog2().max(1));
    let octave = radius.max(1.0).log2();
    let floor = octave.floor();
    let lower = (floor as u32).clamp(1, cap);
    let fraction = octave - floor;
    match (floor as u32) >= 1 && lower < cap && fraction > MIX_START {
        true => BlurDepth::Mixed {
            lower,
            weight: (fraction - MIX_START) / (1.0 - MIX_START),
        },
        false => BlurDepth::Single { levels: lower },
    }
}

struct Pyramid {
    texture: TextureRef,
    extent: Extent,
}

fn pyramid(
    graph: &mut RenderGraph,
    source: TextureRef,
    source_region: Rect,
    size: Extent,
    levels: u32,
    radius: f32,
) -> Pyramid {
    let shared = levels >= HALF_RESOLUTION_LEVELS;
    let offset = radius / (1u32 << levels) as f32
        * match shared {
            true => HALF_RESOLUTION_OFFSET_SCALE,
            false => 1.0,
        };
    let mut chain = vec![source];
    let mut extents = vec![size];
    for level in 1..=levels {
        let extent = extents[level as usize - 1].half();
        let texture = graph.create(extent, TextureFormat::Rgba16Float);
        let region = match level {
            1 => source_region,
            _ => extents[level as usize - 1].bounds(),
        };
        let mut pass = Pass::new(
            PassKind::Filter {
                shader: ShaderId::BlurDown,
                params: FilterParams::blur(region, [offset, level as f32, radius, 0.0]),
            },
            vec![chain[level as usize - 1]],
            vec![texture],
        );
        pass.load = LoadOp::DontCare;
        graph.push(pass);
        chain.push(texture);
        extents.push(extent);
    }
    let first = match shared {
        true => 2,
        false => 1,
    };
    let (output, output_extent) = match shared {
        true => (chain[1], extents[1]),
        false => (graph.create(size, TextureFormat::Rgba16Float), size),
    };
    for level in (first..=levels).rev() {
        let destination = match level {
            1 => output,
            _ => chain[level as usize - 1],
        };
        let mut pass = Pass::new(
            PassKind::Filter {
                shader: ShaderId::BlurUp,
                params: FilterParams::blur(
                    extents[level as usize - 1].bounds(),
                    [offset, level as f32, radius, 0.0],
                ),
            },
            vec![chain[level as usize]],
            vec![destination],
        );
        pass.load = LoadOp::DontCare;
        graph.push(pass);
    }
    Pyramid {
        texture: output,
        extent: output_extent,
    }
}

fn mix(
    graph: &mut RenderGraph,
    lower: Pyramid,
    upper: Pyramid,
    weight: f32,
    size: Extent,
) -> Pyramid {
    let extent = match lower.extent == upper.extent {
        true => lower.extent,
        false => size,
    };
    let output = graph.create(extent, TextureFormat::Rgba16Float);
    let layer = |source: &Pyramid, opacity: f32| ImageInstance {
        source: source.texture,
        bounds: extent.bounds(),
        texels: source.extent.bounds(),
        radius: 0.0,
        opacity,
        clip: None,
        clip_radius: 0.0,
        blend: Blend::Replace,
        linear: crate::linear::Linear2::IDENTITY,
        mask: None,
    };
    graph.push(Pass::new(
        PassKind::Images {
            instances: vec![layer(&lower, 1.0), layer(&upper, weight)],
        },
        vec![lower.texture, upper.texture],
        vec![output],
    ));
    Pyramid {
        texture: output,
        extent,
    }
}

pub fn blur_chain(graph: &mut RenderGraph, request: BlurRequest) -> BlurOutput {
    let (left, top, size) = pixel_region(request.region);
    let source_region = Rect {
        x: left as f32,
        y: top as f32,
        w: size.width as f32,
        h: size.height as f32,
    };
    let build = |graph: &mut RenderGraph, levels: u32| {
        pyramid(
            graph,
            request.source,
            source_region,
            size,
            levels,
            request.radius,
        )
    };
    let output = match depth_for(request.radius, size) {
        BlurDepth::Single { levels } => build(graph, levels),
        BlurDepth::Mixed { lower, weight } => {
            let near = build(graph, lower);
            let far = build(graph, lower + 1);
            mix(graph, near, far, weight, size)
        }
    };
    let (placed_left, placed_top, _) = pixel_region(request.placement);
    BlurOutput {
        texture: output.texture,
        bounds: Rect {
            x: placed_left as f32,
            y: placed_top as f32,
            w: size.width as f32,
            h: size.height as f32,
        },
        texels: output.extent.bounds(),
    }
}
