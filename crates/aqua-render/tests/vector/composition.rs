use aqua_core::{Oklch, Rect};
use aqua_render::{DisplayItem, RenderError, Renderer, Scene, VectorBudget, VectorSource};

#[test]
fn svg_composition_respects_nested_round_clips_opacity_and_cache_bounds() {
    let mut renderer = match pollster::block_on(Renderer::headless(96, 96)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").as_deref() != Ok("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER");
            return;
        }
        Err(error) => panic!("{error}"),
    };
    renderer.set_vector_budget(VectorBudget {
        bytes: 96 * 96 * 4,
        entries: 1,
        ..Default::default()
    });
    let source = VectorSource::svg(
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="96" height="96"><path fill="red" d="M0 0H96V96H0Z"/></svg>"#,
    );
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Rect {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 96.0,
            h: 96.0,
        },
        radius: 0.0,
        fill: Oklch::from_hex("#0000ff").unwrap(),
        border_width: 0.0,
        border_color: Oklch::WHITE,
        shadow: None,
    });
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 8.0,
            y: 8.0,
            w: 64.0,
            h: 64.0,
        },
        radius: 16.0,
    });
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 16.0,
            y: 0.0,
            w: 72.0,
            h: 80.0,
        },
        radius: 0.0,
    });
    scene.insert(DisplayItem::PushOpacity(0.5));
    let id = scene.insert(DisplayItem::Vector {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 96.0,
            h: 96.0,
        },
        source: source.clone(),
        opacity: 1.0,
    });
    scene.insert(DisplayItem::PopOpacity);
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 96 + x) * 4..][..4];
    assert_eq!(pixel(4, 40), &[0, 0, 255, 255]);
    assert_eq!(pixel(80, 40), &[0, 0, 255, 255]);
    let middle = pixel(40, 40);
    assert!(
        (middle[0] as i16 - 128).abs() <= 2 && (middle[2] as i16 - 128).abs() <= 2,
        "isolated vector over blue: {middle:?}"
    );
    assert!(pixel(16, 8)[0] < 30, "ancestor rounded clip remains active");
    assert_eq!(renderer.vector_stats().rasterizations, 1);
    scene.update(
        id,
        DisplayItem::Vector {
            bounds: Rect {
                x: 0.0,
                y: 0.0,
                w: 96.0,
                h: 96.0,
            },
            source,
            opacity: 0.8,
        },
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert_eq!(renderer.vector_stats().rasterizations, 1);
    assert_eq!(renderer.vector_stats().cache_hits, 1);
    assert!(renderer.vector_stats().bytes <= 96 * 96 * 4);
}

fn renderer(width: u32, height: u32) -> Option<Renderer> {
    match pollster::block_on(Renderer::headless(width, height)) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").as_deref() != Ok("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

fn rectangle(bounds: Rect, color: &str) -> DisplayItem {
    DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill: Oklch::from_hex(color).unwrap(),
        border_width: 0.0,
        border_color: Oklch::WHITE,
        shadow: None,
    }
}

#[test]
fn nested_rectangle_clips_survive_fast_updates_and_transforms_preserve_parent_clip() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    let mut scene = Scene::new();
    scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        },
        "#0000ff",
    ));
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 8.0,
            y: 8.0,
            w: 48.0,
            h: 48.0,
        },
        radius: 14.0,
    });
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 16.0,
            y: 0.0,
            w: 48.0,
            h: 64.0,
        },
        radius: 0.0,
    });
    let rect = scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        },
        "#ff0000",
    ));
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::PopClip);
    for color in ["#ff0000", "#00ff00"] {
        scene.update(
            rect,
            rectangle(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 64.0,
                    h: 64.0,
                },
                color,
            ),
        );
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).unwrap();
        let pixels = renderer.readback().unwrap();
        assert!(
            pixels[(8 * 64 + 16) * 4] < 60 && pixels[(8 * 64 + 16) * 4 + 1] < 60,
            "ancestor rounded mask remains active"
        );
        assert_eq!(&pixels[(32 * 64 + 12) * 4..][..4], &[0, 0, 255, 255]);
        assert_eq!(
            &pixels[(32 * 64 + 32) * 4..][..4],
            &Oklch::from_hex(color).unwrap().to_srgb8()
        );
    }
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 0.0,
            y: 0.0,
            w: 40.0,
            h: 64.0,
        },
        radius: 0.0,
    });
    scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::scale(2.0, 2.0).then(aqua_core::Transform2D::translate(20.0, 10.0)),
    ));
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 16.0,
            h: 16.0,
        },
        "#ff0000",
    ));
    scene.insert(DisplayItem::PopOpacity);
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(&pixels[(20 * 64 + 10) * 4..][..4], &[0, 0, 0, 0]);
    assert_eq!(&pixels[(20 * 64 + 44) * 4..][..4], &[0, 0, 0, 0]);
    assert!((pixels[(20 * 64 + 30) * 4] as i16 - 128).abs() <= 1);
    let mut invalid = Scene::new();
    invalid.insert(DisplayItem::PushTransform(aqua_core::Transform2D::scale(
        2.0, 1.0,
    )));
    invalid.insert(DisplayItem::PopTransform);
    let plan = renderer.prepare(&mut invalid);
    assert!(renderer.draw(&plan).is_err());
}

