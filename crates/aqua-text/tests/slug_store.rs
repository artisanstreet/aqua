use aqua_graph::slug::coverage;
use aqua_text::slug::{pack_glyph, PathCommand, SlugGlyph, SlugStore};

fn rounded_blob() -> Vec<PathCommand> {
    vec![
        PathCommand::Move([0.1, 0.0]),
        PathCommand::Quad([0.5, -0.2], [0.9, 0.0]),
        PathCommand::Quad([1.1, 0.4], [0.8, 0.8]),
        PathCommand::Cubic([0.6, 1.0], [0.3, 1.0], [0.1, 0.7]),
        PathCommand::Line([0.0, 0.3]),
        PathCommand::Close,
        PathCommand::Move([0.3, 0.3]),
        PathCommand::Line([0.3, 0.5]),
        PathCommand::Line([0.6, 0.5]),
        PathCommand::Line([0.6, 0.3]),
        PathCommand::Close,
    ]
}

#[test]
fn packed_buffers_evaluate_exactly_like_the_cpu_glyph() {
    let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
    let base = 37;
    let texels = pack_glyph(&glyph, base);
    let fetch = |index: u32| texels[(index - base) as usize];
    for ppem in [9.0f32, 24.0, 96.0] {
        for j in 0..40 {
            for i in 0..40 {
                let x = -0.1 + 1.3 * i as f32 / 39.0;
                let y = -0.2 + 1.4 * j as f32 / 39.0;
                let packed = coverage(fetch, base, [x, y], ppem);
                let direct = glyph.coverage(x, y, ppem);
                assert!(
                    (packed - direct).abs() < 1.0e-6,
                    "ppem {ppem} at ({x}, {y}): {packed} vs {direct}"
                );
            }
        }
    }
}

#[test]
fn shared_curve_lists_never_alias_between_the_two_band_sets() {
    let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
    let texels = pack_glyph(&glyph, 0);
    let lists_at = texels[3][0] as usize;
    let members = glyph.horizontal.curves.len() + glyph.vertical.curves.len();
    assert_eq!(texels.len(), lists_at + members.div_ceil(4));
}

#[test]
fn empty_store_uploads_nothing() {
    let mut store = SlugStore::new();
    assert!(store.uploads().textures.is_empty());
}

#[cfg(windows)]
mod fonts {
    use super::*;
    use aqua_graph::slug::STORE_WIDTH;
    use aqua_graph::{TextureFormat, TextureHandle};
    use aqua_protocol::TextureOp;
    use aqua_text::raster::place_glyph;
    use aqua_text::slug::STORE_HANDLE;
    use aqua_text::slug::{SlugCache, StoreLookup};
    use aqua_text::{FontManager, TextLayout, TextStyle};

    fn keys(text: &str, size: f32) -> Vec<aqua_text::raster::RasterKey> {
        let mut fonts = FontManager::new();
        let style = TextStyle {
            size_px: size,
            ..TextStyle::default()
        };
        let mut layout = TextLayout::new();
        layout.set_text(text, &style);
        let laid = layout.layout(&mut fonts);
        laid.lines[0]
            .glyphs
            .iter()
            .map(|glyph| place_glyph(glyph, 0.0, 0.0, true).key)
            .collect()
    }

    #[test]
    fn uploads_create_once_then_append_rows_and_recreate_on_growth() {
        let mut fonts = FontManager::new();
        let mut cache = SlugCache::new();
        let mut store = SlugStore::new();
        let first = keys("Hello", 32.0);
        for key in &first {
            assert!(matches!(
                store.lookup(&mut cache, &mut fonts, key),
                StoreLookup::Stored(_)
            ));
        }
        let batch = store.uploads();
        let creates = batch
            .textures
            .iter()
            .filter(|upload| matches!(upload.op, TextureOp::Create { .. }))
            .count();
        assert_eq!(creates, 1);
        assert!(batch
            .textures
            .iter()
            .all(|upload| upload.handle == STORE_HANDLE));
        if let TextureOp::Create { extent, format } = &batch.textures[0].op {
            assert_eq!(extent.width, STORE_WIDTH);
            assert_eq!(*format, TextureFormat::Rgba32Float);
        }
        assert!(store.uploads().textures.is_empty());
        for key in keys(
            "The quick brown fox jumps over the lazy dog 0123456789 @&%",
            32.0,
        ) {
            store.lookup(&mut cache, &mut fonts, &key);
        }
        let grown = store.uploads();
        assert!(!grown.textures.is_empty());
        assert_ne!(STORE_HANDLE, TextureHandle(0));
    }
}
