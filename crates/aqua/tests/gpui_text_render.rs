use std::time::Duration;

use aqua::{ui::Text, Instant, UiApp};
use aqua_core::scene::{DisplayItem, GpuiTextColorMap, GpuiTextShimmer};
use aqua_core::{ColorProfile, GpuiHsla, GpuiTextColor, Rect, Size};
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns, StyledSpan};

mod support;

const SIZE: Size = Size { w: 160.0, h: 64.0 };

fn encoded_app(fonts: &SharedFonts, runs: &SharedRuns) -> UiApp {
    UiApp::from_runtime(
        Runtime::new(fonts.clone(), runs.clone())
            .with_color_profile(ColorProfile::GpuiWindowsEncoded),
    )
}

fn build(app: &mut UiApp) {
    app.set_viewport(SIZE.w, SIZE.h, 1.0);
    app.build_frame().unwrap();
}

fn white() -> GpuiTextColor {
    GpuiTextColor {
        hue_degrees: 0.0,
        saturation: 0.0,
        lightness: 1.0,
        alpha: 1.0,
    }
}

fn black() -> GpuiTextColor {
    GpuiTextColor {
        hue_degrees: 0.0,
        saturation: 0.0,
        lightness: 0.0,
        alpha: 1.0,
    }
}

fn painters(fonts: &SharedFonts, runs: &SharedRuns) -> Vec<support::Painter> {
    let painters = support::painters(fonts, runs);
    #[cfg(windows)]
    assert!(painters.len() > 1, "no D3D12 adapter available");
    painters
}

#[test]
fn source_text_color_and_spans_reach_glyph_pixels() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    for mut painter in painters(&fonts, &runs) {
        let mut app = encoded_app(&fonts, &runs);
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
        build(&mut app);
        let frame = painter.paint(app.scene(), SIZE, 1.0);
        assert!(painter.report.glyphs > 0, "{}", painter.label);
        assert!(
            frame.pixels().any(|p| p[3] > 32
                && u16::from(p[0]) > u16::from(p[1]) * 2
                && u16::from(p[0]) > u16::from(p[2]) * 2),
            "{}: red glyphs",
            painter.label
        );
        assert!(
            frame.pixels().any(|p| p[3] > 200
                && u16::from(p[1]) > u16::from(p[0]) * 2
                && u16::from(p[1]) > u16::from(p[2]) * 2),
            "{}: green span background",
            painter.label
        );

        let mut raw = encoded_app(&fonts, &runs);
        raw.mount(|_| {
            Text::new("Raw spans")
                .color(aqua_core::Oklch::TRANSPARENT)
                .spans(vec![
                    StyledSpan::plain("Raw spans").gpui_color([0.0, 1.0, 0.5, 1.0])
                ])
                .build()
        })
        .unwrap();
        build(&mut raw);
        let frame = painter.paint(raw.scene(), SIZE, 1.0);
        assert!(
            frame.pixels().any(|p| p[3] > 32 && p[0] > p[1]),
            "{}: raw span color",
            painter.label
        );
    }
}

fn mapped_box(spread: f32) -> impl FnOnce(&aqua_reactive::Scope) -> aqua_ui::element::Element {
    move |_| {
        let map = GpuiTextColorMap {
            bounds: Rect::from_edges(0.0, 0.0, 60.0, 40.0),
            phase: 0.5,
            spread,
            target: black(),
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
                    .gpui_source_color(GpuiTextColor {
                        hue_degrees: -37.1234,
                        saturation: 0.4,
                        lightness: 0.9,
                        alpha: 1.0,
                    })
                    .build(),
            )
            .build()
    }
}

#[test]
fn text_color_map_darkens_inside_the_clipped_transformed_box_only() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    for mut painter in painters(&fonts, &runs) {
        let mut captures = Vec::new();
        for spread in [0.0, 100.0] {
            let mut app = encoded_app(&fonts, &runs);
            app.mount(mapped_box(spread)).unwrap();
            build(&mut app);
            captures.push(painter.paint(app.scene(), SIZE, 1.0));
        }
        assert!(
            captures[0]
                .pixels()
                .zip(captures[1].pixels())
                .any(|(base, mapped)| base[3] > 80
                    && mapped[3] > 80
                    && u16::from(mapped[0]) + 10 < u16::from(base[0])),
            "{}: spread changes color",
            painter.label
        );
        for (index, pixel) in captures[1].pixels().enumerate() {
            let x = index % SIZE.w as usize;
            if !(20..80).contains(&x) {
                assert_eq!(pixel[3], 0, "{}: clipped at x={x}", painter.label);
            }
        }
    }
}

