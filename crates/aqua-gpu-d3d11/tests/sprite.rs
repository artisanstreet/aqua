use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_d3d11::sprite::{pack_glyph, pack_glyphs, pack_image};
use aqua_graph::{GlyphInstance, ImageInstance, TextureHandle, TextureRef};

fn rect(x: f32) -> Rect {
    Rect {
        x,
        y: 2.0,
        w: 6.0,
        h: 9.0,
    }
}

fn glyph(x: f32, color: Oklch, opacity: f32) -> GlyphInstance {
    GlyphInstance {
        bounds: rect(x),
        texels: rect(x * 2.0),
        color,
        clip: None,
        opacity,
    }
}

#[test]
fn batched_glyph_packing_equals_per_instance_packing_across_colour_changes() {
    let blue = Oklch::from_hex("#3060ff").unwrap();
    let orange = Oklch::from_hex("#ff7a30").unwrap().with_alpha(0.5);
    let glyphs = vec![
        glyph(1.0, blue, 1.0),
        glyph(2.0, blue, 0.5),
        glyph(3.0, orange, 1.0),
        glyph(4.0, blue, 0.25),
        glyph(5.0, blue, 0.25),
    ];
    let batched = pack_glyphs(&glyphs);
    let single: Vec<_> = glyphs.iter().map(pack_glyph).collect();
    assert_eq!(batched, single);
}

#[test]
fn unclipped_sprites_use_a_clip_that_covers_any_surface() {
    let packed = pack_glyph(&glyph(1.0, Oklch::WHITE, 1.0));
    assert!(packed.clip[0] < -1.0e4 && packed.clip[2] > 1.0e5);
}

#[test]
fn image_parameters_carry_radius_and_opacity() {
    let image = ImageInstance {
        source: TextureRef::Persistent(TextureHandle(1)),
        bounds: rect(1.0),
        texels: rect(0.0),
        radius: 4.0,
        opacity: 0.25,
        clip: Some(rect(0.0)),
        clip_radius: 0.0,
        blend: aqua_graph::Blend::Over,
        linear: aqua_graph::Linear2::IDENTITY,
        mask: None,
    };
    let packed = pack_image(&image);
    assert_eq!(&packed.tail[..2], &[4.0, 0.25]);
    assert_eq!(packed.clip, [0.0, 2.0, 6.0, 9.0]);
}
