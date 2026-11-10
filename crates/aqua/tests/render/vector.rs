use std::sync::Arc;

use aqua::render::vector::{gradient_axis, rotated_svg, rotation_steps, VectorCache};
use aqua_core::geometry::Rect;
use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
use aqua_core::{GpuiBackground, GpuiHsla, Point, Transform2D};

#[test]
fn rotation_quantizes_to_whole_steps_and_wraps() {
    assert_eq!(rotation_steps(0.0), 0);
    assert_eq!(rotation_steps(std::f32::consts::TAU), 0);
    assert_eq!(rotation_steps(std::f32::consts::FRAC_PI_2), 30);
    assert_eq!(rotation_steps(-std::f32::consts::FRAC_PI_2), 90);
    assert_eq!(rotation_steps(f32::NAN), 0);
}

#[test]
fn rotated_svg_wraps_the_original_around_its_view_box_centre() {
    let source = "<?xml version=\"1.0\"?><svg viewBox=\"0 0 16 8\" xmlns=\"http://www.w3.org/2000/svg\"><rect width=\"4\" height=\"4\"/></svg>";
    let wrapped = rotated_svg(source, 30);
    assert!(wrapped.contains("rotate(90 8 4)"));
    assert!(wrapped.contains("viewBox=\"0 0 16 8\""));
    assert!(wrapped.contains("<rect"));
    assert!(!wrapped.contains("<?xml"));
}

#[test]
fn rotated_svg_falls_back_to_width_and_height() {
    let wrapped = rotated_svg("<svg width=\"20px\" height=\"10\"></svg>", 60);
    assert!(wrapped.contains("rotate(180 10 5)"));
}

#[test]
fn gradient_axes_follow_css_angles() {
    let bounds = Rect {
        x: 0.0,
        y: 0.0,
        w: 100.0,
        h: 50.0,
    };
    let (start, end) = gradient_axis(bounds, 0.0);
    assert!((start.x - 50.0).abs() < 1e-3 && (start.y - 50.0).abs() < 1e-3);
    assert!((end.x - 50.0).abs() < 1e-3 && end.y.abs() < 1e-3);
    let (start, end) = gradient_axis(bounds, 90.0);
    assert!(start.x.abs() < 1e-3 && (end.x - 100.0).abs() < 1e-3);
}

fn triangle(origin: Point) -> GpuiPath {
    GpuiPath {
        commands_local: vec![
            PathCommand::Move(Point::new(0.0, 0.0)),
            PathCommand::Line(Point::new(10.0, 0.0)),
            PathCommand::Line(Point::new(5.0, 8.0)),
            PathCommand::Close,
        ],
        origin_window_logical: origin,
        element_transform: Transform2D::IDENTITY,
        style: GpuiPathStyle::Fill {
            rule: FillRule::NonZero,
        },
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
    }
}

fn drawing_of(raster: &aqua::render::vector::PathRaster) -> Arc<aqua_core::vector::Drawing> {
    match &raster.source {
        aqua_core::vector::VectorSource::Drawing(drawing) => drawing.clone(),
        aqua_core::vector::VectorSource::Svg(_) => panic!("path rasters are drawings"),
    }
}

#[test]
fn translating_a_path_by_whole_pixels_reuses_the_same_drawing() {
    let mut cache = VectorCache::default();
    let device = Transform2D::scale(2.0, 2.0);
    let first = cache
        .path(&triangle(Point::new(10.0, 10.0)), device)
        .unwrap();
    let moved = cache
        .path(&triangle(Point::new(40.0, 25.0)), device)
        .unwrap();
    assert!(Arc::ptr_eq(&drawing_of(&first), &drawing_of(&moved)));
    assert_eq!(moved.placement.x - first.placement.x, 60.0);
    assert_eq!(moved.placement.y - first.placement.y, 30.0);
}