fn shimmer_app(fonts: &SharedFonts, runs: &SharedRuns) -> UiApp {
    let mut app = encoded_app(fonts, runs);
    app.mount(|_| {
        let shimmer = GpuiTextShimmer {
            key: 7,
            duration: Duration::from_secs(3),
            delay: Duration::from_millis(1500),
            map: GpuiTextColorMap {
                bounds: Rect::from_edges(0.0, 0.0, 120.0, 40.0),
                phase: 0.0,
                spread: 50.0,
                target: black(),
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
                    .gpui_source_color(white())
                    .build(),
            )
            .build()
    })
    .unwrap();
    build(&mut app);
    app
}

#[test]
fn shimmer_holds_during_delay_moves_afterwards_and_reports_activity() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    for mut painter in painters(&fonts, &runs) {
        let app = shimmer_app(&fonts, &runs);
        let at = |ms: u64| Instant::from_nanos(ms * 1_000_000);
        let start = painter.paint_at(app.scene(), SIZE, 1.0, at(0));
        assert!(painter.report.shimmering, "{}", painter.label);
        let still_held = painter.paint_at(app.scene(), SIZE, 1.0, at(1000));
        assert_eq!(
            start.rgba8, still_held.rgba8,
            "{}: held during delay",
            painter.label
        );
        let released = painter.paint_at(app.scene(), SIZE, 1.0, at(1500));
        assert_eq!(
            start.rgba8, released.rgba8,
            "{}: sweep starts at phase zero",
            painter.label
        );
        let moving = painter.paint_at(app.scene(), SIZE, 1.0, at(3000));
        assert_ne!(
            start.rgba8, moving.rgba8,
            "{}: shimmer moves",
            painter.label
        );
        let finished = painter.paint_at(app.scene(), SIZE, 1.0, at(4500));
        assert_eq!(
            start.rgba8, finished.rgba8,
            "{}: sweep completes and waits again",
            painter.label
        );
    }
}

#[test]
fn nested_content_masks_equal_the_single_intersection() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = encoded_app(&fonts, &runs);
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
    build(&mut app);
    let text_items: Vec<_> = app
        .scene()
        .ordered()
        .map(|(_, item)| item.clone())
        .collect();
    for mut painter in painters(&fonts, &runs) {
        let captures: Vec<_> = [false, true]
            .into_iter()
            .map(|nested| {
                let mut scene = aqua_scene::Scene::new();
                scene.insert(DisplayItem::GpuiBackground {
                    bounds: Rect::from_edges(0.0, 0.0, 160.0, 64.0),
                    corners: aqua_core::CornerRadii::uniform(0.0),
                    background: aqua_core::GpuiBackground::Solid(GpuiHsla {
                        h: 0.0,
                        s: 0.0,
                        l: 1.0,
                        a: 1.0,
                    }),
                });
                if nested {
                    scene.insert(DisplayItem::PushGpuiContentMask {
                        bounds: Rect::from_edges(0.0, 0.0, 100.0, 64.0),
                    });
                }
                scene.insert(DisplayItem::PushGpuiContentMask {
                    bounds: Rect::from_edges(20.0, 0.0, if nested { 160.0 } else { 100.0 }, 64.0),
                });
                text_items.iter().for_each(|item| {
                    scene.insert(item.clone());
                });
                scene.insert(DisplayItem::PopClip);
                if nested {
                    scene.insert(DisplayItem::PopClip);
                }
                painter.paint(&scene, SIZE, 1.0)
            })
            .collect();
        assert!(painter.report.unsupported() == 0, "{}", painter.label);
        assert_eq!(captures[0].rgba8, captures[1].rgba8, "{}", painter.label);
    }
}

#[test]
fn transformed_scroll_child_keeps_layout_pre_transform_and_visual_rects_distinct() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut app = encoded_app(&fonts, &runs);
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
                            .child(Text::new("MMMMMMMMMMMMMMMM").build())
                            .build(),
                    )
                    .build(),
            )
            .build()
    })
    .unwrap();
    build(&mut app);
    let root = app.tree().root().unwrap();
    let scroll = app.tree().get(root).unwrap().children[0];
    let child = app.tree().get(scroll).unwrap().children[0];
    let layout = app.tree().rect(child).unwrap();
    let bounds = app.tree().pre_transform_rect(child).unwrap();
    let visual = app.tree().visual_rect(child).unwrap();
    assert_eq!(bounds.x, layout.x - 20.0);
    assert_eq!(visual.x, bounds.x + 20.0);
    assert_ne!(bounds.x, 0.0);

    let shimmer = GpuiTextShimmer {
        key: 99,
        duration: Duration::from_secs(2),
        delay: Duration::ZERO,
        map: GpuiTextColorMap {
            bounds,
            phase: 0.0,
            spread: 50.0,
            target: black(),
        },
    };
    let mut paint = app.tree().paint(child).unwrap();
    paint.gpui_text_shimmer = Some(shimmer);
    assert!(app.tree_mut().set_paint(child, paint));
    app.build_frame().unwrap();
    assert!(app
        .scene()
        .ordered()
        .any(|(_, item)| matches!(item, DisplayItem::PushGpuiTextShimmer(found) if found.map.bounds == bounds)));

    assert!(app
        .tree_mut()
        .scroll_to(scroll, aqua_core::Point::new(10.0, 0.0)));
    assert_eq!(
        app.tree().pre_transform_rect(child).unwrap().x,
        layout.x - 10.0
    );
    assert_eq!(app.tree().visual_rect(child).unwrap().x, layout.x + 10.0);
}
