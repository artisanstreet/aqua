use aqua_render::{DisplayItem, Oklch, Rect, RenderError, Renderer, Scene};

const WIDTH: usize = 96;
const HEIGHT: usize = 72;

type Pixel = [f32; 4];

struct ReferenceLayer {
    pixels: Vec<Pixel>,
    opacity: f32,
}

fn over(source: Pixel, target: Pixel) -> Pixel {
    std::array::from_fn(|channel| source[channel] + target[channel] * (1.0 - source[3]))
}

fn reference(items: &[DisplayItem]) -> Vec<Pixel> {
    let mut layers = vec![ReferenceLayer {
        pixels: vec![[0.0; 4]; WIDTH * HEIGHT],
        opacity: 1.0,
    }];
    let mut clips = Vec::<Rect>::new();
    for item in items {
        match item {
            DisplayItem::Rect {
                bounds,
                fill,
                border_width,
                border_color,
                radius,
                shadow,
            } => {
                assert_eq!(*radius, 0.0);
                assert!(shadow.is_none());
                let layer = layers.last_mut().unwrap();
                for y in 0..HEIGHT {
                    for x in 0..WIDTH {
                        let px = x as f32 + 0.5;
                        let py = y as f32 + 0.5;
                        if !contains(*bounds, px, py)
                            || clips.iter().any(|clip| !contains(*clip, px, py))
                        {
                            continue;
                        }
                        let inset = Rect {
                            x: bounds.x + border_width,
                            y: bounds.y + border_width,
                            w: bounds.w - border_width * 2.0,
                            h: bounds.h - border_width * 2.0,
                        };
                        let color = match contains(inset, px, py) {
                            true => fill,
                            false => border_color,
                        };
                        let [r, g, b, alpha] = color.to_linear();
                        let index = y * WIDTH + x;
                        layer.pixels[index] = over(
                            [r * alpha, g * alpha, b * alpha, alpha],
                            layer.pixels[index],
                        );
                    }
                }
            }
            DisplayItem::PushClip { bounds, radius } => {
                assert_eq!(*radius, 0.0);
                clips.push(*bounds);
            }
            DisplayItem::PopClip => {
                clips.pop().expect("balanced clips");
            }
            DisplayItem::PushOpacity(opacity) => layers.push(ReferenceLayer {
                pixels: vec![[0.0; 4]; WIDTH * HEIGHT],
                opacity: *opacity,
            }),
            DisplayItem::PopOpacity => {
                let source = layers.pop().expect("balanced opacity groups");
                let target = layers.last_mut().expect("parent layer");
                for (target, source_pixel) in target.pixels.iter_mut().zip(source.pixels) {
                    *target = over(
                        source_pixel.map(|channel| channel * source.opacity),
                        *target,
                    );
                }
            }
            _ => panic!(
                "reference supports axis-aligned rectangles, clips and isolated opacity only"
            ),
        }
    }
    assert!(clips.is_empty());
    assert_eq!(layers.len(), 1);
    layers.pop().unwrap().pixels
}

fn contains(bounds: Rect, x: f32, y: f32) -> bool {
    x >= bounds.x && x < bounds.x + bounds.w && y >= bounds.y && y < bounds.y + bounds.h
}

fn near_edge(bounds: Rect, x: f32, y: f32) -> bool {
    let edges = [bounds.x, bounds.x + bounds.w, bounds.y, bounds.y + bounds.h];
    (x - edges[0]).abs() < 3.0
        || (x - edges[1]).abs() < 3.0
        || (y - edges[2]).abs() < 3.0
        || (y - edges[3]).abs() < 3.0
}

fn antialias_boundary(items: &[DisplayItem], x: f32, y: f32) -> bool {
    items.iter().any(|item| match item {
        DisplayItem::Rect {
            bounds,
            border_width,
            ..
        } => {
            near_edge(*bounds, x, y)
                || (*border_width > 0.0
                    && near_edge(
                        Rect {
                            x: bounds.x + border_width,
                            y: bounds.y + border_width,
                            w: bounds.w - border_width * 2.0,
                            h: bounds.h - border_width * 2.0,
                        },
                        x,
                        y,
                    ))
        }
        DisplayItem::PushClip { bounds, .. } => near_edge(*bounds, x, y),
        _ => false,
    })
}

