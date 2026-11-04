use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_graph::{GlyphInstance, ImageInstance};

pub const SPRITE_STRIDE: usize = std::mem::size_of::<SpriteGpu>();

const NO_CLIP: [f32; 4] = [-1.0e5, -1.0e5, 2.0e5, 2.0e5];

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, bytemuck::Pod, bytemuck::Zeroable)]
pub struct SpriteGpu {
    pub bounds: [f32; 4],
    pub texels: [f32; 4],
    pub clip: [f32; 4],
    pub tail: [f32; 4],
}

fn rect(rect: Rect) -> [f32; 4] {
    [rect.x, rect.y, rect.w, rect.h]
}

fn clip(clip: Option<Rect>) -> [f32; 4] {
    clip.map(rect).unwrap_or(NO_CLIP)
}

pub fn pack_glyph(glyph: &GlyphInstance) -> SpriteGpu {
    SpriteGpu {
        bounds: rect(glyph.bounds),
        texels: rect(glyph.texels),
        clip: clip(glyph.clip),
        tail: glyph.color.to_gpu().map(|channel| channel * glyph.opacity),
    }
}

pub fn glyph_packer() -> impl FnMut(&GlyphInstance) -> SpriteGpu {
    let mut memo: Option<(Oklch, [f32; 4])> = None;
    move |glyph| {
        let linear = match memo {
            Some((color, linear)) if color == glyph.color => linear,
            _ => {
                let linear = glyph.color.to_gpu();
                memo = Some((glyph.color, linear));
                linear
            }
        };
        SpriteGpu {
            bounds: rect(glyph.bounds),
            texels: rect(glyph.texels),
            clip: clip(glyph.clip),
            tail: linear.map(|channel| channel * glyph.opacity),
        }
    }
}

pub fn pack_glyphs(glyphs: &[GlyphInstance]) -> Vec<SpriteGpu> {
    glyphs.iter().map(glyph_packer()).collect()
}

pub fn pack_image(image: &ImageInstance) -> SpriteGpu {
    SpriteGpu {
        bounds: rect(image.bounds),
        texels: rect(image.texels),
        clip: clip(image.clip),
        tail: [
            image.radius,
            image.opacity,
            image.clip.map_or(0.0, |_| image.clip_radius),
            0.0,
        ],
    }
}
