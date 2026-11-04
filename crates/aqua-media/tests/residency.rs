use std::sync::Arc;

use aqua_core::vector::{Brush, Drawing, PaintedPath, PathCommand, VectorSource};
use aqua_core::{Oklch, Point, Rect};
use aqua_media::atlas::{
    AtlasConfig, ColorGlyphBitmap, GlyphBitmap, GlyphKey, GlyphRaster, ImageKey, PageFormat, Slot,
};
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig, VectorMode};
use aqua_media::vector::{rasterize, VectorBudget, VectorError};

fn square(side: f32, offset: f32) -> VectorSource {
    let corner = |x: f32, y: f32| Point::new(offset + x, offset + y);
    VectorSource::Drawing(Arc::new(Drawing {
        view_box: Rect {
            x: 0.0,
            y: 0.0,
            w: 10.0,
            h: 10.0,
        },
        paths: vec![PaintedPath::fill(
            vec![
                PathCommand::Move(corner(0.0, 0.0)),
                PathCommand::Line(corner(side, 0.0)),
                PathCommand::Line(corner(side, side)),
                PathCommand::Line(corner(0.0, side)),
                PathCommand::Close,
            ],
            Brush::Solid(Oklch::from_hex("#ffffff").unwrap()),
        )],
    }))
}

fn glyph_key(glyph: u32) -> GlyphKey {
    GlyphKey {
        font: 1,
        glyph,
        size_sixty_fourths: 896,
        subpixel: 0,
        variant: 0,
    }
}

#[test]
fn mask_rasterization_covers_the_shape_with_antialiased_edges() {
    let source = square(6.5, 1.0);
    let raster = rasterize(&source, 10, 10, VectorBudget::default()).unwrap();
    let alpha = raster.alpha();
    assert_eq!(alpha[5 * 10 + 5], 255);
    assert_eq!(alpha[0], 0);
    let edge = alpha[5 * 10 + 7];
    assert!(edge > 100 && edge < 160, "half covered column was {edge}");
    assert_eq!(alpha[9 * 10 + 9], 0);
}

#[test]
fn invalid_sizes_are_rejected_before_any_allocation() {
    let source = square(4.0, 1.0);
    assert_eq!(
        rasterize(&source, 0, 8, VectorBudget::default()).err(),
        Some(VectorError::TooLarge)
    );
    assert_eq!(
        rasterize(&source, 9000, 8, VectorBudget::default()).err(),
        Some(VectorError::TooLarge)
    );
}

#[test]
fn vector_masks_rasterize_once_and_reuse_the_atlas_entry() {
    let mut residency = Residency::new(ResidencyConfig::default());
    let source = square(6.0, 1.0);
    residency.begin_frame();
    let first = residency.vector(&source, 8, 8, VectorMode::Mask).unwrap();
    residency.begin_frame();
    let second = residency.vector(&source, 8, 8, VectorMode::Mask).unwrap();
    assert_eq!(first, second);
    assert_eq!(residency.stats().rasterizations, 1);
    let updates = residency.finish_frame();
    assert_eq!(updates.created.len(), 1);
    assert_eq!(updates.created[0].id.format, PageFormat::Mask8);
    assert_eq!(updates.uploads.len(), 1);
    assert_eq!(updates.uploads[0].data.len(), 64);
}

#[test]
fn colour_vectors_land_in_the_colour_atlas_with_four_bytes_per_texel() {
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let slot = residency
        .vector(&square(6.0, 1.0), 8, 8, VectorMode::Color)
        .unwrap();
    let Slot::Resident(placement) = slot else {
        panic!("expected a resident slot")
    };
    assert_eq!(placement.page.format, PageFormat::Color8);
    assert_eq!(residency.finish_frame().uploads[0].data.len(), 8 * 8 * 4);
}

#[test]
fn failed_vectors_leave_no_entry_behind() {
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let source = square(6.0, 1.0);
    assert!(residency.vector(&source, 0, 8, VectorMode::Mask).is_err());
    assert_eq!(residency.stats().masks.entries, 0);
    assert_eq!(residency.stats().rasterizations, 0);
}

#[test]
fn evicting_a_vector_releases_the_retained_source() {
    let config = ResidencyConfig {
        colors: AtlasConfig {
            budget_bytes: 40 * 40 * 4 + 16,
            max_shared_pages: 1,
            base_edge: 32,
            max_edge: 256,
            ..AtlasConfig::images()
        },
        ..ResidencyConfig::default()
    };
    let mut residency = Residency::new(config);
    residency.begin_frame();
    let first = square(30.0, 1.0);
    residency.vector(&first, 40, 40, VectorMode::Color).unwrap();
    residency.finish_frame();
    assert!(residency.stats().retained_source_bytes > 0);
    residency.begin_frame();
    residency.begin_frame();
    let second = square(30.0, 2.0);
    residency
        .vector(&second, 40, 40, VectorMode::Color)
        .unwrap();
    residency.finish_frame();
    assert_eq!(residency.stats().colors.entries, 1);
    let retained_one_source = residency.stats().retained_source_bytes;
    assert!(retained_one_source > 0);
    residency.clear();
    assert_eq!(residency.stats().retained_source_bytes, 0);
}