#[test]
fn subpixel_translation_gets_its_own_drawing() {
    let mut cache = VectorCache::default();
    let device = Transform2D::IDENTITY;
    let first = cache
        .path(&triangle(Point::new(10.0, 10.0)), device)
        .unwrap();
    let shifted = cache
        .path(&triangle(Point::new(10.5, 10.0)), device)
        .unwrap();
    assert!(!Arc::ptr_eq(&drawing_of(&first), &drawing_of(&shifted)));
}

#[test]
fn mixed_path_bounds_include_controls_and_scaled_arc_reach() {
    let mut path = triangle(Point::new(3.25, 7.5));
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(-2.0, 3.0)),
        PathCommand::Quadratic {
            control: Point::new(-8.0, -4.0),
            to: Point::new(6.0, 2.0),
        },
        PathCommand::Cubic {
            first: Point::new(20.0, -6.0),
            second: Point::new(9.0, 18.0),
            to: Point::new(5.0, 5.0),
        },
        PathCommand::Arc {
            radii: Point::new(2.0, 4.0),
            rotation_radians: 0.0,
            large: false,
            sweep: true,
            to: Point::new(12.0, 10.0),
        },
        PathCommand::Close,
    ];
    let mut cache = VectorCache::default();
    let raster = cache.path(&path, Transform2D::scale(2.0, 2.0)).unwrap();
    assert_eq!(
        raster.placement,
        Rect {
            x: -19.0,
            y: -6.0,
            w: 75.0,
            h: 66.0
        }
    );
    assert_eq!((raster.width, raster.height), (75, 66));
    let drawing = drawing_of(&raster);
    assert_eq!(
        drawing.paths[0].commands[0],
        PathCommand::Move(Point::new(25.5, 21.0))
    );
    assert_eq!(
        drawing.paths[0].commands[4],
        PathCommand::Arc {
            radii: Point::new(4.0, 8.0),
            rotation_radians: 0.0,
            large: false,
            sweep: true,
            to: Point::new(49.5, 41.0),
        }
    );
}

#[test]
fn svg_sources_are_shared_per_tint_and_rotation() {
    let mut cache = VectorCache::default();
    let svg: Arc<str> = "<svg viewBox=\"0 0 4 4\"></svg>".into();
    let plain = cache.svg(&svg, None, 0);
    assert_eq!(plain, cache.svg(&svg, None, 0));
    assert_ne!(plain, cache.svg(&svg, None, 5));
}

#[test]
fn large_paths_survive_monitor_scale_round_trips() {
    use aqua::render::lower::Lowering;
    use aqua_core::{DisplayItem, Size};
    use aqua_protocol::Instant;
    use aqua_scene::Scene;
    use aqua_text::{SharedFonts, SharedRuns};

    let bounds = Rect::from_edges(0.0, 0.0, 1500.0, 900.0);
    let mut path = triangle(Point::ZERO);
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 900.0)),
        PathCommand::Line(Point::new(0.0, 900.0)),
        PathCommand::Close,
    ];
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiPath {
        transformed_window_logical_bounds: bounds,
        path: Arc::new(path),
    });
    let mut lowering = Lowering::new(SharedFonts::new(), SharedRuns::new());
    for scale in [1.0, 2.0, 1.25, 2.0, 1.0] {
        let (commands, _, report) =
            lowering.commands(&scene, Size::new(1500.0, 900.0), scale, Instant::ZERO);
        assert!(
            report.skipped.is_empty(),
            "a large path must remain visible at {scale}x: {:?}",
            report.skipped
        );
        assert!(!commands.is_empty(), "the path must reach the painter");
        lowering.retain_frame(commands);
    }
}

