use aqua_render::text::gpui::RenderingParameters;

#[test]
fn coverage_uses_display_parameters_and_subpixel_channel_order() {
    let params = RenderingParameters {
        gamma_ratios: [0.0; 4],
        grayscale_enhanced_contrast: 1.0,
        subpixel_enhanced_contrast: 1.0,
        is_bgr: false,
    };
    assert_eq!(params.validate(), Ok(params));
    let black = [0.0; 3];
    let white = [1.0; 3];
    assert_eq!(params.grayscale_coverage(0.5, black), 2.0 / 3.0);
    assert_eq!(params.grayscale_coverage(0.5, white), 0.5);
    let sample = [0.25, 0.5, 0.75];
    let normal = params.subpixel_coverage(sample, white);
    let reversed = RenderingParameters {
        is_bgr: true,
        ..params
    }
    .subpixel_coverage(sample, white);
    assert_eq!(normal, sample);
    assert_eq!(reversed, [0.75, 0.5, 0.25]);
    assert!(RenderingParameters {
        grayscale_enhanced_contrast: f32::NAN,
        ..params
    }
    .validate()
    .is_err());
}

#[test]
fn encoded_float_glyphs_reuse_atlas_and_honor_clip_and_transform() {
    use aqua_core::{ColorProfile, GpuiHsla};
    use aqua_render::text::{
        resolve_gpui_scene_text, GlyphonProvider, TextBackend, TextFrame, TextProvider,
    };
    use aqua_render::{Rect, TextRun};
    use aqua_text::{FontFamily, SharedFonts, SharedRuns, StyledSpan, TextLayout, TextStyle};

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
        Err(error) => {
            assert!(
                std::env::var_os("AQUA_REQUIRE_GPU").is_none(),
                "GPU adapter unavailable: {error:?}"
            );
            eprintln!("SKIP: GPU adapter unavailable: {error:?}");
            return;
        }
    };
    assert!(
        adapter
            .features()
            .contains(wgpu::Features::DUAL_SOURCE_BLENDING | wgpu::Features::CLIP_DISTANCES),
        "GPUI text requires dual-source blending and clip distances"
    );
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("gpui-float-text-test"),
        required_features: wgpu::Features::DUAL_SOURCE_BLENDING | wgpu::Features::CLIP_DISTANCES,
        required_limits: wgpu::Limits::default(),
        experimental_features: wgpu::ExperimentalFeatures::default(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("request dual-source GPU device");
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let style = TextStyle {
        size_px: 28.0,
        line_height_px: 34.0,
        ..Default::default()
    };
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::new("H", style.clone()).gpui_source_color([0.0, 1.0, 0.5, 1.0]),
        StyledSpan::new("H", style).gpui_source_color([120.0, 1.0, 0.5, 1.0]),
    ]);
    layout.layout(&mut fonts.lock());
    let (id, revision) = runs.lock().insert(layout);
    let mut provider = GlyphonProvider::new(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8Unorm,
        TextBackend::new(fonts.clone(), runs.clone()),
    );
    provider
        .set_color_profile(
            &device,
            &queue,
            wgpu::TextureFormat::Rgba8Unorm,
            ColorProfile::GpuiWindowsEncoded,
        )
        .expect("activate float GPUI provider");
    let range_opacity = std::cell::RefCell::new(None);
    let color_map = std::cell::RefCell::new(None);
    let render = |provider: &mut GlyphonProvider,
                  id: aqua_text::RunId,
                  revision,
                  opacity,
                  clip: Option<Rect>,
                  scale: f32| {
        let texture = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("gpui-float-text-target"),
            size: wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("gpui-float-text-readback"),
            size: 64 * 64 * 4,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("gpui-float-text-encoder"),
        });
        {
            let _pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("clear"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
        }
        let mut run = resolve_gpui_scene_text(
            &TextRun { id: id.0, revision },
            [4.0, 4.0],
            GpuiHsla {
                h: 0.0,
                s: 0.0,
                l: 1.0,
                a: 1.0,
            },
            opacity,
            clip,
            1.0,
        );
        run.gpui_color = None;
        run.gpui_text_opacity = range_opacity.borrow().clone();
        run.gpui_text_color_map = *color_map.borrow();
        run.gpui_subpixel = Some(true);
        run.scale = scale;
        provider.begin_frame();
        provider.queue_run(run);
        let stats = provider.encode_queued(TextFrame {
            device: &device,
            queue: &queue,
            encoder: &mut encoder,
            target: &view,
            width: 64,
            height: 64,
        });
        assert_eq!(stats.raster_failures, 0, "real float glyph draw");
        assert_eq!(stats.areas, 1);
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(64),
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let result = buffer.slice(..).get_mapped_range().unwrap().to_vec();
        buffer.unmap();
        result
    };
    let full = render(&mut provider, id, revision, 1.0, None, 1.0);
    assert!(full
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[0] > pixel[1] && pixel[3] > 0));
    assert!(full
        .as_chunks::<4>()
        .0
        .iter()
        .any(|pixel| pixel[1] > pixel[0] && pixel[3] > 0));
    assert_eq!(
        full,
        render(&mut provider, id, revision, 1.0, None, 1.0),
        "cached glyphs preserve exact pixels"
    );
    for mapped in [false, true] {
        *color_map.borrow_mut() = mapped.then_some(aqua_core::scene::GpuiTextColorMap {
            bounds: aqua_core::Rect::from_edges(0.0, 0.0, 64.0, 64.0),
            phase: 0.5,
            spread: 100.0,
            target: aqua_core::GpuiTextColor {
                hue_degrees: 0.0,
                saturation: 0.0,
                lightness: 1.0,
                alpha: 1.0,
            },
        });
        let before = render(&mut provider, id, revision, 1.0, None, 1.0);
        *range_opacity.borrow_mut() = Some(std::sync::Arc::from([
            aqua_core::scene::GpuiTextOpacityRange {
                bytes: 1..2,
                opacity: 0.0,
            },
        ]));
        let after = render(&mut provider, id, revision, 1.0, None, 1.0);
        *range_opacity.borrow_mut() = Some(std::sync::Arc::from([
            aqua_core::scene::GpuiTextOpacityRange {
                bytes: 0..1,
                opacity: 0.0,
            },
        ]));
        let second_only = render(&mut provider, id, revision, 1.0, None, 1.0);
        let mut affected = 0;
        let mut preserved = 0;
        for ((full, first), second) in before
            .as_chunks::<4>()
            .0
            .iter()
            .zip(after.as_chunks::<4>().0)
            .zip(second_only.as_chunks::<4>().0)
        {
            if first[3] == 0 && second[3] > 0 {
                affected += 1;
                assert_eq!(first[3], 0, "faded glyph must become transparent");
                assert_eq!(full, second, "remaining glyph must match its isolated draw");
            }
            if first[3] > 0 && second[3] == 0 {
                preserved += 1;
                assert_eq!(full, first, "outside opacity range must remain unchanged");
            }
        }
        assert!(
            affected > 0 && preserved > 0,
            "each glyph must have independently painted pixels"
        );
        *range_opacity.borrow_mut() = None;
    }
    *color_map.borrow_mut() = None;
    let clipped = render(
        &mut provider,
        id,
        revision,
        1.0,
        Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 20.0,
            h: 64.0,
        }),
        1.0,
    );
    for y in 0..64 {
        for x in 20..64 {
            assert_eq!(&clipped[(y * 64 + x) * 4..][..4], &[0, 0, 0, 0]);
        }
    }
    let fractional = render(
        &mut provider,
        id,
        revision,
        1.0,
        Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 20.25,
            h: 64.0,
        }),
        1.0,
    );
    for y in 0..64 {
        for x in 0..64 {
            let pixel = (y * 64 + x) * 4;
            if x < 21 {
                assert_eq!(&fractional[pixel..pixel + 4], &full[pixel..pixel + 4]);
            } else {
                assert_eq!(&fractional[pixel..pixel + 4], &[0, 0, 0, 0]);
            }
        }
    }
    let transformed = render(&mut provider, id, revision, 1.0, None, 1.25);
    assert_ne!(
        full, transformed,
        "transform scale changes rendered geometry"
    );
    let transformed_clip = render(
        &mut provider,
        id,
        revision,
        1.0,
        Some(Rect {
            x: 0.0,
            y: 0.0,
            w: 20.25,
            h: 64.0,
        }),
        1.25,
    );
    for y in 0..64 {
        for x in 0..64 {
            let pixel = (y * 64 + x) * 4;
            if x < 21 {
                assert_eq!(
                    &transformed_clip[pixel..pixel + 4],
                    &transformed[pixel..pixel + 4]
                );
            } else {
                assert_eq!(&transformed_clip[pixel..pixel + 4], &[0, 0, 0, 0]);
            }
        }
    }
    if let Some(path) = std::env::var_os("AQUA_TEST_EMOJI_FONT") {
        fonts
            .lock()
            .load_font_file(path)
            .expect("load color emoji font");
        let mut emoji = TextLayout::new();
        emoji.set_spans(vec![StyledSpan::new(
            "😀",
            TextStyle {
                families: vec![FontFamily::Named("Segoe UI Emoji".into())],
                size_px: 28.0,
                line_height_px: 34.0,
                ..Default::default()
            },
        )]);
        emoji.layout(&mut fonts.lock());
        let (emoji_id, emoji_revision) = runs.lock().insert(emoji);
        let color = render(&mut provider, emoji_id, emoji_revision, 1.0, None, 1.0);
        assert!(
            color
                .as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| { pixel[3] > 0 && (pixel[0] != pixel[1] || pixel[1] != pixel[2]) }),
            "color emoji retains its source colors without a foreground color"
        );
        assert_eq!(
            color,
            render(&mut provider, emoji_id, emoji_revision, 1.0, None, 1.0)
        );
        *range_opacity.borrow_mut() = Some(std::sync::Arc::from([
            aqua_core::scene::GpuiTextOpacityRange {
                bytes: 0..4,
                opacity: 0.0,
            },
        ]));
        assert_eq!(
            color,
            render(&mut provider, emoji_id, emoji_revision, 1.0, None, 1.0),
            "text opacity ranges do not fade color emoji"
        );
        *range_opacity.borrow_mut() = None;
        let transparent = render(&mut provider, emoji_id, emoji_revision, 0.0, None, 1.0);
        assert!(
            transparent.iter().all(|byte| *byte == 0),
            "element opacity applies to color glyphs"
        );
        #[cfg(feature = "gpui-diagnostics")]
        if let Some(body_font) = std::env::var_os("AQUA_TEST_BODY_FONT") {
            use aqua_render::text::gpui::{set_glyph_diagnostic, GlyphDiagnostic};
            fonts
                .lock()
                .load_font_file(body_font)
                .expect("load body font");
            let text =
                "Family 👨‍👩‍👧‍👦 VS ☀️ flag 🏴\u{e0067}\u{e0062}\u{e0065}\u{e006e}\u{e0067}\u{e007f}";
            let mut mixed = TextLayout::new();
            mixed.set_spans(vec![StyledSpan::new(
                text,
                TextStyle {
                    families: vec![FontFamily::Named("Spline Sans".into())],
                    size_px: 16.0,
                    line_height_px: 22.0,
                    ..Default::default()
                },
            )
            .gpui_source_color([0.0, 0.0, 0.0, 1.0])]);
            mixed.layout(&mut fonts.lock());
            let (mixed_id, mixed_revision) = runs.lock().insert(mixed);
            let select = |glyph_ids| {
                set_glyph_diagnostic(Some(GlyphDiagnostic {
                    text: text.to_owned(),
                    glyph_ids,
                }))
                .unwrap()
            };
            select(vec![1271, 1259, 117]);
            let forward = render(&mut provider, mixed_id, mixed_revision, 1.0, None, 0.5);
            select(vec![117, 1259, 1271]);
            let reverse = render(&mut provider, mixed_id, mixed_revision, 1.0, None, 0.5);
            assert_eq!(
                forward, reverse,
                "cached same-layer sprites follow source kind and tile order"
            );
            select(vec![1271, 1259]);
            let emoji_only = render(&mut provider, mixed_id, mixed_revision, 1.0, None, 0.5);
            assert_ne!(
                forward, emoji_only,
                "the LCD glyph must contribute to the mixed draw"
            );
            assert!(
                emoji_only
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .any(|p| p[3] > 0 && (p[0] != p[1] || p[1] != p[2])),
                "fixture must contain color emoji"
            );
            set_glyph_diagnostic(None).unwrap();
        }
    } else {
        eprintln!("SKIP: set AQUA_TEST_EMOJI_FONT to Segoe UI Emoji to qualify color glyphs");
    }
    for text in ["   ", " \u{034f} "] {
        let mut inkless = TextLayout::new();
        inkless.set_spans(vec![
            StyledSpan::new(text, TextStyle::default()).gpui_color([0.0, 0.0, 0.0, 1.0])
        ]);
        inkless.layout(&mut fonts.lock());
        let (inkless_id, inkless_revision) = runs.lock().insert(inkless);
        let pixels = render(&mut provider, inkless_id, inkless_revision, 1.0, None, 1.0);
        assert!(
            pixels.iter().all(|byte| *byte == 0),
            "inkless text must not paint pixels: {text:?}"
        );
    }
    let mut wrapped = TextLayout::new();
    wrapped.set_spans(vec![StyledSpan::new(
        "one two three",
        TextStyle {
            size_px: 12.0,
            line_height_px: 16.0,
            ..Default::default()
        },
    )
    .gpui_source_color([0.0, 0.0, 0.0, 1.0])]);
    wrapped.set_line_mode(aqua_text::LineMode::Multi);
    wrapped.set_wrap(aqua_text::WrapMode::Word);
    wrapped.set_max_width(Some(32.0));
    wrapped.set_gpui_wrap(true);
    let laid = wrapped.layout(&mut fonts.lock());
    assert!(
        laid.lines.len() >= 2,
        "fixture must wrap into multiple visual lines"
    );
    let bands: Vec<_> = laid
        .lines
        .iter()
        .take(2)
        .map(|line| (line.y_top, line.height))
        .collect();
    let (wrapped_id, wrapped_revision) = runs.lock().insert(wrapped);
    let wrapped_pixels = render(&mut provider, wrapped_id, wrapped_revision, 1.0, None, 1.0);
    for (top, height) in bands {
        let start = (top + 4.0).floor().max(0.0) as usize;
        let end = (top + height + 4.0).ceil().min(64.0) as usize;
        assert!(
            (start..end).any(|y| (0..64).any(|x| wrapped_pixels[(y * 64 + x) * 4 + 3] != 0)),
            "each visual line must paint its own glyphs"
        );
    }
    let mut uncovered = TextLayout::new();
    uncovered.set_spans(vec![StyledSpan::plain("uncovered")]);
    uncovered.layout(&mut fonts.lock());
    let (uncovered_id, uncovered_revision) = runs.lock().insert(uncovered);
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gpui-uncovered-target"),
        size: wgpu::Extent3d {
            width: 64,
            height: 64,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("gpui-uncovered"),
    });
    let mut run = resolve_gpui_scene_text(
        &TextRun {
            id: uncovered_id.0,
            revision: uncovered_revision,
        },
        [0.0, 0.0],
        GpuiHsla {
            h: 0.0,
            s: 0.0,
            l: 1.0,
            a: 1.0,
        },
        1.0,
        None,
        1.0,
    );
    run.gpui_color = None;
    run.gpui_subpixel = Some(true);
    provider.queue_run(run);
    let stats = provider.encode_queued(TextFrame {
        device: &device,
        queue: &queue,
        encoder: &mut encoder,
        target: &view,
        width: 64,
        height: 64,
    });
    assert_eq!(
        stats.raster_failures, 1,
        "uncovered glyph without source color is rejected"
    );
}

