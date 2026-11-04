use aqua_media::atlas::*;

fn glyph(id: u32) -> GlyphKey {
    GlyphKey {
        font: 1,
        glyph: id,
        size_sixty_fourths: 14 * 64,
        subpixel: 0,
        variant: 0,
    }
}

fn small_config() -> AtlasConfig {
    AtlasConfig {
        format: PageFormat::Mask8,
        base_edge: 64,
        max_edge: 256,
        max_shared_pages: 2,
        budget_bytes: 64 * 64 * 2 * 4,
    }
}

fn bitmap(width: u32, height: u32) -> Vec<u8> {
    vec![255; (width * height) as usize]
}

fn placement(slot: Slot) -> Placement {
    match slot {
        Slot::Resident(placement) => placement,
        Slot::Blank { .. } => panic!("expected a resident slot"),
    }
}

fn overlaps(a: &Placement, b: &Placement) -> bool {
    a.page == b.page
        && a.x < b.x + b.width
        && b.x < a.x + a.width
        && a.y < b.y + b.height
        && b.y < a.y + a.height
}

#[test]
fn packed_entries_never_overlap_and_stay_inside_their_page() {
    let mut atlas = Atlas::new(AtlasConfig {
        budget_bytes: 1 << 24,
        max_shared_pages: 8,
        ..small_config()
    });
    let mut placed = Vec::new();
    for id in 0..400u32 {
        let (w, h) = (3 + id % 11, 5 + id % 13);
        let slot = atlas.insert(glyph(id), w, h, [0, 0], bitmap(w, h)).unwrap();
        placed.push(placement(slot));
    }
    for (index, a) in placed.iter().enumerate() {
        assert!(a.x + a.width <= a.page_width && a.y + a.height <= a.page_height);
        assert!(
            placed[index + 1..].iter().all(|b| !overlaps(a, b)),
            "entry {index} overlaps"
        );
    }
}

#[test]
fn hit_returns_the_same_placement_and_does_not_upload_again() {
    let mut atlas = Atlas::new(small_config());
    let first = atlas.insert(glyph(1), 8, 8, [1, -2], bitmap(8, 8)).unwrap();
    let uploads = atlas.take_updates().uploads.len();
    assert_eq!(uploads, 1);
    atlas.begin_frame();
    assert_eq!(atlas.get(&glyph(1)), Some(first));
    assert!(atlas.take_updates().is_empty());
}

#[test]
fn page_sizes_grow_then_pages_are_added_until_the_cap() {
    let mut atlas = Atlas::new(AtlasConfig {
        budget_bytes: 1 << 24,
        max_shared_pages: 4,
        ..small_config()
    });
    let mut edges = Vec::new();
    for id in 0..40u32 {
        atlas
            .insert(glyph(id), 60, 60, [0, 0], bitmap(60, 60))
            .unwrap();
        for created in atlas.take_updates().created {
            edges.push(created.width);
        }
    }
    assert_eq!(&edges[..3], &[64, 128, 256]);
    assert!(edges.iter().all(|edge| edge.is_power_of_two()));
}

#[test]
fn eviction_resets_the_oldest_idle_page_and_never_touches_current_frame_entries() {
    let mut atlas = Atlas::new(AtlasConfig {
        max_shared_pages: 2,
        budget_bytes: 64 * 64 + 128 * 128,
        base_edge: 64,
        ..small_config()
    });
    for id in 0..80u32 {
        atlas
            .insert(glyph(id), 16, 16, [0, 0], bitmap(16, 16))
            .unwrap();
    }
    assert_eq!(atlas.stats().pages, 2);
    atlas.take_updates();
    atlas.begin_frame();
    let survivor = atlas
        .insert(glyph(1000), 16, 16, [0, 0], bitmap(16, 16))
        .unwrap();
    assert!(atlas.get(&glyph(0)).is_none());
    assert_eq!(atlas.get(&glyph(1000)), Some(survivor));
    let stats = atlas.stats();
    assert_eq!(stats.pages, 2);
    assert_eq!(stats.page_evictions, 1);
    assert_eq!(stats.entries, 65);
    assert!(stats.resident_bytes <= 64 * 64 + 128 * 128);
}

#[test]
fn entries_used_this_frame_force_growth_over_budget_instead_of_eviction() {
    let mut atlas = Atlas::new(AtlasConfig {
        max_shared_pages: 1,
        budget_bytes: 64 * 64,
        ..small_config()
    });
    let mut slots = Vec::new();
    for id in 0..40u32 {
        slots.push((
            id,
            atlas
                .insert(glyph(id), 16, 16, [0, 0], bitmap(16, 16))
                .unwrap(),
        ));
    }
    let stats = atlas.stats();
    assert!(stats.over_budget_pages >= 1);
    for (id, slot) in slots {
        assert_eq!(atlas.get(&glyph(id)), Some(slot));
    }
}

