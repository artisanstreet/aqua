use aqua_core::{geometry::CornerRadii, scene::ShadowLayer};
use aqua_render::{DisplayItem, Oklch, Rect, RenderError, Renderer, Scene};

fn new_renderer() -> Option<Renderer> {
    match pollster::block_on(Renderer::headless(80, 64)) {
        Ok(renderer) => Some(renderer),
        Err(RenderError::AdapterNotFound(error)) if std::env::var("AQUA_REQUIRE_GPU").is_err() => {
            eprintln!("SKIP: {error}");
            None
        }
        Err(error) => panic!("{error}"),
    }
}

fn shadow(bounds: Rect, layer: ShadowLayer) -> DisplayItem {
    DisplayItem::Shadow {
        bounds,
        corners: CornerRadii::uniform(0.0),
        layer,
    }
}

fn alpha(pixels: &[u8], x: usize, y: usize) -> u8 {
    pixels[(y * 80 + x) * 4 + 3]
}

#[test]
fn gpui_quad_pulse_replays_retained_geometry_and_wraps_clock() {
    use aqua_core::scene::GpuiQuadPulse;
    use aqua_core::Insets;

    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let mut scene = Scene::new();
    let scope = scene.insert(DisplayItem::PushGpuiQuadPulse(GpuiQuadPulse {
        start_ms: u32::MAX - 399,
        period_ms: 1600,
        min_opacity: 0.4,
    }));
    scene.insert(DisplayItem::Quad {
        bounds: Rect::from_edges(5.0, 5.0, 20.0, 20.0),
        corners: CornerRadii::uniform(0.0),
        fill: Oklch::WHITE,
        border_widths: Insets::uniform(0.0),
        border_color: Oklch::TRANSPARENT,
        dashed: false,
    });
    scene.insert(DisplayItem::Rect {
        bounds: Rect::from_edges(25.0, 5.0, 40.0, 20.0),
        radius: 0.0,
        fill: Oklch::WHITE,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    });
    scene.insert(DisplayItem::PopGpuiQuadPulse);
    renderer.set_gpui_pulse_clock_ms(Some(u32::MAX - 399));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert!(alpha(&renderer.readback().unwrap(), 10, 10) > 250);
    renderer.set_gpui_pulse_clock_ms(Some(400));
    let plan = renderer.prepare(&mut scene);
    assert_eq!(plan.nodes_updated, 0);
    assert_eq!(plan.nodes_traversed, 0);
    assert!(plan.partial);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!((alpha(&pixels, 10, 10) as i32 - 102).abs() <= 2);
    assert!(alpha(&pixels, 30, 10) > 250);
    scene.update(
        scope,
        DisplayItem::PushGpuiQuadPulse(GpuiQuadPulse::default()),
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert!(!renderer.has_gpui_quad_pulse());
    assert!(alpha(&renderer.readback().unwrap(), 10, 10) > 250);
}

fn gaussian_alpha(x: f32, y: f32, sigma: f32) -> f32 {
    let erf = |value: f32| {
        let a = value.abs();
        let q = 1.0 + (0.278393 + (0.230389 + 0.078108 * a * a) * a) * a;
        value.signum() * (1.0 - 1.0 / (q * q * q * q))
    };
    let start = (-3.0 * sigma).clamp(y - 10.0, y + 10.0);
    let end = (3.0 * sigma).clamp(y - 10.0, y + 10.0);
    let step = (end - start) / 4.0;
    (0..4)
        .map(|i| {
            let sample = start + (i as f32 + 0.5) * step;
            let integral = 0.5
                * (erf((x + 10.0) * (0.5_f32.sqrt() / sigma))
                    - erf((x - 10.0) * (0.5_f32.sqrt() / sigma)));
            integral * (-(sample * sample) / (2.0 * sigma * sigma)).exp()
                / ((2.0 * std::f32::consts::PI).sqrt() * sigma)
                * step
        })
        .sum()
}

#[test]
fn shadow_draws_zero_blur_rims_and_gaussian_tail_and_repaints_old_bounds() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let mut layer = ShadowLayer {
        dx: 0.0,
        dy: 0.0,
        blur: 0.0,
        spread: 2.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    let id = scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(alpha(&pixels, 18, 30) > 240);
    assert_eq!(alpha(&pixels, 25, 30), 0);
    layer.blur = 8.0;
    layer.spread = 0.0;
    layer.dx = 20.0;
    scene.update(id, shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    assert!(plan.partial);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 18, 30), 0);
    let expected = (gaussian_alpha(12.5, 0.5, 4.0) * 255.0).round() as i32;
    assert!((alpha(&pixels, 62, 30) as i32 - expected).abs() <= 3);
    assert_eq!(alpha(&pixels, 30, 30), 0);
    let mut fresh = new_renderer();
    if let Some(ref mut fresh) = fresh {
        let mut fresh_scene = Scene::new();
        fresh_scene.insert(shadow(bounds, layer));
        let plan = fresh.prepare(&mut fresh_scene);
        fresh.draw(&plan).unwrap();
        assert_eq!(pixels, fresh.readback().unwrap());
    }
}