#[test]
fn tiled_path_gradients_match_the_unsplit_raster_at_tile_boundaries() {
    use aqua_core::{GpuiGradientColorSpace, GpuiGradientStop};
    use aqua_media::vector::{rasterize, VectorBudget};

    let mut path = triangle(Point::new(20.0, 12.0));
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 900.0)),
        PathCommand::Line(Point::new(0.0, 900.0)),
        PathCommand::Close,
    ];
    path.background = GpuiBackground::LinearGradient {
        angle: 145.0,
        color_space: GpuiGradientColorSpace::Srgb,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: GpuiHsla {
                    h: 0.0,
                    s: 1.0,
                    l: 0.5,
                    a: 1.0,
                },
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: GpuiHsla {
                    h: 0.6,
                    s: 1.0,
                    l: 0.5,
                    a: 1.0,
                },
            },
        ],
    };
    let raster = VectorCache::default()
        .path(&path, Transform2D::scale(2.0, 2.0))
        .unwrap();
    let origin = raster.placement;
    let reference = rasterize(
        &raster.source,
        raster.width,
        raster.height,
        VectorBudget {
            max_pixels: 8 * 1024 * 1024,
            ..VectorBudget::default()
        },
    )
    .unwrap();
    for tile in raster.tiles() {
        let pixels = rasterize(
            &tile.source,
            tile.width,
            tile.height,
            VectorBudget::default(),
        )
        .unwrap();
        let left = (tile.placement.x - origin.x) as u32;
        let top = (tile.placement.y - origin.y) as u32;
        for y in (0..tile.height).step_by(37) {
            for x in [0, 1, tile.width / 2, tile.width - 2, tile.width - 1] {
                let actual = ((y * tile.width + x) * 4) as usize;
                let expected = (((top + y) * reference.width + left + x) * 4) as usize;
                assert!(
                    (0..4).all(|c| pixels.pixels[actual + c]
                        .abs_diff(reference.pixels[expected + c])
                        <= 1),
                    "tile seams must preserve the same gradient and alpha at {}, {}",
                    left + x,
                    top + y
                );
            }
        }
    }
}

#[test]
fn tile_sources_are_reused_while_placements_follow_translated_paths() {
    use aqua_core::vector::VectorSource;
    let mut path = triangle(Point::ZERO);
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(3000.0, 0.0)),
        PathCommand::Line(Point::new(3000.0, 1800.0)),
        PathCommand::Close,
    ];
    let mut cache = VectorCache::default();
    let first = cache.path(&path, Transform2D::IDENTITY).unwrap();
    let first = cache.tiles(first);
    path.origin_window_logical = Point::new(25.0, 40.0);
    let later = cache.path(&path, Transform2D::IDENTITY).unwrap();
    let later = cache.tiles(later);
    assert_eq!(first.len(), later.len());
    for (a, b) in first.iter().zip(&later) {
        let (VectorSource::Drawing(a_source), VectorSource::Drawing(b_source)) =
            (&a.source, &b.source)
        else {
            panic!("path tiles must remain drawings");
        };
        assert!(
            Arc::ptr_eq(a_source, b_source),
            "tile residency must survive redraws"
        );
        assert_eq!(b.placement, a.placement.translate(25.0, 40.0));
    }
}

#[test]
fn tile_cache_evicts_old_geometry_during_repeated_resizing_and_releases_it_on_trim() {
    use aqua_core::vector::VectorSource;
    let mut path = triangle(Point::ZERO);
    let mut cache = VectorCache::default();
    let mut first = None;
    for width in 3000..3064 {
        path.commands_local = vec![
            PathCommand::Move(Point::ZERO),
            PathCommand::Line(Point::new(width as f32, 0.0)),
            PathCommand::Line(Point::new(width as f32, 1800.0)),
            PathCommand::Close,
        ];
        let raster = cache.path(&path, Transform2D::IDENTITY).unwrap();
        let tiles = cache.tiles(raster);
        if first.is_none() {
            let VectorSource::Drawing(source) = &tiles[0].source else {
                unreachable!()
            };
            first = Some(Arc::downgrade(source));
        }
        assert!(cache.tile_cache_bytes() <= 8 << 20);
    }
    assert!(first.unwrap().upgrade().is_none());
    assert!(cache.tile_cache_bytes() > 0);
    cache.clear();
    assert_eq!(cache.tile_cache_bytes(), 0);
}

