use aqua_core::geometry::Rect;
use aqua_core::Oklch;
use aqua_gpu_ref::raster::Layer;
use aqua_gpu_ref::sprites::{
    draw_glyphs, draw_images, sample_image, RefTexture, TextureError, Textures,
};
use aqua_graph::{
    Extent, GlyphInstance, ImageInstance, ImageMask, TextureFormat, TextureHandle, TextureRef,
    ToneCurve,
};

fn rect(x: f32, y: f32, w: f32, h: f32) -> Rect {
    Rect { x, y, w, h }
}

fn white(alpha: f32) -> Oklch {
    Oklch::from_hex("#ffffff").unwrap().with_alpha(alpha)
}

fn mask_texture(width: u32, height: u32, fill: impl Fn(u32, u32) -> u8) -> RefTexture {
    let mut texture = RefTexture::new(Extent { width, height }, TextureFormat::R8Unorm).unwrap();
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| fill(x, y))
        .collect();
    texture
        .write(0, 0, Extent { width, height }, &data)
        .unwrap();
    texture
}

fn color_texture(width: u32, height: u32, fill: impl Fn(u32, u32) -> [u8; 4]) -> RefTexture {
    let mut texture = RefTexture::new(Extent { width, height }, TextureFormat::Rgba8Unorm).unwrap();
    let data: Vec<u8> = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| fill(x, y))
        .collect();
    texture
        .write(0, 0, Extent { width, height }, &data)
        .unwrap();
    texture
}

#[test]
fn glyph_pixels_take_their_coverage_from_the_matching_atlas_texel() {
    let atlas = mask_texture(8, 8, |x, y| (x * 16 + y) as u8);
    let mut layer = Layer::filled(16, 16, [0.0; 4]);
    let glyph = GlyphInstance {
        bounds: rect(4.0, 5.0, 3.0, 2.0),
        texels: rect(2.0, 1.0, 3.0, 2.0),
        color: white(1.0),
        clip: None,
        opacity: 1.0,
    };
    draw_glyphs(&mut layer, &atlas, &[glyph], ToneCurve::native());
    for y in 0..16usize {
        for x in 0..16usize {
            let alpha = layer.pixels[y * 16 + x][3];
            match (4..7).contains(&x) && (5..7).contains(&y) {
                true => {
                    let coverage = ((x - 4 + 2) * 16 + (y - 5 + 1)) as f32 / 255.0;
                    let expected = ToneCurve::native().alpha(coverage, 1.0);
                    assert!(
                        (alpha - expected).abs() < 1e-6,
                        "({x},{y}) {alpha} vs {expected}"
                    );
                }
                false => assert_eq!(alpha, 0.0, "({x},{y})"),
            }
        }
    }
}

#[test]
fn glyph_clip_and_opacity_scale_the_coverage() {
    let atlas = mask_texture(4, 4, |_, _| 255);
    let mut layer = Layer::filled(8, 8, [0.0; 4]);
    let glyph = GlyphInstance {
        bounds: rect(0.0, 0.0, 4.0, 4.0),
        texels: rect(0.0, 0.0, 4.0, 4.0),
        color: white(1.0),
        clip: Some(rect(0.0, 0.0, 2.0, 4.0)),
        opacity: 0.5,
    };
    draw_glyphs(&mut layer, &atlas, &[glyph], ToneCurve::native());
    assert!((layer.pixels[1][3] - 0.5).abs() < 1e-6);
    assert!(layer.pixels[3][3].abs() < 1e-6);
}

#[test]
fn glyph_sampling_clamps_inside_its_atlas_rectangle() {
    let atlas = mask_texture(8, 4, |x, _| if x < 4 { 255 } else { 0 });
    let mut layer = Layer::filled(8, 4, [0.0; 4]);
    let glyph = GlyphInstance {
        bounds: rect(0.0, 0.0, 8.0, 4.0),
        texels: rect(0.0, 0.0, 4.0, 4.0),
        color: white(1.0),
        clip: None,
        opacity: 1.0,
    };
    draw_glyphs(&mut layer, &atlas, &[glyph], ToneCurve::native());
    assert!(layer
        .pixels
        .iter()
        .all(|pixel| (pixel[3] - 1.0).abs() < 1e-6));
}

#[test]
fn one_to_one_bilinear_sampling_returns_texels_exactly_at_pixel_centres() {
    let texture = color_texture(4, 4, |x, y| [(x * 40) as u8, (y * 40) as u8, 7, 255]);
    let region = rect(0.0, 0.0, 4.0, 4.0);
    for y in 0..4u32 {
        for x in 0..4u32 {
            let sample = sample_image(
                &texture,
                region,
                (x as f32 + 0.5) / 4.0,
                (y as f32 + 0.5) / 4.0,
            );
            assert!((sample[0] - (x * 40) as f32 / 255.0).abs() < 1e-6);
            assert!((sample[1] - (y * 40) as f32 / 255.0).abs() < 1e-6);
        }
    }
}

#[test]
fn bilinear_edges_clamp_to_the_sub_rectangle_instead_of_bleeding_from_neighbours() {
    let texture = color_texture(8, 4, |x, _| {
        if x < 4 {
            [255, 0, 0, 255]
        } else {
            [0, 0, 255, 255]
        }
    });
    let region = rect(0.0, 0.0, 4.0, 4.0);
    let right_edge = sample_image(&texture, region, 0.999, 0.5);
    assert!(
        right_edge[2] < 1e-6,
        "blue bled into the sub-rect: {right_edge:?}"
    );
}

