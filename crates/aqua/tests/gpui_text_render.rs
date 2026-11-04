use aqua::{UiApp, ui::Text};
use aqua_core::{ColorProfile, GpuiHsla};
use aqua_render::{Renderer, text::TextBackend};
use aqua_text::{SharedFonts, SharedRuns, StyledSpan};

#[test]
fn ui_app_source_text_reaches_the_encoded_glyph_renderer() {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle();
    descriptor.backends = wgpu::Backends::from_env().unwrap_or_else(wgpu::Backends::all);
    let instance = wgpu::Instance::new(descriptor);
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
        apply_limit_buckets: false,
    })) {
        Ok(adapter) => adapter,
        Err(_) if std::env::var_os("AQUA_REQUIRE_GPU").is_none() => return,
        Err(error) => panic!("GPU adapter unavailable: {error:?}"),
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpui-ui-text-test"),
        required_features: wgpu::Features::DUAL_SOURCE_BLENDING | wgpu::Features::CLIP_DISTANCES,
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request GPUI text GPU device");
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = UiApp::new(fonts.clone(), runs.clone())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        Text::new("Source color")
            .spans(vec![StyledSpan::plain("Source color").gpui_background([
                1.0 / 3.0,
                1.0,
                0.5,
                1.0,
            ])])
            .gpui_color(GpuiHsla {
                h: 0.0,
                s: 1.0,
                l: 0.5,
                a: 1.0,
            })
            .build()
    })
    .unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    let mut renderer = Renderer::new_with_text(
        device,
        queue,
        wgpu::TextureFormat::Rgba8Unorm,
        TextBackend::new(fonts.clone(), runs.clone()),
    );
    renderer
        .set_color_profile(ColorProfile::GpuiWindowsEncoded)
        .unwrap();
    renderer.set_gpui_window_opaque(false);
    renderer.set_size(160, 64, 1.0).unwrap();
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 32
        && u16::from(p[0]) > u16::from(p[1]) * 2
        && u16::from(p[0]) > u16::from(p[2]) * 2));
    assert!(pixels.as_chunks::<4>().0.iter().any(|p| p[3] > 200
        && u16::from(p[1]) > u16::from(p[0]) * 2
        && u16::from(p[1]) > u16::from(p[2]) * 2));

    let mut app = UiApp::new(fonts.clone(), runs.clone())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        Text::new("Raw spans")
            .color(aqua_core::Oklch::TRANSPARENT)
            .spans(vec![
                StyledSpan::plain("Raw spans").gpui_color([0.0, 1.0, 0.5, 1.0]),
            ])
            .build()
    })
    .unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    renderer.set_size(160, 64, 1.0).unwrap();
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    assert!(
        pixels
            .as_chunks::<4>()
            .0
            .iter()
            .any(|p| p[3] > 32 && p[0] > p[1])
    );

    let mut captures = Vec::new();
    for spread in [0.0, 100.0] {
        let mut app = UiApp::new(fonts.clone(), runs.clone())
            .with_color_profile(ColorProfile::GpuiWindowsEncoded);
        app.mount(move |_| {
            let map = aqua_core::scene::GpuiTextColorMap {
                bounds: aqua_core::Rect::from_edges(0.0, 0.0, 60.0, 40.0),
                phase: 0.5,
                spread,
                target: aqua_core::GpuiTextColor {
                    hue_degrees: 0.0,
                    saturation: 0.0,
                    lightness: 0.0,
                    alpha: 1.0,
                },
            };
            aqua::ui::Box::new()
                .style(
                    aqua::Style::new()
                        .width(aqua::px(60.0))
                        .height(aqua::px(40.0))
                        .clip(true)
                        .transform(aqua_core::Transform2D::translate(20.0, 0.0))
                        .gpui_text_color_map(map),
                )
                .child(
                    Text::new("MMMMMMMMMMMM")
                        .gpui_source_color(aqua_core::GpuiTextColor {
                            hue_degrees: -37.1234,
                            saturation: 0.4,
                            lightness: 0.9,
                            alpha: 1.0,
                        })
                        .build(),
                )
                .build()
        })
        .unwrap();
        app.set_viewport(160.0, 64.0, 1.0);
        app.build_frame().unwrap();
        renderer.set_size(160, 64, 1.0).unwrap();
        let plan = renderer.prepare(app.scene_mut());
        renderer.draw(&plan).unwrap();
        captures.push(renderer.readback().unwrap());
    }
    assert!(
        captures[0]
            .as_chunks::<4>()
            .0
            .iter()
            .zip(captures[1].as_chunks::<4>().0.iter())
            .any(|(base, mapped)| base[3] > 80
                && mapped[3] > 80
                && u16::from(mapped[0]) + 10 < u16::from(base[0]))
    );
    for (index, pixel) in captures[1].as_chunks::<4>().0.iter().enumerate() {
        let x = index % 160;
        if !(20..80).contains(&x) {
            assert_eq!(pixel[3], 0);
        }
    }

    let mut app = UiApp::new(fonts.clone(), runs.clone())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        let shimmer = aqua_core::scene::GpuiTextShimmer {
            key: 7,
            duration: std::time::Duration::from_secs(3),
            delay: std::time::Duration::from_millis(1500),
            map: aqua_core::scene::GpuiTextColorMap {
                bounds: aqua_core::Rect::from_edges(0.0, 0.0, 120.0, 40.0),
                phase: 0.0,
                spread: 50.0,
                target: aqua_core::GpuiTextColor {
                    hue_degrees: 0.0,
                    saturation: 0.0,
                    lightness: 0.0,
                    alpha: 1.0,
                },
            },
        };
        aqua::ui::Box::new()
            .style(
                aqua::Style::new()
                    .width(aqua::px(120.0))
                    .height(aqua::px(40.0))
                    .gpui_text_shimmer(shimmer),
            )
            .child(
                Text::new("MMMMMMMMMMMMMMMM")
                    .gpui_source_color(aqua_core::GpuiTextColor {
                        hue_degrees: 0.0,
                        saturation: 0.0,
                        lightness: 1.0,
                        alpha: 1.0,
                    })
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    renderer.set_size(160, 64, 1.0).unwrap();
    let start = std::time::Instant::now();
    renderer.set_gpui_shimmer_time(Some(start));
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).unwrap();
    let held = renderer.readback().unwrap();
    assert_eq!(
        renderer.gpui_shimmer_delay(),
        Some(std::time::Duration::from_millis(1500))
    );
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_millis(1500)));
    let plan = renderer.prepare(app.scene_mut());
    assert_eq!(plan.nodes_updated, 0);
    assert_eq!(plan.nodes_traversed, 0);
    renderer.draw(&plan).unwrap();
    assert_eq!(
        renderer.gpui_shimmer_delay(),
        Some(std::time::Duration::ZERO)
    );
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_secs(3)));
    let plan = renderer.prepare(app.scene_mut());
    assert_eq!(plan.nodes_updated, 0);
    assert_eq!(plan.nodes_traversed, 0);
    renderer.draw(&plan).unwrap();
    let moving = renderer.readback().unwrap();
    assert_ne!(held, moving);
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_millis(4500)));
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).unwrap();
    assert_eq!(
        renderer.gpui_shimmer_delay(),
        Some(std::time::Duration::from_millis(1500))
    );
    assert_eq!(
        renderer.gpui_shimmer_wake_delay(),
        Some(std::time::Duration::from_millis(1500))
    );
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_millis(6500)));
    assert!(renderer.wake_gpui_shimmers());
    assert!(!renderer.wake_gpui_shimmers());
    assert_eq!(renderer.gpui_shimmer_wake_delay(), None);
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_secs(8)));
    let plan = renderer.prepare(app.scene_mut());
    assert_eq!(plan.nodes_traversed, 0);
    renderer.draw(&plan).unwrap();
    assert_eq!(renderer.readback().unwrap(), moving);

    let mut app = UiApp::new(fonts.clone(), runs.clone())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        aqua::ui::Box::new()
            .style(
                aqua::Style::new()
                    .width(aqua::px(160.0))
                    .height(aqua::px(64.0))
                    .padding_insets(aqua_core::Insets {
                        left: 30.0,
                        right: 0.0,
                        top: 8.0,
                        bottom: 0.0,
                    }),
            )
            .child(
                aqua::ui::Scroll::new()
                    .style(
                        aqua::Style::new()
                            .width(aqua::px(100.0))
                            .height(aqua::px(40.0)),
                    )
                    .offset_xy((20.0, 0.0))
                    .child(
                        aqua::ui::Box::new()
                            .style(
                                aqua::Style::new()
                                    .width(aqua::px(120.0))
                                    .height(aqua::px(40.0))
                                    .flex_shrink(0.0)
                                    .transform(aqua_core::Transform2D::translate(20.0, 0.0)),
                            )
                            .child(
                                Text::new("MMMMMMMMMMMMMMMM")
                                    .gpui_source_color(aqua_core::GpuiTextColor {
                                        hue_degrees: 0.0,
                                        saturation: 0.0,
                                        lightness: 1.0,
                                        alpha: 1.0,
                                    })
                                    .build(),
                            )
                            .build(),
                    )
                    .build(),
            )
            .build()
    })
    .unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    let root = app.tree().root().unwrap();
    let scroll = app.tree().get(root).unwrap().children[0];
    let child = app.tree().get(scroll).unwrap().children[0];
    let layout = app.tree().rect(child).unwrap();
    let bounds = app.tree().pre_transform_rect(child).unwrap();
    let visual = app.tree().visual_rect(child).unwrap();
    assert_eq!(bounds.x, layout.x - 20.0);
    assert_eq!(visual.x, bounds.x + 20.0);
    assert_ne!(bounds.x, 0.0);
    let shimmer = aqua_core::scene::GpuiTextShimmer {
        key: 99,
        duration: std::time::Duration::from_secs(2),
        delay: std::time::Duration::ZERO,
        map: aqua_core::scene::GpuiTextColorMap {
            bounds,
            phase: 0.0,
            spread: 50.0,
            target: aqua_core::GpuiTextColor {
                hue_degrees: 0.0,
                saturation: 0.0,
                lightness: 0.0,
                alpha: 1.0,
            },
        },
    };
    let mut paint = app.tree().paint(child).unwrap();
    paint.gpui_text_shimmer = Some(shimmer);
    assert!(app.tree_mut().set_paint(child, paint));
    app.build_frame().unwrap();
    assert!(app.scene_mut().ordered().any(|(_, item)| matches!(item, aqua_core::scene::DisplayItem::PushGpuiTextShimmer(found) if found.map.bounds == bounds)));
    let start = std::time::Instant::now();
    renderer.set_gpui_shimmer_time(Some(start));
    let plan = renderer.prepare(app.scene_mut());
    renderer.draw(&plan).unwrap();
    renderer.set_gpui_shimmer_time(Some(start + std::time::Duration::from_secs(1)));
    let plan = renderer.prepare(app.scene_mut());
    assert_eq!(plan.nodes_traversed, 0);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let mut checked = 0;
    let mut darkened = 0;
    for (index, pixel) in pixels.as_chunks::<4>().0.iter().enumerate() {
        if pixel[3] < 200 {
            continue;
        }
        let x = index % 160;
        let position = (x as f32 + 0.5 - visual.x) / visual.w;
        let mapped = aqua_render::text::gpui::map::shimmer_color(
            [0.0, 0.0, 1.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            position,
            0.5,
            50.0,
        );
        let expected = mapped[2] * 255.0;
        let actual = f32::from(pixel[0]) * 255.0 / f32::from(pixel[3]);
        assert!(
            (actual - expected).abs() < 6.0,
            "x={x} actual={actual} expected={expected}"
        );
        checked += 1;
        if expected < 235.0 {
            darkened += 1;
        }
    }
    assert!(
        checked > 10 && darkened > 5,
        "checked={checked}, darkened={darkened}"
    );
    assert!(
        app.tree_mut()
            .scroll_to(scroll, aqua_core::Point::new(10.0, 0.0))
    );
    assert_eq!(
        app.tree().pre_transform_rect(child).unwrap().x,
        layout.x - 10.0
    );
    assert_eq!(app.tree().visual_rect(child).unwrap().x, layout.x + 10.0);

    let mut app = UiApp::new(fonts.clone(), runs.clone())
        .with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        Text::new("MMMMMMMMMMMM")
            .gpui_color(GpuiHsla {
                h: 0.0,
                s: 0.0,
                l: 0.0,
                a: 1.0,
            })
            .build()
    })
    .unwrap();
    app.set_viewport(160.0, 64.0, 1.0);
    app.build_frame().unwrap();
    let text_items: Vec<_> = app
        .scene_mut()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    renderer.set_gpui_window_opaque(true);
    let mut captures = Vec::new();
    for nested in [false, true] {
        let mut scene = aqua_render::Scene::new();
        scene.insert(aqua_core::scene::DisplayItem::GpuiBackground {
            bounds: aqua_core::Rect::from_edges(0.0, 0.0, 160.0, 64.0),
            corners: aqua_core::CornerRadii::uniform(0.0),
            background: aqua_core::GpuiBackground::Solid(GpuiHsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 1.0,
            }),
        });
        if nested {
            scene.insert(aqua_core::scene::DisplayItem::PushGpuiContentMask {
                bounds: aqua_core::Rect::from_edges(0.0, 0.0, 100.0, 64.0),
            });
        }
        scene.insert(aqua_core::scene::DisplayItem::PushGpuiContentMask {
            bounds: aqua_core::Rect::from_edges(
                20.0,
                0.0,
                if nested { 160.0 } else { 100.0 },
                64.0,
            ),
        });
        for item in &text_items {
            scene.insert(item.clone());
        }
        scene.insert(aqua_core::scene::DisplayItem::PopClip);
        if nested {
            scene.insert(aqua_core::scene::DisplayItem::PopClip);
        }
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).unwrap();
        captures.push(renderer.readback().unwrap());
    }
    assert_eq!(captures[0], captures[1]);
}