#[test]
fn large_tiled_paths_keep_their_pixels_through_repeated_presents() {
    use aqua_core::DisplayItem;
    use aqua_scene::Scene;
    use aqua_text::{SharedFonts, SharedRuns};
    let mut path = triangle(Point::ZERO);
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 0.0)),
        PathCommand::Line(Point::new(1500.0, 900.0)),
        PathCommand::Line(Point::new(0.0, 900.0)),
        PathCommand::Close,
    ];
    let mut scene = Scene::new();
    for hue in [0.0, 1.0 / 3.0, 2.0 / 3.0] {
        path.background = GpuiBackground::Solid(GpuiHsla {
            h: hue,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        });
        scene.insert(DisplayItem::GpuiPath {
            transformed_window_logical_bounds: Rect::from_edges(0.0, 0.0, 1500.0, 900.0),
            path: Arc::new(path.clone()),
        });
    }
    path.origin_window_logical = Point::new(300.0, 200.0);
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(400.0, 0.0)),
        PathCommand::Line(Point::new(400.0, 300.0)),
        PathCommand::Line(Point::new(0.0, 300.0)),
        PathCommand::Close,
    ];
    path.background = GpuiBackground::Solid(GpuiHsla {
        h: 1.0 / 3.0,
        s: 1.0,
        l: 0.5,
        a: 1.0,
    });
    scene.insert(DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect::from_edges(300.0, 200.0, 700.0, 500.0),
        path: Arc::new(path),
    });
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    #[cfg(windows)]
    let painters = crate::support::hardware(fonts, runs);
    #[cfg(not(windows))]
    let painters = crate::support::painters(&fonts, &runs);
    for mut painter in painters {
        for _ in 0..3 {
            let frame = painter.paint(&scene, aqua_core::Size::new(1500.0, 900.0), 2.0);
            for (x, y) in [
                (10, 10),
                (1000, 500),
                (1023, 1024),
                (2048, 1024),
                (2800, 1600),
            ] {
                let pixel = frame.pixel(x, y);
                let expected = match (600..1400).contains(&x) && (400..1000).contains(&y) {
                    true => pixel[1] > 250 && pixel[2] < 5 && pixel[0] < 5,
                    false => pixel[2] > 250 && pixel[1] < 5 && pixel[0] < 5,
                };
                assert!(
                    expected && pixel[3] == 255,
                    "{} must preserve the path at {x},{y}, got {pixel:?}",
                    painter.label
                );
            }
        }
    }
}

#[test]
fn frequently_used_paths_survive_unrelated_cache_churn() {
    let mut cache = VectorCache::default();
    let stable = triangle(Point::new(0.0, 0.0));
    let first = cache.path(&stable, Transform2D::IDENTITY).unwrap();
    for width in 20..600 {
        let mut changed = triangle(Point::new(0.0, 0.0));
        changed.commands_local[1] = PathCommand::Line(Point::new(width as f32, 0.0));
        cache.path(&changed, Transform2D::IDENTITY).unwrap();
        let current = cache.path(&stable, Transform2D::IDENTITY).unwrap();
        match (&first.source, &current.source) {
            (
                aqua_core::vector::VectorSource::Drawing(first),
                aqua_core::vector::VectorSource::Drawing(current),
            ) => {
                assert!(Arc::ptr_eq(first, current));
            }
            _ => panic!("path drawing expected"),
        }
    }
}