#[test]
fn glyph_and_image_paths_share_the_frame_update_stream() {
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let mut rasterizer = |key: &GlyphKey| {
        Some(GlyphBitmap {
            width: 5,
            height: 7,
            left: 0,
            top: 7,
            coverage: vec![key.glyph as u8; 35],
        })
    };
    residency.glyph(glyph_key(3), &mut rasterizer).unwrap();
    let image = ImageKey {
        asset: 9,
        frame: 0,
        width: 4,
        height: 4,
    };
    residency.image(image, &[7u8; 64]).unwrap();
    let updates = residency.finish_frame();
    assert_eq!(updates.created.len(), 2);
    assert_eq!(updates.uploads.len(), 2);
    assert_eq!(updates.upload_bytes(), 35 + 64);
    residency.begin_frame();
    assert!(residency.image_resident(&image).is_some());
    assert!(residency.finish_frame().is_empty());
}

#[test]
fn clearing_after_device_loss_makes_every_resident_entry_upload_again() {
    let mut residency = Residency::new(ResidencyConfig::default());
    let mut rasterizer = |key: &GlyphKey| {
        Some(GlyphBitmap {
            width: 5,
            height: 7,
            left: 0,
            top: 7,
            coverage: vec![key.glyph as u8; 35],
        })
    };
    residency.begin_frame();
    residency.glyph(glyph_key(1), &mut rasterizer).unwrap();
    assert_eq!(residency.finish_frame().uploads.len(), 1);
    residency.begin_frame();
    residency.glyph(glyph_key(1), &mut rasterizer).unwrap();
    assert!(residency.finish_frame().is_empty());
    residency.clear();
    residency.begin_frame();
    residency.glyph(glyph_key(1), &mut rasterizer).unwrap();
    let updates = residency.finish_frame();
    assert_eq!(updates.created.len(), 1);
    assert_eq!(updates.uploads.len(), 1);
}

fn any_source(key: &GlyphKey) -> Option<GlyphRaster> {
    match key.variant {
        1 => Some(GlyphRaster::Color(ColorGlyphBitmap {
            width: 3,
            height: 2,
            left: 1,
            top: 2,
            premultiplied_rgba: vec![9u8; 24],
        })),
        2 => None,
        _ => Some(GlyphRaster::Mask(GlyphBitmap {
            width: 2,
            height: 2,
            left: 0,
            top: 2,
            coverage: vec![200u8; 4],
        })),
    }
}

fn variant_key(glyph: u32, variant: u8) -> GlyphKey {
    GlyphKey {
        variant,
        ..glyph_key(glyph)
    }
}

#[test]
fn glyph_any_routes_mask_color_and_blank_glyphs_to_their_atlases() {
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let mut source = any_source;
    let mask = residency.glyph_any(variant_key(1, 0), &mut source).unwrap();
    let color = residency.glyph_any(variant_key(1, 1), &mut source).unwrap();
    let blank = residency.glyph_any(variant_key(1, 2), &mut source).unwrap();
    assert!(
        matches!(mask, GlyphSlot::Mask(Slot::Resident(p)) if p.page.format == PageFormat::Mask8)
    );
    assert!(
        matches!(color, GlyphSlot::Color(Slot::Resident(p)) if p.page.format == PageFormat::Color8)
    );
    assert!(matches!(blank, GlyphSlot::Mask(Slot::Blank { .. })));
    let updates = residency.finish_frame();
    assert_eq!(updates.uploads.len(), 2);
    residency.begin_frame();
    let mut unreachable = |_: &GlyphKey| -> Option<GlyphRaster> { panic!("must be resident") };
    assert_eq!(
        residency
            .glyph_any(variant_key(1, 1), &mut unreachable)
            .unwrap(),
        color
    );
    assert_eq!(
        residency
            .glyph_any(variant_key(1, 0), &mut unreachable)
            .unwrap(),
        mask
    );
    assert_eq!(
        residency
            .glyph_any(variant_key(1, 2), &mut unreachable)
            .unwrap(),
        blank
    );
}

#[test]
fn clearing_forgets_color_glyphs_and_drops_everything_still_streaming() {
    let mut residency = Residency::new(ResidencyConfig::default());
    let mut source = any_source;
    residency.begin_frame();
    residency.glyph_any(variant_key(4, 1), &mut source).unwrap();
    residency.finish_frame();
    assert_eq!(residency.generation(), 0);
    residency.clear();
    assert_eq!(residency.generation(), 1);
    let stats = residency.stats();
    assert_eq!(stats.colors.entries, 0);
    assert_eq!(stats.colors.pages, 0);
    assert_eq!(stats.streaming_bytes, 0);
    residency.begin_frame();
    residency.glyph_any(variant_key(4, 1), &mut source).unwrap();
    let updates = residency.finish_frame();
    assert_eq!(updates.created.len(), 1);
    assert_eq!(updates.uploads.len(), 1);
    assert!(updates.released.is_empty());
}

#[test]
fn color_glyphs_share_one_entry_across_subpixel_bins() {
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let mut source = any_source;
    let bins = (0..4u8)
        .map(|subpixel| {
            let key = GlyphKey {
                subpixel,
                ..variant_key(7, 1)
            };
            residency.glyph_any(key, &mut source).unwrap()
        })
        .collect::<Vec<_>>();
    assert!(bins.iter().all(|slot| *slot == bins[0]));
    let updates = residency.finish_frame();
    assert_eq!(updates.uploads.len(), 1);
    assert_eq!(residency.stats().colors.entries, 1);
}
