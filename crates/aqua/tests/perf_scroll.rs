use std::time::Instant as WallInstant;

use aqua::render::lower::Lowering;
use aqua_core::{Point, Size};
use aqua_graph::PaintCommand;
use aqua_protocol::Instant;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll as UiScroll, Text as UiText};
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn mount(runtime: &mut Runtime, rows: usize) {
    runtime
        .mount(|_| {
            UiScroll::new()
                .style(Style::new().width(px(1200.0)).height(px(800.0)))
                .child(
                    UiBox::new()
                        .style(
                            Style::new()
                                .axis(Axis::Column)
                                .width(px(760.0))
                                .height(px(40000.0)),
                        )
                        .children((0..rows).map(|index| {
                            UiBox::new()
                                .style(Style::new().width(px(760.0)))
                                .child(
                                    UiText::builder()
                                        .arg(format!(
                                            "Message {index}. {}",
                                            PARAGRAPH.repeat(1 + index % 4)
                                        ))
                                        .wrap(true)
                                        .style(Style::new().width(px(720.0)))
                                        .build(),
                                )
                                .build()
                        }))
                        .build(),
                )
                .build()
        })
        .unwrap();
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * fraction).round() as usize]
}

#[test]
#[ignore]
fn scroll_lowering_report() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    mount(&mut runtime, 400);
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    let mut lowering = Lowering::new(fonts, runs);
    let size = Size::new(1200.0, 800.0);
    let mut build = Vec::new();
    let mut lower = Vec::new();
    let mut items = 0;
    let mut commands = 0;
    let mut glyphs = 0;
    let mut moved = 0;
    for step in 0..400 {
        let started = WallInstant::now();
        runtime
            .tree_mut()
            .scroll_to(scroll, Point::new(0.0, 3.0 + step as f32 * 7.0));
        runtime.build_frame().unwrap();
        build.push(started.elapsed().as_secs_f64() * 1e6);
        let started = WallInstant::now();
        let (out, _, report) =
            lowering
                .at(Instant::ZERO)
                .commands(runtime.scene(), size, 1.0, Instant::ZERO);
        lower.push(started.elapsed().as_secs_f64() * 1e6);
        items = report.items;
        glyphs = report.glyphs;
        moved = runtime.tree().scroll_offset(scroll).unwrap().y as i32;
        commands = out.len();
    }
    build.sort_by(f64::total_cmp);
    lower.sort_by(f64::total_cmp);
    println!(
        "perf-scroll items={items} commands={commands} glyphs={glyphs} offset={moved} build p50={:.0} p90={:.0} lower p50={:.0} p90={:.0} us",
        percentile(&build, 0.5),
        percentile(&build, 0.9),
        percentile(&lower, 0.5),
        percentile(&lower, 0.9)
    );
}

fn placed(commands: &[PaintCommand]) -> Vec<(i32, i32, i32, i32, u32)> {
    commands
        .iter()
        .flat_map(|command| match command {
            PaintCommand::Glyphs(run) => run
                .glyphs
                .iter()
                .map(|glyph| {
                    (
                        (glyph.bounds.x * 4.0) as i32,
                        (glyph.bounds.y * 4.0) as i32,
                        glyph.bounds.w as i32,
                        glyph.bounds.h as i32,
                        glyph.color.l.to_bits(),
                    )
                })
                .collect(),
            _ => Vec::new(),
        })
        .collect()
}

#[test]
fn cached_runs_match_fresh_lowering_after_scroll() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    mount(&mut runtime, 60);
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    let size = Size::new(1200.0, 800.0);
    let mut retained = Lowering::new(fonts.clone(), runs.clone());
    for offset in [0.0, 5.0, 37.0, 37.5, 120.0, 121.0] {
        runtime
            .tree_mut()
            .scroll_to(scroll, Point::new(0.0, offset));
        runtime.build_frame().unwrap();
        let (cached, _, _) = retained.commands(runtime.scene(), size, 1.0, Instant::ZERO);
        let mut fresh = Lowering::new(fonts.clone(), runs.clone());
        let (expected, _, _) = fresh.commands(runtime.scene(), size, 1.0, Instant::ZERO);
        assert_eq!(placed(&cached), placed(&expected), "offset {offset}");
    }
}