#[test]
fn image_corners_are_rounded_and_opacity_applies() {
    let texture = color_texture(16, 16, |_, _| [255, 255, 255, 255]);
    let mut textures = Textures::default();
    textures
        .create(
            TextureHandle(3),
            Extent {
                width: 16,
                height: 16,
            },
            TextureFormat::Rgba8Unorm,
        )
        .unwrap();
    textures
        .write(
            TextureHandle(3),
            0,
            0,
            Extent {
                width: 16,
                height: 16,
            },
            &texture.data,
        )
        .unwrap();
    let mut layer = Layer::filled(24, 24, [0.0; 4]);
    let image = ImageInstance {
        source: TextureRef::Persistent(TextureHandle(3)),
        bounds: rect(4.0, 4.0, 16.0, 16.0),
        texels: rect(0.0, 0.0, 16.0, 16.0),
        radius: 6.0,
        opacity: 0.5,
        clip: None,
        clip_radius: 0.0,
        blend: aqua_graph::Blend::Over,
        linear: aqua_graph::Linear2::IDENTITY,
        mask: None,
    };
    draw_images(&mut layer, &textures, &[image]);
    assert!((layer.pixels[12 * 24 + 12][3] - 0.5).abs() < 1e-6);
    assert!(layer.pixels[4 * 24 + 4][3] < 0.05);
    assert!(layer.pixels[3 * 24 + 12][3] < 0.3);
}

fn upload(textures: &mut Textures, handle: TextureHandle, texture: &RefTexture) {
    textures
        .create(handle, texture.extent, texture.format)
        .unwrap();
    textures
        .write(handle, 0, 0, texture.extent, &texture.data)
        .unwrap();
}

#[test]
fn a_mask_cuts_the_image_to_its_shape() {
    let mut textures = Textures::default();
    upload(
        &mut textures,
        TextureHandle(3),
        &color_texture(8, 8, |_, _| [255, 255, 255, 255]),
    );
    upload(
        &mut textures,
        TextureHandle(4),
        &mask_texture(24, 12, |x, y| match (10..14).contains(&x) && y >= 2 {
            true => 255,
            false => 0,
        }),
    );
    let mut layer = Layer::filled(8, 8, [0.0; 4]);
    let image = ImageInstance {
        source: TextureRef::Persistent(TextureHandle(3)),
        bounds: rect(0.0, 0.0, 8.0, 8.0),
        texels: rect(0.0, 0.0, 8.0, 8.0),
        radius: 0.0,
        opacity: 1.0,
        clip: None,
        clip_radius: 0.0,
        blend: aqua_graph::Blend::Replace,
        linear: aqua_graph::Linear2::IDENTITY,
        mask: Some(ImageMask {
            source: TextureRef::Persistent(TextureHandle(4)),
            offset: [10.0, 2.0],
            scale: [1.0, 1.0],
        }),
    };
    draw_images(&mut layer, &textures, &[image]);
    for y in 0..8usize {
        for x in 0..8usize {
            let expected = match x < 4 {
                true => 1.0,
                false => 0.0,
            };
            assert_eq!(layer.pixels[y * 8 + x][3], expected, "({x},{y})");
        }
    }
}

#[test]
fn a_mask_rasterised_at_a_different_scale_is_stretched_over_the_image() {
    let mut textures = Textures::default();
    upload(
        &mut textures,
        TextureHandle(3),
        &color_texture(16, 16, |_, _| [255, 255, 255, 255]),
    );
    upload(
        &mut textures,
        TextureHandle(4),
        &mask_texture(8, 8, |x, _| match x < 4 {
            true => 255,
            false => 0,
        }),
    );
    let mut layer = Layer::filled(16, 16, [0.0; 4]);
    let image = ImageInstance {
        source: TextureRef::Persistent(TextureHandle(3)),
        bounds: rect(0.0, 0.0, 16.0, 16.0),
        texels: rect(0.0, 0.0, 16.0, 16.0),
        radius: 0.0,
        opacity: 1.0,
        clip: None,
        clip_radius: 0.0,
        blend: aqua_graph::Blend::Replace,
        linear: aqua_graph::Linear2::IDENTITY,
        mask: Some(ImageMask {
            source: TextureRef::Persistent(TextureHandle(4)),
            offset: [0.0, 0.0],
            scale: [0.5, 0.5],
        }),
    };
    draw_images(&mut layer, &textures, &[image]);
    for y in 1..15usize {
        for x in 1..6usize {
            assert_eq!(layer.pixels[y * 16 + x][3], 1.0, "({x},{y})");
        }
        for x in 9..16usize {
            assert_eq!(layer.pixels[y * 16 + x][3], 0.0, "({x},{y})");
        }
    }
}

#[test]
fn textures_reject_writes_outside_their_extent_and_unknown_handles() {
    let mut textures = Textures::default();
    textures
        .create(
            TextureHandle(1),
            Extent {
                width: 4,
                height: 4,
            },
            TextureFormat::R8Unorm,
        )
        .unwrap();
    assert_eq!(
        textures.write(
            TextureHandle(1),
            3,
            0,
            Extent {
                width: 2,
                height: 1
            },
            &[0, 0]
        ),
        Err(TextureError::OutOfBounds)
    );
    assert_eq!(
        textures.write(
            TextureHandle(1),
            0,
            0,
            Extent {
                width: 2,
                height: 1
            },
            &[0]
        ),
        Err(TextureError::DataSize)
    );
    assert_eq!(
        textures.write(
            TextureHandle(9),
            0,
            0,
            Extent {
                width: 1,
                height: 1
            },
            &[0]
        ),
        Err(TextureError::UnknownHandle)
    );
    assert!(textures.release(TextureHandle(1)));
    assert!(textures.is_empty());
}