fn rectangle(bounds: Rect, color: &str, alpha: f32, border_width: f32) -> DisplayItem {
    DisplayItem::Rect {
        bounds,
        radius: 0.0,
        fill: Oklch::from_hex(color).unwrap().with_alpha(alpha),
        border_width,
        border_color: Oklch::from_hex("#e09030").unwrap().with_alpha(0.7),
        shadow: None,
    }
}

#[test]
fn clipped_nested_opacity_and_painter_order_match_cpu_compositor() {
    let mut renderer = match pollster::block_on(Renderer::headless(WIDTH as u32, HEIGHT as u32)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").ok().as_deref() != Some("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER: CPU conformance requires a GPU adapter");
            return;
        }
        Err(error) => panic!("GPU conformance setup failed: {error:?}"),
    };
    for offset in [0.0, 2.0, 4.0, 6.0] {
        let items = vec![
            rectangle(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 96.0,
                    h: 72.0,
                },
                "#253040",
                1.0,
                0.0,
            ),
            rectangle(
                Rect {
                    x: 8.0 + offset,
                    y: 8.0,
                    w: 72.0,
                    h: 56.0,
                },
                "#dd0030",
                0.6,
                5.0,
            ),
            DisplayItem::PushClip {
                bounds: Rect {
                    x: 16.0,
                    y: 12.0,
                    w: 68.0,
                    h: 48.0,
                },
                radius: 0.0,
            },
            DisplayItem::PushOpacity(0.65),
            rectangle(
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 56.0,
                    h: 64.0,
                },
                "#20c090",
                0.8,
                0.0,
            ),
            DisplayItem::PushOpacity(0.45),
            rectangle(
                Rect {
                    x: 32.0,
                    y: 4.0 + offset,
                    w: 64.0,
                    h: 64.0,
                },
                "#4060e0",
                0.9,
                0.0,
            ),
            rectangle(
                Rect {
                    x: 16.0,
                    y: 28.0,
                    w: 48.0,
                    h: 32.0,
                },
                "#ece030",
                0.7,
                0.0,
            ),
            DisplayItem::PopOpacity,
            DisplayItem::PushClip {
                bounds: Rect {
                    x: 40.0,
                    y: 20.0,
                    w: 32.0,
                    h: 28.0,
                },
                radius: 0.0,
            },
            rectangle(
                Rect {
                    x: 8.0,
                    y: 0.0,
                    w: 88.0,
                    h: 72.0,
                },
                "#a020e0",
                0.35,
                0.0,
            ),
            DisplayItem::PopClip,
            DisplayItem::PopOpacity,
            DisplayItem::PopClip,
            rectangle(
                Rect {
                    x: 4.0,
                    y: 52.0,
                    w: 84.0,
                    h: 16.0,
                },
                "#30e0b0",
                0.25,
                0.0,
            ),
        ];
        let expected = reference(&items);
        let mut scene = Scene::new();
        for item in &items {
            scene.insert(item.clone());
        }
        let plan = renderer.prepare(&mut scene);
        renderer.draw(&plan).unwrap();
        let actual = renderer.readback().unwrap();
        let mut compared = 0;
        for y in 0..HEIGHT {
            for x in 0..WIDTH {
                if antialias_boundary(&items, x as f32 + 0.5, y as f32 + 0.5) {
                    continue;
                }
                compared += 1;
                let index = y * WIDTH + x;
                for channel in 0..4 {
                    let expected =
                        (expected[index][channel].clamp(0.0, 1.0) * 255.0).round() as i16;
                    let actual = actual[index * 4 + channel] as i16;
                    assert!((actual - expected).abs() <= 4,
                        "offset {offset}, pixel ({x}, {y}), channel {channel}: GPU {actual}, CPU {expected}");
                }
            }
        }
        assert!(compared > 300, "insufficient interior coverage: {compared}");
    }
}

