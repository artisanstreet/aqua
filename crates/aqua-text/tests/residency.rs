#![cfg(windows)]

use aqua_media::atlas::Slot;
use aqua_media::residency::{GlyphSlot, Residency, ResidencyConfig};
use aqua_text::raster::{place_glyph, quantize_size, RasterKey};
use aqua_text::residency::GlyphResolver;
use aqua_text::{FontManager, TextLayout, TextStyle};

fn keys(fonts: &mut FontManager, text: &str, size: f32) -> Vec<RasterKey> {
    let mut layout = TextLayout::new();
    layout.set_text(
        text,
        &TextStyle {
            size_px: size,
            ..TextStyle::default()
        },
    );
    let laid = layout.layout(fonts);
    laid.lines[0]
        .glyphs
        .iter()
        .map(|glyph| place_glyph(glyph, 0.0, 20.0, true).key)
        .collect()
}

#[test]
fn repeated_glyphs_upload_once_and_spaces_stay_blank() {
    let mut fonts = FontManager::new();
    let keys = keys(&mut fonts, "l l", 18.0);
    let mut resolver = GlyphResolver::new();
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let first = resolver
        .resolve(&mut residency, &mut fonts, &keys[0])
        .unwrap();
    let space = resolver
        .resolve(&mut residency, &mut fonts, &keys[1])
        .unwrap();
    let again = resolver
        .resolve(&mut residency, &mut fonts, &keys[0])
        .unwrap();
    assert!(matches!(first, GlyphSlot::Mask(Slot::Resident(_))));
    assert!(matches!(space, GlyphSlot::Mask(Slot::Blank { .. })));
    assert_eq!(first, again);
    let updates = residency.finish_frame();
    assert_eq!(updates.uploads.len(), 1);
    assert_eq!(residency.stats().masks.hits, 1);
}

#[test]
fn glyph_keys_are_stable_and_distinguish_subpixel_and_size() {
    let mut fonts = FontManager::new();
    let base = keys(&mut fonts, "a", 18.0)[0];
    let mut resolver = GlyphResolver::new();
    let first = resolver.key(&base);
    assert_eq!(first, resolver.key(&base));
    let shifted = RasterKey {
        subpixel: (base.subpixel + 1) % 4,
        ..base
    };
    let larger = RasterKey {
        size_bits: quantize_size(18.5).to_bits(),
        ..base
    };
    assert_ne!(first, resolver.key(&shifted));
    assert_ne!(first, resolver.key(&larger));
    assert_eq!(first.size_sixty_fourths, 18 * 64);
}

#[test]
fn recovery_clear_makes_the_same_glyph_upload_again() {
    let mut fonts = FontManager::new();
    let keys = keys(&mut fonts, "a", 18.0);
    let mut resolver = GlyphResolver::new();
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    resolver
        .resolve(&mut residency, &mut fonts, &keys[0])
        .unwrap();
    assert_eq!(residency.finish_frame().uploads.len(), 1);
    residency.clear();
    residency.begin_frame();
    resolver
        .resolve(&mut residency, &mut fonts, &keys[0])
        .unwrap();
    let updates = residency.finish_frame();
    assert_eq!((updates.created.len(), updates.uploads.len()), (1, 1));
}

#[test]
fn lowering_a_laid_out_run_yields_one_instance_per_inked_glyph_hanging_from_the_baseline() {
    use aqua_core::Oklch;
    use aqua_text::residency::RunStyle;
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "Hi, ok",
        &TextStyle {
            size_px: 20.0,
            ..TextStyle::default()
        },
    );
    let laid = layout.layout(&mut fonts);
    let mut resolver = GlyphResolver::new();
    let mut residency = Residency::new(ResidencyConfig::default());
    residency.begin_frame();
    let style = RunStyle {
        color: Oklch::from_hex("#ffffff").unwrap(),
        opacity: 0.5,
        clip: None,
    };
    let draw = resolver
        .lower(&mut residency, &mut fonts, &laid, [10.0, 30.0], style)
        .unwrap();
    assert_eq!(draw.masks.len() + draw.blank, laid.lines[0].glyphs.len());
    assert_eq!(draw.blank, 1);
    assert!(draw.colors.is_empty());
    let baseline = 30.0 + laid.lines[0].baseline_y;
    for (_, instance) in &draw.masks {
        assert_eq!(instance.opacity, 0.5);
        assert!(
            instance.bounds.y < baseline.round()
                && instance.bounds.bottom() > baseline.round() - 30.0
        );
        assert!(instance.bounds.x >= 8.0);
    }
    let again = resolver
        .lower(&mut residency, &mut fonts, &laid, [10.0, 30.0], style)
        .unwrap();
    assert_eq!(again.masks.len(), draw.masks.len());
    let distinct: std::collections::HashSet<(u32, u32)> = draw
        .masks
        .iter()
        .map(|(_, instance)| (instance.texels.x as u32, instance.texels.y as u32))
        .collect();
    assert_eq!(residency.finish_frame().uploads.len(), distinct.len());
}