#[test]
fn gradient_paths_and_even_odd_holes_use_curves_and_match_reference_pixels() {
    use aqua_core::{DisplayItem, GpuiGradientColorSpace, GpuiGradientStop, Size};
    use aqua_scene::Scene;
    use aqua_text::{SharedFonts, SharedRuns};
    let mut path = triangle(Point::new(10.0, 10.0));
    path.commands_local = vec![
        PathCommand::Move(Point::new(0.0, 0.0)),
        PathCommand::Line(Point::new(60.0, 0.0)),
        PathCommand::Line(Point::new(60.0, 60.0)),
        PathCommand::Line(Point::new(0.0, 60.0)),
        PathCommand::Close,
        PathCommand::Move(Point::new(20.0, 20.0)),
        PathCommand::Line(Point::new(40.0, 20.0)),
        PathCommand::Line(Point::new(40.0, 40.0)),
        PathCommand::Line(Point::new(20.0, 40.0)),
        PathCommand::Close,
    ];
    path.style = GpuiPathStyle::Fill {
        rule: FillRule::EvenOdd,
    };
    path.background = GpuiBackground::LinearGradient {
        angle: 180.0,
        color_space: GpuiGradientColorSpace::Srgb,
        stops: [
            GpuiGradientStop {
                percentage: 0.0,
                color: GpuiHsla {
                    h: 0.0,
                    s: 1.0,
                    l: 0.5,
                    a: 0.65,
                },
            },
            GpuiGradientStop {
                percentage: 1.0,
                color: GpuiHsla {
                    h: 2.0 / 3.0,
                    s: 1.0,
                    l: 0.5,
                    a: 1.0,
                },
            },
        ],
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect {
            x: 10.0,
            y: 10.0,
            w: 60.0,
            h: 60.0,
        },
        path: Arc::new(path),
    });
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let size = Size { w: 80.0, h: 80.0 };
    for scale in [1.0, 1.25, 2.0] {
        let expected =
            super::support::reference(fonts.clone(), runs.clone()).paint(&scene, size, scale);
        for mut painter in super::support::painters(&fonts, &runs) {
            let rendered = painter.paint(&scene, size, scale);
            assert_eq!(painter.report.curves, 1, "{}", painter.label);
            assert_eq!(painter.report.unsupported(), 0);
            for (x, y) in [(15.0, 15.0), (20.0, 60.0), (40.0, 40.0), (65.0, 40.0)] {
                let x = (x * scale) as usize;
                let y = (y * scale) as usize;
                let actual = rendered.pixel(x, y);
                let expected = expected.pixel(x, y);
                assert!(
                    actual
                        .into_iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(b) <= 2),
                    "{} at {x},{y}: {actual:?} vs {expected:?}",
                    painter.label
                );
            }
            assert_eq!(painter.paint(&scene, size, scale).rgba8, rendered.rgba8);
        }
    }
}

#[test]
fn svg_curves_preserve_gradients_strokes_group_opacity_and_post_composite_tint() {
    use aqua_core::vector::VectorSource;
    use aqua_core::{DisplayItem, Oklch, Size};
    use aqua_scene::Scene;
    use aqua_text::{SharedFonts, SharedRuns};
    let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="48"><defs><linearGradient id="g"><stop stop-color="#ff2020" stop-opacity=".4"/><stop offset=".5" stop-color="#20ff40"/><stop offset="1" stop-color="#2040ff"/></linearGradient></defs><g opacity=".6" transform="translate(1,2) skewX(4)"><rect x="4" y="4" width="40" height="32" fill="url(#g)"/><path d="M 16 8 H 58 V 42 H 16 Z" fill="#ff8020" fill-opacity=".5" stroke="#2080ff" stroke-width="3" stroke-linejoin="round"/></g></svg>"##;
    for source in [
        VectorSource::svg(svg),
        VectorSource::tinted_svg(svg, Oklch::from_srgb8(90, 190, 245, 0.65)),
    ] {
        let raster = aqua_media::vector::rasterize(
            &source,
            96,
            72,
            aqua_media::vector::VectorBudget::default(),
        )
        .unwrap();
        let mut rgba = raster.pixels.clone();
        for pixel in rgba.chunks_exact_mut(4) {
            let alpha = u32::from(pixel[3]);
            for channel in &mut pixel[..3] {
                *channel = match alpha {
                    0 => 0,
                    _ => ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8,
                };
            }
        }
        let bounds = Rect {
            x: 0.0,
            y: 0.0,
            w: 96.0,
            h: 72.0,
        };
        let size = Size { w: 96.0, h: 72.0 };
        let mut curves = Scene::new();
        curves.insert(DisplayItem::Vector {
            bounds,
            source,
            opacity: 0.8,
        });
        let mut bitmap = Scene::new();
        bitmap.insert(DisplayItem::Image {
            bounds,
            layer: 77,
            fit: aqua_core::image::ImageFit::Fill,
            position: Point::new(0.5, 0.5),
            source: Rect {
                x: 0.0,
                y: 0.0,
                w: 1.0,
                h: 1.0,
            },
            opacity: 0.8,
            rounded_clip: 0.0,
        });
        for mut painter in super::support::painters(&SharedFonts::new(), &SharedRuns::new()) {
            let rendered = painter.paint(&curves, size, 1.0);
            assert_eq!(painter.report.curves, 3, "{}", painter.label);
            assert_eq!(painter.report.unsupported(), 0);
            painter.register_image(77, 96, 72, &rgba);
            let expected = painter.paint(&bitmap, size, 1.0);
            for (x, y) in [(12, 12), (30, 20), (60, 40), (80, 55), (2, 2)] {
                let actual = rendered.pixel(x, y);
                let expected = expected.pixel(x, y);
                assert!(
                    actual
                        .into_iter()
                        .zip(expected)
                        .all(|(a, b)| a.abs_diff(b) <= 3),
                    "{} at {x},{y}: {actual:?} vs {expected:?}",
                    painter.label
                );
            }
        }
    }
}

