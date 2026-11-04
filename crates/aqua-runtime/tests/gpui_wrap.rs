use aqua_core::{ColorProfile, DisplayItem};
use aqua_runtime::Runtime;
use aqua_text::{CaretAffinity, CaretPosition, RunId, SharedFonts, SharedRuns, TextStyle};
use aqua_ui::element::ui::Text;
use aqua_ui::layout::{ProfiledCosmicMeasurer, TextMeasurer};
use aqua_ui::style::{Style, px};

#[test]
fn gpui_wrap_measurement_and_retained_carets_share_visual_lines() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let style = TextStyle::default();
    let measurer = ProfiledCosmicMeasurer {
        fonts: fonts.clone(),
        scale: 1.0,
        gpui_line_height: true,
        gpui_wrap: true,
    };
    let measured = measurer.measure_styled("alpha beta gamma", &style, &[], Some(55.0), true);
    let mut app =
        Runtime::new(fonts, runs.clone()).with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.mount(|_| {
        Text::new("alpha beta gamma")
            .wrap(true)
            .text_style(style)
            .style(Style::new().width(px(55.0)))
            .build()
    })
    .unwrap();
    app.set_viewport(200.0, 200.0, 1.0);
    app.build_frame().unwrap();
    let run = app
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
        })
        .unwrap();
    let registry = runs.lock();
    let layout = registry
        .resolve_expected(RunId(run.id), run.revision)
        .unwrap()
        .layout;
    let laid = layout.laid_out().unwrap();
    assert!(laid.lines.len() > 1);
    assert!((laid.size.1 - measured.h).abs() < 0.001);
    let boundary = laid.lines[1].display_range.start;
    let up = layout
        .caret_position(CaretPosition {
            byte: boundary,
            affinity: CaretAffinity::Upstream,
        })
        .unwrap();
    let down = layout
        .caret_position(CaretPosition {
            byte: boundary,
            affinity: CaretAffinity::Downstream,
        })
        .unwrap();
    assert!(down.y_top > up.y_top);
    drop(registry);
    let node = app.tree().root().unwrap();
    let frontier = app
        .text_frontier(node, boundary, CaretAffinity::Downstream)
        .unwrap();
    assert!((frontier.line_bottom - (down.y_top + down.height)).abs() < 0.001);
    app = app.with_color_profile(ColorProfile::AquaLinear);
    app.build_frame().unwrap();
    let ordinary_run = app
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
        })
        .unwrap();
    let registry = runs.lock();
    let ordinary = registry
        .resolve_expected(RunId(ordinary_run.id), ordinary_run.revision)
        .unwrap()
        .layout;
    assert!(ordinary.laid_out().unwrap().lines.len() > 1);
    drop(registry);
    app = app.with_color_profile(ColorProfile::GpuiWindowsEncoded);
    app.build_frame().unwrap();
    let restored_run = app
        .scene()
        .ordered()
        .find_map(|(_, item)| match item {
            DisplayItem::Text { run, .. } => Some(run.clone()),
            _ => None,
        })
        .unwrap();
    let registry = runs.lock();
    let restored = registry
        .resolve_expected(RunId(restored_run.id), restored_run.revision)
        .unwrap()
        .layout;
    assert!(restored.laid_out().unwrap().lines.len() > 1);
}
