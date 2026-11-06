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

#[test]
fn clearing_keeps_the_gpu_texture_and_rewrites_it_in_place() {
    use aqua_protocol::TextureOp;
    let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
    let mut store: SlugStore<u32> = SlugStore::with_budget(1 << 20);
    store.insert(1, &glyph);
    let first = store.uploads();
    assert!(first
        .textures
        .iter()
        .any(|upload| matches!(upload.op, TextureOp::Create { .. })));
    store.clear();
    assert!(store.is_empty());
    assert!(store.uploads().textures.is_empty());
    store.insert(2, &glyph);
    let second = store.uploads();
    assert!(second
        .textures
        .iter()
        .all(|upload| matches!(upload.op, TextureOp::Write { .. })));
    assert!(!second.textures.is_empty());
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
    fn outline_cache_stays_within_its_budget() {
        let mut fonts = FontManager::new();
        let all = keys(
            "The quick brown fox jumps over the lazy dog 0123456789 ABCDEFGHIJKLMNOPQRSTUVWXYZ",
            32.0,
        );
        let mut unbounded = SlugCache::new();
        all.iter().for_each(|key| {
            unbounded.lookup(&mut fonts, key);
        });
        let budget = unbounded.heap_bytes() / 4;
        let mut bounded = SlugCache::with_budget(budget);
        all.iter().for_each(|key| {
            bounded.lookup(&mut fonts, key);
            assert!(bounded.heap_bytes() <= budget);
        });
        assert!(bounded.len() < unbounded.len());
        assert!(!bounded.is_empty());
    }

    #[test]
    fn uploads_create_once_and_a_warmed_face_never_grows_the_store() {
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
        let revision = fonts.font_revision();
        for key in keys(
            "The quick brown fox jumps over the lazy dog 0123456789 @&%",
            32.0,
        ) {
            assert!(matches!(
                store.resident(&key, revision),
                Some(StoreLookup::Stored(_)) | Some(StoreLookup::Blank)
            ));
        }
        assert!(store.uploads().textures.is_empty());
        assert_ne!(STORE_HANDLE, TextureHandle(0));
    }

    #[test]
    fn resident_lookup_refuses_a_stale_font_revision() {
        let mut fonts = FontManager::new();
        let mut cache = SlugCache::new();
        let mut store = SlugStore::new();
        let key = keys("a", 32.0)[0];
        store.lookup(&mut cache, &mut fonts, &key);
        assert!(store.resident(&key, fonts.font_revision()).is_some());
        assert!(store.resident(&key, fonts.font_revision() + 1).is_none());
    }
}

mod eviction {
    use super::*;
    use aqua_graph::slug::STORE_WIDTH;
    use aqua_protocol::TextureOp;
    use aqua_text::slug::StoreLookup;

    fn stored(lookup: StoreLookup) -> u32 {
        match lookup {
            StoreLookup::Stored(glyph) => glyph.index,
            other => panic!("expected stored glyph, got {other:?}"),
        }
    }

    fn glyph_len() -> usize {
        let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
        pack_glyph(&glyph, 0).len()
    }

    #[test]
    fn least_recently_used_glyphs_make_room_and_current_frame_glyphs_stay() {
        let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
        let len = glyph_len();
        let mut store: SlugStore<u32> = SlugStore::with_budget((len * 3 * 16) as u64);
        let first = stored(store.insert(0, &glyph));
        let second = stored(store.insert(1, &glyph));
        let third = stored(store.insert(2, &glyph));
        assert_eq!([first, second, third], [0, len as u32, 2 * len as u32]);
        store.uploads();
        store.touch(&0);
        store.touch(&2);
        let fourth = stored(store.insert(3, &glyph));
        assert_eq!(fourth, len as u32);
        assert_eq!(store.touch(&1), None);
        assert!(store.touch(&0).is_some() && store.touch(&2).is_some());
        assert_eq!(store.evictions(), 1);
        assert_eq!(store.overruns(), 0);
        assert_eq!(store.live_texels(), 3 * len);
    }

    #[test]
    fn a_full_frame_of_live_glyphs_grows_past_the_budget_and_counts_it() {
        let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
        let len = glyph_len();
        let mut store: SlugStore<u32> = SlugStore::with_budget((len * 2 * 16) as u64);
        (0..4).for_each(|key| {
            store.insert(key, &glyph);
        });
        assert_eq!(store.evictions(), 0);
        assert_eq!(store.overruns(), 2);
        assert_eq!(store.len(), 4);
        store.uploads();
        store.insert(9, &glyph);
        assert_eq!(store.evictions(), 1);
    }

    #[test]
    fn reused_blocks_upload_only_their_rows_and_evicted_glyphs_read_back_exactly() {
        let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
        let len = glyph_len();
        let per_row = STORE_WIDTH as usize / len;
        let count = per_row * 3;
        let mut store: SlugStore<u32> = SlugStore::with_budget((count * len * 16) as u64);
        (0..count as u32).for_each(|key| {
            store.insert(key, &glyph);
        });
        let first = store.uploads();
        assert!(first
            .textures
            .iter()
            .any(|upload| matches!(upload.op, TextureOp::Create { .. })));
        store.uploads();
        (1..count as u32).for_each(|key| {
            store.touch(&key);
        });
        let replaced = stored(store.insert(10_000, &glyph));
        assert_eq!(replaced, 0);
        let batch = store.uploads();
        assert_eq!(batch.textures.len(), 1);
        match &batch.textures[0].op {
            TextureOp::Write { region, .. } => {
                assert_eq!(region.y, 0);
                assert_eq!(region.extent.height, 1);
            }
            other => panic!("expected a row write, got {other:?}"),
        }
    }

    #[test]
    fn shrinking_far_below_the_texture_recreates_it_smaller() {
        let glyph = SlugGlyph::from_path(&rounded_blob()).expect("outline");
        let len = glyph_len();
        let count = STORE_WIDTH as usize * 40 / len;
        let mut store: SlugStore<u32> = SlugStore::with_budget(u64::MAX / 2);
        (0..count as u32).for_each(|key| {
            store.insert(key, &glyph);
        });
        store.uploads();
        let big = store.resident_bytes();
        let mut small: SlugStore<u32> = SlugStore::with_budget((len * 2 * 16) as u64);
        (0..8).for_each(|key| {
            small.insert(key, &glyph);
            small.uploads();
        });
        assert!(small.resident_bytes() < big);
    }
}