#[test]
fn inset_shadow_is_clipped_to_element_and_complements_hole() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let layer = ShadowLayer {
        dx: 0.0,
        dy: 2.0,
        blur: 0.0,
        spread: 0.0,
        inset: true,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 19, 20), 0);
    assert!(alpha(&pixels, 25, 20) > 240);
    assert_eq!(alpha(&pixels, 25, 25), 0);
}

#[test]
fn updating_background_preserves_inset_layer_during_partial_redraw() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let background = |fill| DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    };
    let layer = ShadowLayer {
        dx: 0.0,
        dy: 2.0,
        blur: 0.0,
        spread: 0.0,
        inset: true,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    let id = scene.insert(background(Oklch::WHITE));
    scene.insert(shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    scene.update(id, background(Oklch::from_hex("#ff0000").unwrap()));
    let plan = renderer.prepare(&mut scene);
    assert!(plan.partial);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let rim = &pixels[(20 * 80 + 25) * 4..][..4];
    let center = &pixels[(25 * 80 + 25) * 4..][..4];
    assert!(rim[0] < 10 && center[0] > 200);
}

#[test]
fn transformed_clipped_shadow_update_matches_full_redraw() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let clip = Rect::from_edges(10.0, 10.0, 65.0, 50.0);
    let bounds = Rect::from_edges(20.0, 20.0, 35.0, 35.0);
    let mut layer = ShadowLayer {
        dx: 1.0,
        dy: 1.0,
        blur: 6.0,
        spread: 3.0,
        inset: false,
        color: Oklch::BLACK,
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushClip {
        bounds: clip,
        radius: 0.0,
    });
    scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::translate(8.0, 0.0),
    ));
    let id = scene.insert(shadow(bounds, layer));
    scene.insert(DisplayItem::PopTransform);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    layer.dx = 9.0;
    scene.update(id, shadow(bounds, layer));
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let updated = renderer.readback().unwrap();
    let Some(mut fresh) = new_renderer() else {
        return;
    };
    let mut fresh_scene = Scene::new();
    fresh_scene.insert(DisplayItem::PushClip {
        bounds: clip,
        radius: 0.0,
    });
    fresh_scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::translate(8.0, 0.0),
    ));
    fresh_scene.insert(shadow(bounds, layer));
    fresh_scene.insert(DisplayItem::PopTransform);
    fresh_scene.insert(DisplayItem::PopClip);
    let plan = fresh.prepare(&mut fresh_scene);
    fresh.draw(&plan).unwrap();
    assert_eq!(updated, fresh.readback().unwrap());
}

#[test]
fn mixed_corners_and_edge_widths_shape_fill_and_border() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let corners = CornerRadii {
        top_left: 0.0,
        top_right: 8.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Quad {
        bounds,
        corners,
        fill: Oklch::TRANSPARENT,
        border_widths: aqua_core::Insets {
            top: 2.0,
            right: 4.0,
            bottom: 0.0,
            left: 0.0,
        },
        border_color: Oklch::WHITE,
        dashed: false,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(alpha(&pixels, 25, 20) > 240);
    assert!(alpha(&pixels, 38, 30) > 240);
    assert_eq!(alpha(&pixels, 20, 30), 0);
    assert_eq!(alpha(&pixels, 30, 39), 0);
    assert_eq!(alpha(&pixels, 39, 20), 0);
}

#[test]
fn mixed_corner_clip_and_dashed_edge_apply_to_rectangles() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let bounds = Rect::from_edges(20.0, 20.0, 40.0, 40.0);
    let corners = CornerRadii {
        top_left: 0.0,
        top_right: 8.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushRoundedClip { bounds, corners });
    scene.insert(DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill: Oklch::WHITE,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    });
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 20, 20), 255);
    assert_eq!(alpha(&pixels, 39, 20), 0);
    let mut dashed_scene = Scene::new();
    dashed_scene.insert(DisplayItem::Quad {
        bounds,
        corners: CornerRadii::uniform(0.0),
        fill: Oklch::TRANSPARENT,
        border_widths: aqua_core::Insets {
            top: 2.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        },
        border_color: Oklch::WHITE,
        dashed: true,
    });
    let plan = renderer.prepare(&mut dashed_scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(alpha(&pixels, 22, 20) > alpha(&pixels, 26, 20));
}