#[test]
fn large_images_get_dedicated_pages_and_release_returns_the_memory() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig {
        format: PageFormat::Color8,
        base_edge: 64,
        max_edge: 512,
        max_shared_pages: 2,
        budget_bytes: 1 << 24,
    });
    let key = ImageKey {
        asset: 7,
        frame: 0,
        width: 300,
        height: 200,
    };
    let slot = placement(
        atlas
            .insert(key, 300, 200, [0, 0], vec![0; 300 * 200 * 4])
            .unwrap(),
    );
    assert_eq!(
        (slot.x, slot.y, slot.page_width, slot.page_height),
        (0, 0, 300, 200)
    );
    assert_eq!(atlas.resident_bytes(), 300 * 200 * 4);
    atlas.take_updates();
    assert!(atlas.remove(&key));
    assert_eq!(atlas.resident_bytes(), 0);
    assert_eq!(atlas.take_updates().released, vec![slot.page]);
}

#[test]
fn image_budget_evicts_least_recently_used_dedicated_pages() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig {
        format: PageFormat::Color8,
        base_edge: 64,
        max_edge: 512,
        max_shared_pages: 2,
        budget_bytes: 2 * 200 * 200 * 4 + 100,
    });
    let key = |asset| ImageKey {
        asset,
        frame: 0,
        width: 200,
        height: 200,
    };
    for asset in 0..2 {
        atlas
            .insert(key(asset), 200, 200, [0, 0], vec![0; 200 * 200 * 4])
            .unwrap();
        atlas.begin_frame();
    }
    atlas.get(&key(0));
    atlas.begin_frame();
    atlas
        .insert(key(2), 200, 200, [0, 0], vec![0; 200 * 200 * 4])
        .unwrap();
    assert!(atlas.get(&key(1)).is_none());
    assert!(atlas.get(&key(0)).is_some());
    assert!(atlas.get(&key(2)).is_some());
    assert!(atlas.resident_bytes() <= 2 * 200 * 200 * 4 + 100);
}

#[test]
fn oversized_and_malformed_inserts_are_rejected() {
    let mut atlas = Atlas::new(small_config());
    assert!(matches!(
        atlas.insert(glyph(1), 300, 4, [0, 0], bitmap(300, 4)),
        Err(AtlasError::TooLarge { .. })
    ));
    assert!(matches!(
        atlas.insert(glyph(2), 4, 4, [0, 0], vec![0; 3]),
        Err(AtlasError::DataSize {
            expected: 16,
            actual: 3
        })
    ));
}

#[test]
fn trim_idle_releases_pages_not_used_recently_and_keeps_active_ones() {
    let mut atlas = Atlas::new(AtlasConfig {
        budget_bytes: 1 << 24,
        max_shared_pages: 4,
        ..small_config()
    });
    for id in 0..3u32 {
        atlas
            .insert(glyph(id), 60, 60, [0, 0], bitmap(60, 60))
            .unwrap();
    }
    atlas.take_updates();
    let pages_before = atlas.stats().pages;
    assert!(pages_before >= 2);
    for _ in 0..10 {
        atlas.begin_frame();
        atlas.get(&glyph(2));
    }
    let released = atlas.trim_idle(5);
    assert_eq!(released as u32, pages_before - 1);
    assert_eq!(atlas.take_updates().released.len(), released);
    assert!(atlas.get(&glyph(2)).is_some());
    assert!(atlas.get(&glyph(0)).is_none());
}

#[test]
fn page_created_and_released_in_one_frame_cancels_out() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig {
        format: PageFormat::Color8,
        base_edge: 64,
        max_edge: 512,
        max_shared_pages: 2,
        budget_bytes: 1 << 24,
    });
    let key = ImageKey {
        asset: 1,
        frame: 0,
        width: 300,
        height: 300,
    };
    atlas
        .insert(key, 300, 300, [0, 0], vec![0; 300 * 300 * 4])
        .unwrap();
    atlas.remove(&key);
    assert!(atlas.take_updates().is_empty());
}

#[test]
fn glyph_resolution_rasterizes_once_and_caches_blank_glyphs() {
    let mut atlas = Atlas::new(AtlasConfig {
        budget_bytes: 1 << 24,
        ..small_config()
    });
    let mut calls = 0;
    let mut rasterizer = |key: &GlyphKey| {
        calls += 1;
        match key.glyph {
            32 => None,
            _ => Some(Raster {
                width: 6,
                height: 9,
                bearing: [1, 8],
                data: vec![200; 54],
            }),
        }
    };
    let letter = atlas.resolve(glyph(65), &mut rasterizer).unwrap();
    assert_eq!(atlas.resolve(glyph(65), &mut rasterizer).unwrap(), letter);
    assert!(matches!(
        atlas.resolve(glyph(32), &mut rasterizer).unwrap(),
        Slot::Blank { .. }
    ));
    assert!(matches!(
        atlas.resolve(glyph(32), &mut rasterizer).unwrap(),
        Slot::Blank { .. }
    ));
    assert_eq!(calls, 2);
    assert_eq!(placement(letter).bearing, [1, 8]);
}