#[test]
fn content_blur_replaces_sharp_source_and_obeys_ancestor_clip() {
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: Rect {
            x: 16.0,
            y: 0.0,
            w: 32.0,
            h: 64.0,
        },
        radius: 0.0,
    });
    scene.insert(DisplayItem::PushContentBlur {
        bounds: Rect {
            x: 16.0,
            y: 16.0,
            w: 32.0,
            h: 32.0,
        },
        radius: 6.0,
    });
    scene.insert(rectangle(
        Rect {
            x: 30.0,
            y: 20.0,
            w: 4.0,
            h: 24.0,
        },
        "#ff0000",
    ));
    scene.insert(DisplayItem::PopContentBlur);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let alpha = |x: usize, y: usize| pixels[(y * 64 + x) * 4 + 3];
    assert!(
        alpha(32, 32) > 20 && alpha(32, 32) < 220,
        "sharp source removed: {}",
        alpha(32, 32)
    );
    assert!(alpha(27, 32) > 0, "blur spreads beyond source");
    assert_eq!(alpha(14, 32), 0);
    assert_eq!(alpha(50, 32), 0);
}

#[test]
fn image_fitting_and_source_crop_compose_inside_opacity_group() {
    use aqua_core::{ImageId, Point, RawId};
    use aqua_render::media::{DecodedImage, GpuImageCache, GpuMediaBudget};
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    let mut rgba = Vec::new();
    for _y in 0..8 {
        for x in 0..16 {
            rgba.extend(if x < 8 {
                [255, 0, 0, 255]
            } else {
                [0, 255, 0, 255]
            });
        }
    }
    let image = DecodedImage::from_rgba(16, 8, rgba).unwrap();
    let mut cache = GpuImageCache::new(GpuMediaBudget::default());
    let uploaded = cache
        .upload(
            renderer.device(),
            renderer.queue(),
            ImageId::from_raw(RawId {
                index: 0,
                generation: 0,
            }),
            &image,
        )
        .unwrap();
    renderer.register_external(7, uploaded.view.clone(), 16, 8);
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(DisplayItem::Image {
        bounds: Rect {
            x: 8.0,
            y: 8.0,
            w: 48.0,
            h: 48.0,
        },
        layer: 7,
        fit: aqua_render::ImageFit::Contain,
        position: Point::new(0.5, 0.5),
        source: Rect {
            x: 0.5,
            y: 0.0,
            w: 0.5,
            h: 1.0,
        },
        opacity: 1.0,
        rounded_clip: 8.0,
    });
    scene.insert(DisplayItem::PopOpacity);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let middle = &pixels[(32 * 64 + 32) * 4..][..4];
    assert!(
        middle[0] <= 1
            && (middle[1] as i16 - 128).abs() <= 1
            && (middle[3] as i16 - 128).abs() <= 1,
        "cropped green: {middle:?}"
    );
    assert_eq!(&pixels[(8 * 64 + 8) * 4..][..4], &[0, 0, 0, 0]);
}

#[test]
fn snapshot_transition_sweeps_old_frame_without_mutating_new_base() {
    use aqua_render::effects::SnapshotTransition;
    use std::time::Duration;
    let Some(mut renderer) = renderer(64, 64) else {
        return;
    };
    let mut scene = Scene::new();
    let rect = scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        },
        "#ff0000",
    ));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    renderer
        .start_snapshot_transition(SnapshotTransition {
            duration: Duration::from_secs(1),
            band: 0.18,
            shimmer: Oklch::WHITE.with_alpha(0.0),
        })
        .unwrap();
    scene.update(
        rect,
        rectangle(
            Rect {
                x: 0.0,
                y: 0.0,
                w: 64.0,
                h: 64.0,
            },
            "#0000ff",
        ),
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert_eq!(&renderer.readback().unwrap()[..4], &[255, 0, 0, 255]);
    assert!(renderer.set_snapshot_transition_elapsed(Duration::from_millis(500)));
    let pixels = renderer.readback().unwrap();
    assert_eq!(&pixels[(8 * 64 + 8) * 4..][..4], &[0, 0, 255, 255]);
    assert_eq!(&pixels[(56 * 64 + 56) * 4..][..4], &[255, 0, 0, 255]);
    assert!(!renderer.set_snapshot_transition_elapsed(Duration::from_secs(1)));
    let pixels = renderer.readback().unwrap();
    assert!(pixels
        .as_chunks::<4>()
        .0
        .iter()
        .all(|pixel| pixel[0] == 0 && pixel[1] == 0 && pixel[2] == pixel[3]));
}