#[test]
fn rounded_backdrop_blur_respects_its_four_corner_region() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let mut scene = Scene::new();
    let background = |bounds, fill| DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill,
        border_width: 0.0,
        border_color: Oklch::TRANSPARENT,
        shadow: None,
    };
    scene.insert(background(
        Rect::from_edges(0.0, 0.0, 40.0, 64.0),
        Oklch::WHITE,
    ));
    scene.insert(background(
        Rect::from_edges(40.0, 0.0, 80.0, 64.0),
        Oklch::BLACK,
    ));
    let bounds = Rect::from_edges(30.0, 16.0, 60.0, 48.0);
    let corners = CornerRadii {
        top_left: 12.0,
        top_right: 0.0,
        bottom_right: 0.0,
        bottom_left: 0.0,
    };
    scene.insert(DisplayItem::BackdropBlurRounded {
        bounds,
        radius: 8.0,
        corners,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let pixel = |x: usize, y: usize| pixels[(y * 80 + x) * 4];
    assert_eq!(pixel(30, 16), 255);
    assert!(pixel(39, 30) < 245);
    assert_eq!(pixel(25, 30), 255);
}

#[test]
fn fractional_scale_snaps_uniform_rounded_quad_and_thin_stroke_like_gpui() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    renderer.set_size(80, 64, 1.5).unwrap();
    let bounds = Rect::from_edges(5.0, 5.0, 15.0, 15.0);
    let mut scene = Scene::new();
    scene.insert(DisplayItem::Quad {
        bounds,
        corners: CornerRadii::uniform(2.0),
        fill: Oklch::WHITE,
        border_widths: aqua_core::Insets::uniform(0.4),
        border_color: Oklch::BLACK,
        dashed: false,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 80 + x) * 4..][..4];
    assert_eq!(pixel(14, 6)[3], 0);
    assert_eq!(pixel(14, 7)[3], 255);
    assert!(pixel(14, 7)[0] < 10);
    assert!(pixel(14, 8)[0] > 240);
    assert_eq!(pixel(14, 22)[3], 0);
}

#[test]
fn transformed_scale_is_applied_before_gpui_stroke_snapping() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    renderer.set_size(80, 64, 1.2).unwrap();
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(aqua_core::Transform2D::scale(
        1.25, 1.25,
    )));
    scene.insert(DisplayItem::Quad {
        bounds: Rect::from_edges(4.0, 4.0, 12.0, 12.0),
        corners: CornerRadii::uniform(0.0),
        fill: Oklch::WHITE,
        border_widths: aqua_core::Insets::uniform(0.4),
        border_color: Oklch::BLACK,
        dashed: false,
    });
    scene.insert(DisplayItem::PopTransform);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let pixel = |x: usize, y: usize| &pixels[(y * 80 + x) * 4..][..4];
    assert_eq!(pixel(10, 5)[3], 0);
    assert!(pixel(10, 6)[0] < 10);
    assert!(pixel(10, 7)[0] > 240);
    assert_eq!(pixel(10, 18)[3], 0);
}

#[test]
fn source_background_does_not_silently_fallback_to_linear_rendering() {
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(1.0, 1.0, 10.0, 10.0),
        corners: CornerRadii::uniform(0.0),
        background: aqua_core::GpuiBackground::Solid(aqua_core::GpuiHsla {
            h: 0.12345,
            s: 0.65,
            l: 0.4,
            a: 1.0,
        }),
    });
    let plan = renderer.prepare(&mut scene);
    assert!(renderer.draw(&plan).is_err());
}