#[test]
fn grouped_text_respects_rectangles_after_it_and_isolates_nested_alpha() {
    use aqua_render::text::{GlyphonProvider, TextBackend};
    use aqua_render::TextRun;
    use aqua_text::{SharedFonts, SharedRuns, TextLayout, TextStyle};

    let mut renderer = match pollster::block_on(Renderer::headless(WIDTH as u32, HEIGHT as u32)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").ok().as_deref() != Some("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER: text composition conformance requires a GPU adapter");
            return;
        }
        Err(error) => panic!("GPU conformance setup failed: {error:?}"),
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut layout = TextLayout::new();
    layout.set_text(
        "HHHH",
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
    scene.insert(DisplayItem::PushOpacity(0.8));
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(DisplayItem::Text {
        at: [8.0, 8.0],
        run: TextRun { id: id.0, revision },
        color: Oklch::WHITE,
        opacity: 1.0,
    });
    scene.insert(DisplayItem::PopOpacity);
    scene.insert(DisplayItem::PopOpacity);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let visible = renderer.readback().unwrap();
    let ink: Vec<_> = visible
        .as_chunks::<4>().0.iter()
        .enumerate()
        .filter(|(_, pixel)| pixel[3] > 90)
        .map(|(index, _)| index)
        .collect();
    assert!(ink.len() > 40, "actual glyph coverage required");
    assert!(
        ink.iter().all(|index| visible[index * 4 + 3] <= 103),
        "nested group alpha must multiply"
    );
    scene = Scene::new();
    scene.insert(DisplayItem::PushOpacity(0.5));
    scene.insert(DisplayItem::Text {
        at: [8.0, 8.0],
        run: TextRun { id: id.0, revision },
        color: Oklch::WHITE,
        opacity: 1.0,
    });
    scene.insert(rectangle(
        Rect {
            x: 0.0,
            y: 0.0,
            w: WIDTH as f32,
            h: HEIGHT as f32,
        },
        "#000000",
        1.0,
        0.0,
    ));
    scene.insert(DisplayItem::PopOpacity);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let covered = renderer.readback().unwrap();
    for index in ink {
        assert_eq!(
            &covered[index * 4..index * 4 + 3],
            &[0, 0, 0],
            "later rectangle must cover text at pixel {index}"
        );
    }
}

#[test]
fn inline_span_colors_survive_group_and_text_opacity() {
    use aqua_render::text::{GlyphonProvider, TextBackend};
    use aqua_render::TextRun;
    use aqua_text::{SharedFonts, SharedRuns, StyledSpan, TextLayout, TextStyle};

    let mut renderer = match pollster::block_on(Renderer::headless(WIDTH as u32, HEIGHT as u32)) {
        Ok(renderer) => renderer,
        Err(RenderError::AdapterNotFound(_))
            if std::env::var("AQUA_REQUIRE_GPU").ok().as_deref() != Some("1") =>
        {
            eprintln!("SKIP-NO-ADAPTER: inline color conformance requires a GPU adapter");
            return;
        }
        Err(error) => panic!("GPU conformance setup failed: {error:?}"),
    };
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let style = TextStyle {
        size_px: 24.0,
        line_height_px: 28.0,
        ..Default::default()
    };
    let mut layout = TextLayout::new();
    layout.set_spans(vec![
        StyledSpan::new("HH ", style.clone()).color([255, 0, 0, 255]),
        StyledSpan::new("HH", style).color([0, 255, 0, 255]),
    ]);
    layout.layout(&mut fonts.lock());
    let (id, revision) = runs.lock().insert(layout);
    renderer.set_text_provider(Box::new(GlyphonProvider::new(
        renderer.device(),
        renderer.queue(),
        renderer.format(),
        TextBackend::new(fonts, runs),
    )));
    let mut scene = Scene::new();
    scene.insert(DisplayItem::PushOpacity(0.8));
    scene.insert(DisplayItem::Text {
        at: [4.0, 8.0],
        run: TextRun { id: id.0, revision },
        color: Oklch::from_hex("#0000ff").unwrap(),
        opacity: 0.5,
    });
    scene.insert(DisplayItem::PopOpacity);
    let plan = renderer.prepare(&mut scene);
    renderer.draw(&plan).unwrap();
    let pixels = renderer.readback().unwrap();
    let red = pixels
        .as_chunks::<4>().0.iter()
        .filter(|pixel| pixel[0] > 90 && pixel[1] < 3 && pixel[2] < 3)
        .count();
    let green = pixels
        .as_chunks::<4>().0.iter()
        .filter(|pixel| pixel[1] > 90 && pixel[0] < 3 && pixel[2] < 3)
        .count();
    assert!(
        red > 20 && green > 20,
        "inline glyph colors must override the blue default: red {red}, green {green}"
    );
    assert!(
        pixels.as_chunks::<4>().0.iter().all(|pixel| pixel[3] <= 103),
        "text and group opacity apply to explicit span colors"
    );
}