#[test]
fn shimmer_preserves_source_color_outside_band_and_alpha_inside() {
    use aqua_render::text::gpui::map::{indistinguishable, shimmer_color};

    let base = [0.123, 0.8, 0.4, 0.37];
    let target = [215.0, 0.7, 0.8, 0.1];
    assert_eq!(shimmer_color(base, target, 0.0, 0.0, 50.0), base);
    assert_eq!(shimmer_color(base, target, 0.5, 0.0, 0.0), base);
    let center = shimmer_color(base, target, 0.75, 0.5, 50.0);
    assert_ne!(center, base);
    assert_eq!(center[3].to_bits(), base[3].to_bits());
    assert_eq!(center, shimmer_color(base, target, 0.75, 1.5, 50.0));
    assert!(indistinguishable(center, center));
    assert!(!indistinguishable(base, center));
}

#[test]
fn emoji_remnant_does_not_displace_following_glyph_or_ordinary_space() {
    use aqua_render::text::gpui::pairing::visible_glyph_indices;
    let glyphs = [(8, true), (3, true), (3, false), (12, false)];
    assert_eq!(visible_glyph_indices(glyphs, [8, 3, 12]), Ok(vec![0, 2, 3]));
    assert!(visible_glyph_indices(glyphs, [8, 12, 3]).is_err());
    assert!(visible_glyph_indices(glyphs, [8, 12]).is_err());
}

#[cfg(feature = "gpui-diagnostics")]
#[test]
fn diagnostic_selection_rejects_empty_and_duplicate_requests() {
    use aqua_render::text::gpui::{set_glyph_diagnostic, GlyphDiagnostic};
    assert!(set_glyph_diagnostic(Some(GlyphDiagnostic {
        text: "family".into(),
        glyph_ids: vec![1271, 1271]
    }))
    .is_err());
    assert!(set_glyph_diagnostic(Some(GlyphDiagnostic {
        text: String::new(),
        glyph_ids: vec![1271]
    }))
    .is_err());
    assert!(set_glyph_diagnostic(Some(GlyphDiagnostic {
        text: "family".into(),
        glyph_ids: vec![1259, 1271]
    }))
    .is_ok());
    assert!(set_glyph_diagnostic(None).is_ok());
}