#[test]
fn compiled_vector_cache_evicts_owned_sources_and_geometry_under_churn() {
    use aqua_core::vector::VectorSource;
    let mut cache = VectorCache::default();
    let source = VectorSource::svg(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path d="M 0 0 H 24 V 24 Z"/></svg>"#,
    );
    let VectorSource::Svg(svg) = &source else {
        unreachable!()
    };
    let source_weak = Arc::downgrade(svg);
    let (_, scene) = cache.curves(&source).unwrap();
    let scene_weak = Arc::downgrade(&scene);
    assert!(Arc::ptr_eq(&scene, &cache.curves(&source).unwrap().1));
    drop(scene);
    drop(source);
    assert!(source_weak.upgrade().is_some());
    for index in 0..80 {
        let source = VectorSource::svg(format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24"><path id="p{index}" d="M 1 1 H 23 V 23 Z"/></svg>"#
        ));
        cache.curves(&source).unwrap();
        assert!(cache.curve_cache_bytes() <= 8 << 20);
    }
    assert!(source_weak.upgrade().is_none());
    assert!(scene_weak.upgrade().is_none());
    cache.clear();
    assert_eq!(cache.curve_cache_bytes(), 0);
}

#[test]
fn raw_vector_source_caches_bound_bytes_and_release_evicted_sources() {
    let mut cache = VectorCache::default();
    let mut path = triangle(Point::new(0.0, 0.0));
    path.commands_local = (0..10_000)
        .flat_map(|_| path.commands_local.clone())
        .collect();
    let first = cache.path(&path, Transform2D::IDENTITY).unwrap();
    let drawing = drawing_of(&first);
    let weak = Arc::downgrade(&drawing);
    drop(first);
    drop(drawing);
    for index in 1..32 {
        path.background = GpuiBackground::Solid(GpuiHsla {
            h: index as f32 / 40.0,
            s: 0.5,
            l: 0.5,
            a: 1.0,
        });
        cache.path(&path, Transform2D::IDENTITY).unwrap();
        assert!(cache.source_cache_bytes() <= 24 << 20);
    }
    assert!(weak.upgrade().is_none());
    let padding = " ".repeat(1 << 20);
    let source: Arc<str> =
        format!(r#"<svg viewBox="0 0 24 24"><!--{padding}--><path d="M 0 0 H 24 V 24 Z"/></svg>"#)
            .into();
    let first = cache.svg(&source, None, 0);
    let aqua_core::vector::VectorSource::Svg(svg) = &first else {
        unreachable!()
    };
    let weak = Arc::downgrade(svg);
    drop(first);
    drop(source);
    for index in 0..8 {
        let source = format!(
            r#"<svg viewBox="0 0 24 24"><!--{padding}{index}--><path d="M 0 0 H 24 V 24 Z"/></svg>"#
        )
        .into();
        cache.svg(&source, None, 0);
        assert!(cache.source_cache_bytes() <= 24 << 20);
    }
    assert!(weak.upgrade().is_none());
    cache.clear();
    assert_eq!(cache.source_cache_bytes(), 0);
}

#[test]
fn drawing_curves_stretch_nonzero_view_boxes_and_keep_fill_before_stroke() {
    use aqua_core::vector::{Brush, Drawing, GradientStop, PaintedPath, Stroke, VectorSource};
    use aqua_core::{DisplayItem, Oklch, Size};
    use aqua_scene::Scene;
    use aqua_text::{SharedFonts, SharedRuns};
    let mut path = PaintedPath::fill(
        vec![
            PathCommand::Move(Point::new(20.0, 10.0)),
            PathCommand::Line(Point::new(60.0, 10.0)),
            PathCommand::Line(Point::new(60.0, 30.0)),
            PathCommand::Line(Point::new(20.0, 30.0)),
            PathCommand::Close,
        ],
        Brush::Linear {
            start: Point::new(20.0, 10.0),
            end: Point::new(60.0, 30.0),
            stops: Arc::from([
                GradientStop {
                    position: 0.0,
                    color: Oklch::from_srgb8(200, 40, 80, 0.7),
                },
                GradientStop {
                    position: 1.0,
                    color: Oklch::from_srgb8(40, 90, 230, 1.0),
                },
            ]),
        },
    );
    path.stroke = Some(Stroke::new(
        Brush::Solid(Oklch::from_srgb8(230, 180, 70, 1.0)),
        3.0,
    ));
    let source = VectorSource::Drawing(Arc::new(Drawing {
        view_box: Rect {
            x: 20.0,
            y: 10.0,
            w: 40.0,
            h: 20.0,
        },
        paths: vec![path],
    }));
    let mut rgba = aqua_media::vector::rasterize(
        &source,
        100,
        60,
        aqua_media::vector::VectorBudget::default(),
    )
    .unwrap()
    .pixels;
    for pixel in rgba.chunks_exact_mut(4) {
        let alpha = u32::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = match alpha {
                0 => 0,
                _ => ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8,
            };
        }
    }
    let bounds = Rect {
        x: 10.0,
        y: 20.0,
        w: 100.0,
        h: 60.0,
    };
    let size = Size { w: 120.0, h: 90.0 };
    let mut curves = Scene::new();
    curves.insert(DisplayItem::Vector {
        bounds,
        source,
        opacity: 1.0,
    });
    let mut bitmap = Scene::new();
    bitmap.insert(DisplayItem::Image {
        bounds,
        layer: 78,
        fit: aqua_core::image::ImageFit::Fill,
        position: Point::new(0.5, 0.5),
        source: Rect {
            x: 0.0,
            y: 0.0,
            w: 1.0,
            h: 1.0,
        },
        opacity: 1.0,
        rounded_clip: 0.0,
    });
    for mut painter in super::support::painters(&SharedFonts::new(), &SharedRuns::new()) {
        let rendered = painter.paint(&curves, size, 1.0);
        assert_eq!(painter.report.curves, 2, "{}", painter.label);
        painter.register_image(78, 100, 60, &rgba);
        let expected = painter.paint(&bitmap, size, 1.0);
        for (x, y) in [(12, 22), (30, 40), (80, 60), (108, 78), (8, 40)] {
            let actual = rendered.pixel(x, y);
            let expected = expected.pixel(x, y);
            assert!(
                actual
                    .into_iter()
                    .zip(expected)
                    .all(|(a, b)| a.abs_diff(b) <= 3),
                "{} at {x},{y}: {actual:?} vs {expected:?}",
                painter.label
            );
        }
    }
}