#[test]
fn subpixel_bins_round_to_quarter_pixels_and_carry_into_the_integer_part() {
    assert_eq!(subpixel_bin(10.0), (10, 0));
    assert_eq!(subpixel_bin(10.3), (10, 1));
    assert_eq!(subpixel_bin(10.5), (10, 2));
    assert_eq!(subpixel_bin(10.9), (11, 0));
    assert_eq!(subpixel_bin(-0.1), (0, 0));
    assert_eq!(subpixel_bin(-0.3), (-1, 3));
}

#[test]
fn streaming_splits_large_uploads_into_bands_within_the_frame_budget() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig {
        format: PageFormat::Color8,
        base_edge: 64,
        max_edge: 512,
        max_shared_pages: 2,
        budget_bytes: 1 << 24,
    });
    let key = ImageKey {
        asset: 1,
        frame: 0,
        width: 256,
        height: 256,
    };
    let pixels: Vec<u8> = (0..256 * 256 * 4).map(|i| (i % 251) as u8).collect();
    let slot = placement(atlas.insert(key, 256, 256, [0, 0], pixels.clone()).unwrap());
    let mut streamer = Streamer::new();
    let budget = StreamBudget {
        bytes_per_frame: 64 * 1024,
    };
    let mut gathered = vec![0u8; pixels.len()];
    let mut frames = 0;
    let mut first = streamer.submit(atlas.take_updates(), budget);
    loop {
        frames += 1;
        assert!(first.upload_bytes() <= budget.bytes_per_frame);
        for upload in first.uploads.drain(..) {
            let start = upload.y as usize * 256 * 4;
            gathered[start..start + upload.data.len()].copy_from_slice(&upload.data);
        }
        if streamer.is_idle() {
            break;
        }
        assert!(streamer.is_pending(slot.page));
        first = streamer.submit(Default::default(), budget);
    }
    assert_eq!(frames, 4);
    assert_eq!(gathered, pixels);
}

#[test]
fn streaming_drops_bands_for_pages_released_before_they_were_sent() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig {
        format: PageFormat::Color8,
        base_edge: 64,
        max_edge: 512,
        max_shared_pages: 2,
        budget_bytes: 1 << 24,
    });
    let key = ImageKey {
        asset: 1,
        frame: 0,
        width: 256,
        height: 256,
    };
    atlas
        .insert(key, 256, 256, [0, 0], vec![1; 256 * 256 * 4])
        .unwrap();
    let mut streamer = Streamer::new();
    let budget = StreamBudget {
        bytes_per_frame: 64 * 1024,
    };
    let sent = streamer.submit(atlas.take_updates(), budget);
    assert_eq!(sent.created.len(), 1);
    atlas.remove(&key);
    let after = streamer.submit(atlas.take_updates(), budget);
    assert_eq!(after.released.len(), 1);
    assert!(after.uploads.is_empty());
    assert!(streamer.is_idle());
}

#[test]
fn editor_sized_color_atlas_keeps_shared_pages_small_and_large_items_exact() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig::images());
    let wide = ImageKey {
        asset: 1,
        frame: 0,
        width: 1016,
        height: 251,
    };
    let slot = placement(
        atlas
            .insert(wide, 1016, 251, [0, 0], vec![0; 1016 * 251 * 4])
            .unwrap(),
    );
    assert_eq!((slot.page_width, slot.page_height), (1016, 251));
    for asset in 10..80u64 {
        let key = ImageKey {
            asset,
            frame: 0,
            width: 400,
            height: 300,
        };
        atlas
            .insert(key, 400, 300, [0, 0], vec![0; 400 * 300 * 4])
            .unwrap();
    }
    let shared: Vec<_> = atlas
        .page_layout()
        .into_iter()
        .filter(|page| page.shared)
        .collect();
    assert!(shared.iter().all(|page| page.width <= 512));
}

#[test]
fn small_icons_open_a_small_first_page() {
    let mut atlas: Atlas<ImageKey> = Atlas::new(AtlasConfig::images());
    for asset in 0..20u64 {
        let key = ImageKey {
            asset,
            frame: 0,
            width: 22,
            height: 22,
        };
        atlas.insert(key, 22, 22, [0, 0], vec![0; 22 * 22 * 4]).unwrap();
    }
    assert_eq!(atlas.resident_bytes(), 256 * 256 * 4);
}
