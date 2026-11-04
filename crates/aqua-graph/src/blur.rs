use aqua_core::geometry::Rect;

use crate::graph::RenderGraph;
use crate::plan::{
    Extent, FilterParams, LoadOp, Pass, PassKind, ShaderId, TextureFormat, TextureRef,
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Snapshot {
    pub texture: TextureRef,
    pub bounds: Rect,
}

fn pixel_region(region: Rect) -> (u32, u32, Extent) {
    let left = region.x.floor().max(0.0) as u32;
    let top = region.y.floor().max(0.0) as u32;
    let right = region.right().ceil().max(left as f32 + 1.0) as u32;
    let bottom = region.bottom().ceil().max(top as f32 + 1.0) as u32;
    (left, top, Extent::new(right - left, bottom - top))
}

pub fn levels_for(radius: f32, size: Extent) -> u32 {
    let wanted = radius.max(1.0).log2().ceil() as u32;
    let smallest = size.width.min(size.height).max(2);
    let fits = smallest.ilog2();
    wanted.clamp(1, MAX_LEVELS).min(fits.max(1))
}

pub fn snapshot(graph: &mut RenderGraph, from: TextureRef, region: Rect) -> Snapshot {
    let (left, top, size) = pixel_region(region);
    let texture = graph.create(size, TextureFormat::Rgba16Float);
    graph.push(Pass::new(
        PassKind::Copy {
            source_origin: [left, top],
            dest_origin: [0, 0],
            size,
        },
        vec![from],
        vec![texture],
    ));
    Snapshot {
        texture,
        bounds: Rect {
            x: left as f32,
            y: top as f32,
            w: size.width as f32,
            h: size.height as f32,
        },
    }
}

pub fn blur_chain(graph: &mut RenderGraph, request: BlurRequest) -> BlurOutput {
    let (left, top, size) = pixel_region(request.region);
    let levels = levels_for(request.radius, size);
    let offset = request.radius / (1u32 << levels) as f32;
    let source_region = Rect {
        x: left as f32,
        y: top as f32,
        w: size.width as f32,
        h: size.height as f32,
    };
    let mut chain = vec![request.source];
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
                params: FilterParams {
                    region,
                    values: [offset, level as f32, request.radius, 0.0],
                },
            },
            vec![chain[level as usize - 1]],
            vec![texture],
        );
        pass.load = LoadOp::DontCare;
        graph.push(pass);
        chain.push(texture);
        extents.push(extent);
    }
    let output = graph.create(size, TextureFormat::Rgba16Float);
    for level in (1..=levels).rev() {
        let destination = match level {
            1 => output,
            _ => chain[level as usize - 1],
        };
        let mut pass = Pass::new(
            PassKind::Filter {
                shader: ShaderId::BlurUp,
                params: FilterParams {
                    region: extents[level as usize - 1].bounds(),
                    values: [offset, level as f32, request.radius, 0.0],
                },
            },
            vec![chain[level as usize]],
            vec![destination],
        );
        pass.load = LoadOp::DontCare;
        graph.push(pass);
    }
    let (placed_left, placed_top, _) = pixel_region(request.placement);
    BlurOutput {
        texture: output,
        bounds: Rect {
            x: placed_left as f32,
            y: placed_top as f32,
            w: size.width as f32,
            h: size.height as f32,
        },
        texels: size.bounds(),
    }
}