#[test]
fn text_color_map_changes_only_glyph_rgb_and_preserves_alpha() {
    use aqua_render::text::{GlyphonProvider, TextBackend};
    use aqua_text::{SharedFonts, SharedRuns, TextLayout, TextStyle};
    let Some(mut renderer) = renderer(128, 48) else {
        return;
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "HHHHHH",
        &TextStyle {
            size_px: 24.0,
            line_height_px: 28.0,
            ..Default::default()
        },
    );
    layout.layout(&mut fonts.lock());
    let (id, revision) = runs.lock().insert(layout);
    renderer.set_text_provider(Box::new(GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        TextBackend::new(fonts, runs),
    )));
    let mut scene = Scene::new();
    let map = scene.insert(DisplayItem::PushTextColorMap(
        aqua_core::scene::TextColorMap {
            start: 0.0,
            end: 128.0,
            target: Oklch::from_hex("#0000ff").unwrap(),
            strength: 0.0,
        },
    ));
    scene.insert(DisplayItem::Text {
        at: [8.0, 8.0],
        run: aqua_core::scene::TextRun { id: id.0, revision },
        color: Oklch::from_hex("#ff0000").unwrap(),
        opacity: 0.5,
    });
    scene.insert(DisplayItem::PopTextColorMap);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let before = renderer.readback().unwrap();
    scene.update(
        map,
        DisplayItem::PushTextColorMap(aqua_core::scene::TextColorMap {
            start: 0.0,
            end: 128.0,
            target: Oklch::from_hex("#0000ff").unwrap(),
            strength: 1.0,
        }),
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let after = renderer.readback().unwrap();
    let mut changed = 0;
    for (original, mapped) in before
        .as_chunks::<4>()
        .0
        .iter()
        .zip(after.as_chunks::<4>().0.iter())
    {
        assert!((original[3] as i16 - mapped[3] as i16).abs() <= 1);
        if mapped[3] > 20 && mapped[2] > original[2] + 10 {
            changed += 1;
        }
    }
    assert!(changed > 40, "mapped visible glyph coverage: {changed}");
}

#[test]
fn raster_modes_invalidate_atlas_and_preserve_shaping_through_roundtrip() {
    use aqua_core::scene::TextRasterMode;
    use aqua_render::text::{GlyphonProvider, TextBackend};
    use aqua_text::{SharedFonts, SharedRuns, TextLayout, TextStyle};
    let Some(mut renderer) = renderer(256, 64) else {
        return;
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "Hamburgefons office ffi",
        &TextStyle {
            size_px: 15.3,
            line_height_px: 24.0,
            ..Default::default()
        },
    );
    layout.layout(&mut fonts.lock());
    let (id, revision) = runs.lock().insert(layout);
    renderer.set_text_provider(Box::new(GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        TextBackend::new(fonts, runs.clone()),
    )));
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Text {
        at: [8.25, 8.0],
        run: aqua_core::scene::TextRun { id: id.0, revision },
        color: Oklch::WHITE,
        opacity: 1.0,
    });
    let mut images = Vec::new();
    for mode in [
        TextRasterMode::Outline,
        TextRasterMode::PlatformDefault,
        TextRasterMode::Outline,
    ] {
        renderer.set_text_raster_mode(mode).unwrap();
        let plan = renderer.prepare(&mut scene);
        assert!(!plan.is_clean());
        renderer.draw(&plan).unwrap();
        images.push(renderer.readback().unwrap());
        assert!(runs.lock().resolve_expected(id, revision).is_ok());
    }
    assert_eq!(images[0], images[2]);
    assert_ne!(
        images[0], images[1],
        "native/hinted raster must differ from unhinted outlines"
    );
    assert!(images.iter().all(|image| image
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[3] > 8)
        .count()
        > 60));
}