#[test]
fn gpui_encoded_path_composites_float_color_and_retains_painter_order() {
    use aqua_core::vector::{FillRule, GpuiPath, GpuiPathStyle, PathCommand};
    use aqua_core::{ColorProfile, GpuiBackground, GpuiHsla, Point, Transform2D};
    use aqua_render::text::{ShapedRun, TextFrame, TextProvider, TextStats};
    use std::sync::Arc;

    struct EmptyText;
    impl TextProvider for EmptyText {
        fn set_raster_mode(
            &mut self,
            _: &wgpu::Device,
            _: &wgpu::Queue,
            _: wgpu::TextureFormat,
            _: aqua_core::scene::TextRasterMode,
        ) -> Result<(), String> {
            Ok(())
        }
        fn set_color_profile(
            &mut self,
            _: &wgpu::Device,
            _: &wgpu::Queue,
            _: wgpu::TextureFormat,
            _: ColorProfile,
        ) -> Result<(), String> {
            Ok(())
        }
        fn queue_run(&mut self, _: ShapedRun) {}
        fn encode_queued(&mut self, _: TextFrame<'_>) -> TextStats {
            TextStats::default()
        }
        fn measure_ink(&mut self, _: u64) -> Option<Rect> {
            None
        }
        fn trim_atlas(&mut self) {}
        fn stats_last(&self) -> TextStats {
            TextStats::default()
        }
    }
    let Some(mut renderer) = new_renderer() else {
        return;
    };
    renderer.set_text_provider(Box::new(EmptyText));
    renderer
        .set_color_profile(ColorProfile::GpuiWindowsEncoded)
        .unwrap();
    let background = GpuiHsla {
        h: 0.0,
        s: 1.0,
        l: 0.5,
        a: 1.0,
    };
    let foreground = GpuiHsla {
        h: 1.0 / 3.0,
        s: 1.0,
        l: 0.5,
        a: 0.5,
    };
    let bounds = Rect::from_edges(10.0, 10.0, 60.0, 54.0);
    let path = GpuiPath {
        commands_local: vec![
            PathCommand::Move(Point::new(0.0, 0.0)),
            PathCommand::Line(Point::new(40.0, 0.0)),
            PathCommand::Line(Point::new(0.0, 36.0)),
            PathCommand::Close,
        ],
        origin_window_logical: Point::new(15.0, 15.0),
        element_transform: Transform2D::IDENTITY,
        style: GpuiPathStyle::Fill {
            rule: FillRule::NonZero,
        },
        background: GpuiBackground::Solid(foreground),
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds,
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(background),
    });
    scene.insert(DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect::from_edges(15.0, 15.0, 55.0, 51.0),
        path: Arc::new(path),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let inside = &pixels[(20 * 80 + 20) * 4..][..4];
    let outside = &pixels[(45 * 80 + 50) * 4..][..4];
    assert!(
        (inside[0] as i32 - 127).abs() <= 2 && (inside[1] as i32 - 128).abs() <= 2,
        "{inside:?}"
    );
    assert!(outside[0] >= 253 && outside[1] <= 2, "{outside:?}");

    let mut scene = Scene::new();
    let icon = Arc::<str>::from(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="10" height="10" fill="#ff0000"/></svg>"##,
    );
    scene.insert(DisplayItem::PushRoundedClip {
        bounds: Rect::from_edges(4.0, 4.0, 24.0, 24.0),
        corners: CornerRadii::uniform(8.0),
    });
    scene.insert(DisplayItem::GpuiSvg {
        bounds: Rect::from_edges(4.0, 4.0, 24.0, 24.0),
        source: icon.clone(),
        tint: Some(GpuiHsla {
            h: 1.0 / 3.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
        opacity: 1.0,
    });
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::GpuiTransformedSvg {
        bounds: Rect::from_edges(58.0, 4.0, 78.0, 24.0),
        source: Arc::from(r##"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"><rect width="5" height="10" fill="#fff"/></svg>"##),
        tint: GpuiHsla { h: 2.0 / 3.0, s: 1.0, l: 0.5, a: 1.0 },
        opacity: 1.0,
        transform: aqua_core::scene::GpuiSvgTransform { rotation_radians: std::f32::consts::FRAC_PI_2, ..Default::default() },
    });
    scene.insert(DisplayItem::GpuiSvg {
        bounds: Rect::from_edges(34.0, 4.0, 54.0, 24.0),
        source: icon,
        tint: None,
        opacity: 1.0,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let green = &pixels[(14 * 80 + 14) * 4..][..4];
    let red = &pixels[(14 * 80 + 44) * 4..][..4];
    assert!(green[1] >= 250 && green[0] <= 2, "{green:?}");
    assert!(red[0] >= 250 && red[1] <= 2, "{red:?}");
    assert_eq!(alpha(&pixels, 4, 4), 0);
    assert!(pixels[(8 * 80 + 68) * 4 + 2] >= 250);
    assert_eq!(alpha(&pixels, 68, 20), 0);

    let mut coverage_scene = Scene::new();
    coverage_scene.insert(DisplayItem::GpuiSvg {
        bounds: Rect::from_edges(4.0, 4.0, 28.0, 28.0),
        source: Arc::from(r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><path d="M1 8 L6 14 L15 2 L12 1 L6 10 L3 6 Z" fill="white"/></svg>"#),
        tint: Some(GpuiHsla {
            h: 1.0 / 3.0,
            s: 0.72,
            l: 0.36,
            a: 0.8,
        }),
        opacity: 1.0,
    });
    let plan = renderer.prepare(&mut coverage_scene);
    renderer.draw(&plan).unwrap();
    let default_coverage = renderer.readback().unwrap();
    renderer
        .set_text_raster_mode(aqua_core::scene::TextRasterMode::Outline)
        .unwrap();
    let plan = renderer.prepare(&mut coverage_scene);
    renderer.draw(&plan).unwrap();
    let outline_coverage = renderer.readback().unwrap();
    assert!(default_coverage
        .as_chunks::<4>()
        .0
        .iter()
        .zip(outline_coverage.as_chunks::<4>().0.iter())
        .any(|(normal, outline)| normal[3] > outline[3] && outline[3] > 0));
    renderer
        .set_text_raster_mode(aqua_core::scene::TextRasterMode::PlatformDefault)
        .unwrap();
    let plan = renderer.prepare(&mut coverage_scene);
    renderer.draw(&plan).unwrap();
    assert_eq!(default_coverage, renderer.readback().unwrap());

    let mut scene = Scene::new();
    let screen = Rect::from_edges(0.0, 0.0, 80.0, 64.0);
    scene.insert(DisplayItem::GpuiBackground {
        bounds: screen,
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(background),
    });
    for mask in [
        Rect::from_edges(0.0, 0.0, 30.25, 64.0),
        Rect::from_edges(30.25, 0.0, 80.0, 64.0),
    ] {
        scene.insert(DisplayItem::PushGpuiContentMask { bounds: mask });
        scene.insert(DisplayItem::GpuiBackground {
            bounds: screen,
            corners: CornerRadii::uniform(0.0),
            background: GpuiBackground::Solid(foreground),
        });
        scene.insert(DisplayItem::PopClip);
    }
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    for x in 26..35 {
        let pixel = &pixels[(30 * 80 + x) * 4..][..4];
        let expected = if x == 30 { [63, 192] } else { [127, 128] };
        assert!(
            (pixel[0] as i32 - expected[0]).abs() <= 2
                && (pixel[1] as i32 - expected[1]).abs() <= 2,
            "{x}: {pixel:?}"
        );
    }

    let mut scene = Scene::new();
    let screen = Rect::from_edges(0.0, 0.0, 80.0, 64.0);
    let id = scene.insert(DisplayItem::GpuiBackground {
        bounds: screen,
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(background),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    renderer
        .start_gpui_snapshot_transition(aqua_core::scene::GpuiSnapshotTransition {
            duration: std::time::Duration::from_secs(1),
            band: 0.18,
            shimmer: GpuiHsla {
                a: 0.0,
                ..foreground
            },
        })
        .unwrap();
    scene.update(
        id,
        DisplayItem::GpuiBackground {
            bounds: screen,
            corners: CornerRadii::uniform(0.0),
            background: GpuiBackground::Solid(GpuiHsla {
                h: 2.0 / 3.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
        },
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert!(renderer.set_snapshot_transition_elapsed(std::time::Duration::from_millis(500)));
    let pixels = renderer.readback().unwrap();
    let new_pixel = &pixels[(8 * 80 + 8) * 4..][..4];
    let old_pixel = &pixels[(56 * 80 + 72) * 4..][..4];
    assert!(new_pixel[2] >= 250 && new_pixel[0] <= 2, "{new_pixel:?}");
    assert!(old_pixel[0] >= 250 && old_pixel[2] <= 2, "{old_pixel:?}");
    renderer.cancel_snapshot_transition();

    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(8.0, 8.0, 48.0, 48.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            a: 1.0,
            ..background
        }),
    });
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(24.0, 8.0, 64.0, 48.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            a: 1.0,
            ..foreground
        }),
    });
    scene.insert(DisplayItem::PopOpacity);
    scene.insert(DisplayItem::PopOpacity);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let only_first = &pixels[(20 * 80 + 16) * 4..][..4];
    let overlap = &pixels[(20 * 80 + 32) * 4..][..4];
    let only_nested = &pixels[(20 * 80 + 56) * 4..][..4];
    assert!((only_first[3] as i32 - 128).abs() <= 2, "{only_first:?}");
    assert!((overlap[3] as i32 - 160).abs() <= 2, "{overlap:?}");
    assert!((only_nested[3] as i32 - 64).abs() <= 2, "{only_nested:?}");

    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        }),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let source = aqua_core::GpuiTextColor {
        hue_degrees: -37.12345,
        saturation: 0.73,
        lightness: 0.41,
        alpha: 0.45,
    };
    renderer
        .start_gpui_source_snapshot_transition(aqua_core::scene::GpuiSourceSnapshotTransition {
            duration: std::time::Duration::from_secs(1),
            band: 0.18,
            shimmer: source,
        })
        .unwrap();
    assert!(renderer.set_snapshot_transition_elapsed(std::time::Duration::from_millis(500)));
    let pixels = renderer.readback().unwrap();
    use palette::IntoColor;
    let expected: palette::rgb::Rgba = palette::Hsla::new(
        source.hue_degrees,
        source.saturation,
        source.lightness,
        source.alpha,
    )
    .into_color();
    let center = &pixels[(32 * 80 + 40) * 4..][..4];
    for (actual, channel) in center[..3]
        .iter()
        .zip([expected.red, expected.green, expected.blue])
    {
        assert!(
            (*actual as f32 - channel * expected.alpha * 255.0).abs() <= 2.0,
            "{center:?}"
        );
    }
    renderer.cancel_snapshot_transition();

    renderer.set_size(79, 63, 1.0).unwrap();
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 39.0, 63.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(39.0, 0.0, 79.0, 63.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 0.0,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::BackdropBlurRounded {
        bounds: Rect::from_edges(27.0, 12.0, 61.0, 51.0),
        corners: CornerRadii::uniform(0.0),
        radius: 8.0,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let outside = pixels[(30 * 79 + 24) * 4];
    let blurred = pixels[(30 * 79 + 38) * 4];
    let far = pixels[(30 * 79 + 65) * 4];
    assert!(outside >= 250 && far <= 3, "{outside} {far}");
    assert!(blurred > 20 && blurred < 235, "{blurred}");

    renderer.set_size(80, 64, 1.0).unwrap();
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushTransform(
        aqua_core::Transform2D::translate(0.25, 0.25),
    ));
    scene.insert(DisplayItem::PushGpuiContentMask {
        bounds: Rect::from_edges(10.1, 10.1, 30.2, 30.2),
    });
    scene.insert(DisplayItem::PushGpuiContentMask {
        bounds: Rect::from_edges(15.4, 8.0, 35.0, 25.1),
    });
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::PopTransform);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 15, 10), 255);
    assert_eq!(alpha(&pixels, 30, 25), 255);
    assert_eq!(alpha(&pixels, 14, 10), 0);
    assert_eq!(alpha(&pixels, 31, 25), 0);
    assert_eq!(alpha(&pixels, 15, 26), 0);

    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushGpuiContentMask {
        bounds: Rect::from_edges(10.1, 10.1, 10.2, 20.0),
    });
    scene.insert(DisplayItem::PushGpuiContentMask {
        bounds: Rect::from_edges(10.3, 10.1, 10.4, 20.0),
    });
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::PopClip);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(alpha(&pixels, 10, 12), 255);
    assert_eq!(alpha(&pixels, 11, 12), 0);
    assert_eq!(alpha(&pixels, 9, 12), 0);

    renderer.set_size(501, 301, 1.25).unwrap();
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 400.8, 240.8),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 0.96,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::GpuiPath {
        transformed_window_logical_bounds: Rect::from_edges(144.0, 16.0, 216.0, 74.0),
        path: Arc::new(GpuiPath {
            commands_local: vec![
                PathCommand::Move(Point::new(4.0, 8.0)),
                PathCommand::Line(Point::new(66.0, 3.0)),
                PathCommand::Line(Point::new(40.0, 52.0)),
                PathCommand::Close,
            ],
            origin_window_logical: Point::new(144.0, 16.0),
            element_transform: Transform2D::IDENTITY,
            style: GpuiPathStyle::Fill {
                rule: FillRule::NonZero,
            },
            background: GpuiBackground::Solid(GpuiHsla {
                h: 1.0 / 3.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            }),
        }),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(&pixels[(23 * 501 + 261) * 4..][..4], &[245, 245, 245, 255]);
    let inside = &pixels[(50 * 501 + 220) * 4..][..4];
    assert!(inside[1] >= 250 && inside[0] <= 2, "{inside:?}");

    renderer.set_size(80, 64, 1.0).unwrap();
    let texture = renderer.device().create_texture(&wgpu::TextureDescriptor {
        label: None,
        size: wgpu::Extent3d {
            width: 8,
            height: 4,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    let mut image = Vec::new();
    for _ in 0..4 {
        for x in 0..8 {
            image.extend_from_slice(if (2..6).contains(&x) {
                &[0, 255, 0, 255]
            } else {
                &[255, 0, 0, 255]
            });
        }
    }
    renderer.queue().write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        &image,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(32),
            rows_per_image: Some(4),
        },
        wgpu::Extent3d {
            width: 8,
            height: 4,
            depth_or_array_layers: 1,
        },
    );
    renderer.register_external(
        81,
        texture.create_view(&wgpu::TextureViewDescriptor::default()),
        8,
        4,
    );
    renderer.register_gpui_image(81, 81, 0, Arc::from(image));
    let mut scene = Scene::new();
    for (bounds, fit, radius) in [
        (
            Rect::from_edges(10.25, 5.0, 26.25, 13.0),
            aqua_core::image::ImageFit::Fill,
            0.0,
        ),
        (
            Rect::from_edges(30.25, 5.0, 46.25, 21.0),
            aqua_core::image::ImageFit::Cover,
            0.0,
        ),
        (
            Rect::from_edges(50.25, 5.0, 66.25, 21.0),
            aqua_core::image::ImageFit::Fill,
            6.0,
        ),
    ] {
        scene.insert(DisplayItem::Image {
            bounds,
            layer: 81,
            fit,
            position: Point::new(0.5, 0.5),
            source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
            opacity: 1.0,
            rounded_clip: radius,
        });
    }
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!((190..=193).contains(&alpha(&pixels, 10, 8)));
    assert_eq!(alpha(&pixels, 26, 8), 0);
    let crop = &pixels[(13 * 80 + 33) * 4..][..4];
    assert!(crop[1] >= 250 && crop[0] <= 2, "{crop:?}");
    assert_eq!(alpha(&pixels, 50, 5), 0);
    assert_eq!(alpha(&pixels, 58, 13), 255);

    renderer.register_external(
        82,
        texture.create_view(&wgpu::TextureViewDescriptor::default()),
        8,
        4,
    );
    renderer.register_gpui_image(82, 82, 0, Arc::from([200u8, 40, 100, 128].repeat(32)));
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    for (left, grouped) in [(10.0, false), (40.0, true)] {
        let bounds = Rect::from_edges(left, 30.0, left + 16.0, 38.0);
        scene.insert(DisplayItem::PushGpuiContentMask {
            bounds: Rect::from_edges(left + 2.0, 30.0, left + 14.0, 38.0),
        });
        if grouped {
            scene.insert(DisplayItem::PushContentBlur {
                bounds,
                radius: 0.0,
            });
        }
        scene.insert(DisplayItem::Image {
            bounds,
            layer: 82,
            fit: aqua_core::image::ImageFit::Fill,
            position: Point::new(0.5, 0.5),
            source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
            opacity: 1.0,
            rounded_clip: 0.0,
        });
        if grouped {
            scene.insert(DisplayItem::PopContentBlur);
        }
        scene.insert(DisplayItem::PopClip);
    }
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert_eq!(&pixels[(34 * 80 + 16) * 4..][..4], &[227, 147, 177, 255]);
    assert_eq!(&pixels[(34 * 80 + 46) * 4..][..4], &[227, 147, 177, 255]);
    assert_eq!(&pixels[(34 * 80 + 11) * 4..][..4], &[255, 255, 255, 255]);
    assert_eq!(&pixels[(34 * 80 + 41) * 4..][..4], &[255, 255, 255, 255]);

    renderer.register_external(
        83,
        texture.create_view(&wgpu::TextureViewDescriptor::default()),
        8,
        4,
    );
    renderer.register_gpui_image(83, 83, 0, Arc::from([10u8, 20, 30, 255].repeat(32)));
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    scene.insert(DisplayItem::Image {
        bounds: Rect::from_edges(10.0, 10.0, 26.0, 18.0),
        layer: 83,
        fit: aqua_core::image::ImageFit::Fill,
        position: Point::new(0.5, 0.5),
        source: Rect::from_edges(0.0, 0.0, 1.0, 1.0),
        opacity: 1.0,
        rounded_clip: 0.0,
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert_eq!(
        &renderer.readback().unwrap()[(14 * 80 + 18) * 4..][..4],
        &[255, 255, 255, 255]
    );

    assert!(renderer.supports_gpui_meshes());
    let vertex =
        |x, y, z, color| aqua_core::GpuiMeshVertex::new([x, y, z], [0.0, 0.0, 1.0], Some(color));
    let mesh = Arc::new(aqua_core::GpuiMesh::new(vec![
        vertex(-0.8, -0.8, 0.8, [1.0, 0.0, 0.0]),
        vertex(0.8, -0.8, 0.8, [1.0, 0.0, 0.0]),
        vertex(0.0, 0.8, 0.8, [1.0, 0.0, 0.0]),
        vertex(-0.8, -0.8, 0.2, [0.0, 0.0, 1.0]),
        vertex(0.8, -0.8, 0.2, [0.0, 0.0, 1.0]),
        vertex(0.0, 0.8, 0.2, [0.0, 0.0, 1.0]),
    ]));
    let identity = [
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 1.0, 0.0, 0.0],
        [0.0, 0.0, 1.0, 0.0],
        [0.0, 0.0, 0.0, 1.0],
    ];
    let style = aqua_core::GpuiMeshStyle {
        view: identity,
        projection: identity,
        base_color: [0.5; 3],
        edge_color: [0.0; 4],
    };
    let mut scene = Scene::new();
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(0.0, 0.0, 80.0, 64.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        }),
    });
    scene.insert(DisplayItem::PushGpuiContentMask {
        bounds: Rect::from_edges(20.0, 20.0, 40.0, 60.0),
    });
    let id = scene.insert(DisplayItem::GpuiMesh {
        bounds: Rect::from_edges(20.0, 20.0, 60.0, 60.0),
        corners: CornerRadii::uniform(0.0),
        mesh: mesh.clone(),
        style,
    });
    scene.insert(DisplayItem::PopClip);
    scene.insert(DisplayItem::GpuiBackground {
        bounds: Rect::from_edges(30.0, 38.0, 33.0, 42.0),
        corners: CornerRadii::uniform(0.0),
        background: GpuiBackground::Solid(GpuiHsla {
            h: 1.0 / 3.0,
            s: 1.0,
            l: 0.5,
            a: 1.0,
        }),
    });
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let front = &pixels[(40 * 80 + 36) * 4..][..4];
    assert!(front[2] > front[0] && front[2] > front[1], "{front:?}");
    assert_eq!(&pixels[(40 * 80 + 44) * 4..][..4], &[255, 255, 255, 255]);
    assert_eq!(&pixels[(40 * 80 + 31) * 4..][..4], &[0, 255, 0, 255]);
    scene.update(
        id,
        DisplayItem::GpuiMesh {
            bounds: Rect::from_edges(20.0, 20.0, 60.0, 60.0),
            corners: CornerRadii::uniform(0.0),
            mesh,
            style: aqua_core::GpuiMeshStyle {
                edge_color: [1.0, 1.0, 1.0, 1.0],
                ..style
            },
        },
    );
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    assert_ne!(pixels, renderer.readback().unwrap());
}