#[test]
fn separated_text_chunks_keep_distinct_vertex_buffers_when_later_chunk_grows() {
    use aqua_render::text::{GlyphonProvider, TextBackend};
    use aqua_text::{SharedFonts, SharedRuns, TextLayout, TextStyle};
    let Some(mut renderer) = renderer(320, 256) else {
        return;
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut registered = Vec::new();
    for text in ["HH".to_owned(), "W".repeat(300), "HH".to_owned()] {
        let mut layout = TextLayout::new();
        layout.set_text(
            &text,
            &TextStyle {
                size_px: 12.0,
                line_height_px: 14.0,
                ..Default::default()
            },
        );
        layout.set_max_width(Some(160.0));
        layout.layout(&mut fonts.lock());
        registered.push(runs.lock().insert(layout));
    }
    renderer.set_text_provider(Box::new(GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        TextBackend::new(fonts, runs),
    )));
    let mut scene = Scene::new();
    for (index, (id, revision)) in registered.into_iter().enumerate() {
        scene.insert(DisplayItem::PushOpacity(0.8));
        scene.insert(DisplayItem::Text {
            at: [8.0 + index as f32 * 128.0, 8.0],
            run: aqua_core::scene::TextRun { id: id.0, revision },
            color: Oklch::from_hex(["#ff0000", "#00ff00", "#0000ff"][index]).unwrap(),
            opacity: 1.0,
        });
        scene.insert(DisplayItem::PopOpacity);
    }
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let image = renderer.readback().unwrap();
    for (channel, x_start) in [(0, 8), (1, 136), (2, 264)] {
        let lit = (8..32)
            .flat_map(|y| (x_start..x_start + 32).map(move |x| (y * 320 + x) * 4))
            .filter(|offset| image[*offset + channel] > 20)
            .count();
        assert!(
            lit > 35,
            "text chunk {channel} remains distinct after later vertex growth: {lit}"
        );
    }
}

#[test]
fn resizing_unchanged_scene_rebuilds_pixels_and_duplicate_size_preserves_snapshot() {
    use aqua_render::effects::SnapshotTransition;
    use std::time::Duration;
    let Some(mut renderer) = renderer(32, 32) else {
        return;
    };
    let mut scene = Scene::new();
    scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: 64.0,
            h: 64.0,
        },
        "#ff0000",
    ));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    renderer.set_size(64, 64, 1.0).unwrap();
    let plan = renderer.prepare(&mut scene);
    assert!(
        !plan.is_clean(),
        "replaced base must repaint unchanged scene"
    );
    renderer.draw(&plan).unwrap();
    assert_eq!(
        &renderer.readback().unwrap()[(32 * 64 + 32) * 4..][..4],
        &[255, 0, 0, 255]
    );
    renderer
        .start_snapshot_transition(SnapshotTransition {
            duration: Duration::from_secs(1),
            band: 0.18,
            shimmer: Oklch::WHITE,
        })
        .unwrap();
    renderer.set_size(64, 64, 1.0).unwrap();
    assert!(
        renderer.snapshot_transition_active(),
        "duplicate native resize preserves active snapshot"
    );
    let plan = renderer.prepare(&mut scene);
    assert!(plan.is_clean());
}

#[test]
fn replacing_gpu_image_at_capacity_preserves_other_resident_views() {
    use aqua_core::{ImageId, RawId};
    use aqua_render::media::{DecodedImage, GpuImageCache, GpuMediaBudget};
    let Some(renderer) = renderer(32, 32) else {
        return;
    };
    let mut cache = GpuImageCache::new(GpuMediaBudget {
        max_gpu_bytes: 1024,
        max_textures: 2,
    });
    let first = ImageId::from_raw(RawId {
        index: 0,
        generation: 0,
    });
    let second = ImageId::from_raw(RawId {
        index: 1,
        generation: 0,
    });
    let image = |color: [u8; 4]| DecodedImage::from_rgba(2, 2, color.repeat(4)).unwrap();
    cache
        .upload(
            renderer.device(),
            renderer.queue(),
            first,
            &image([255, 0, 0, 255]),
        )
        .unwrap();
    cache
        .upload(
            renderer.device(),
            renderer.queue(),
            second,
            &image([0, 0, 255, 255]),
        )
        .unwrap();
    cache
        .upload(
            renderer.device(),
            renderer.queue(),
            first,
            &image([0, 255, 0, 255]),
        )
        .unwrap();
    assert!(cache.view(first).is_some() && cache.view(second).is_some());
    assert_eq!(cache.len(), 2);
    assert_eq!(cache.resident_bytes(), 1024);
    assert_eq!(cache.evictions, 0);
}
