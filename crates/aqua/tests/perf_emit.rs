use std::time::Instant as WallInstant;

use aqua::render::damage::DamagePlanner;
use aqua::render::lower::Lowering;
use aqua_core::{Oklch, Point, Size};
use aqua_graph::{plan_paint_with, Extent, PaintCommand, PlanLimits};
use aqua_protocol::Instant;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Scroll as UiScroll, Text as UiText};
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn color(hex: &str) -> Oklch {
    Oklch::from_hex(hex).unwrap()
}

fn mount(runtime: &mut Runtime, rows: usize) {
    runtime
        .mount(|_| {
            UiScroll::new()
                .style(Style::new().width(px(1200.0)).height(px(800.0)))
                .child(
                    UiBox::new()
                        .style(Style::new().axis(Axis::Column).width(px(760.0)))
                        .children((0..rows).map(|index| {
                            UiBox::new()
                                .style(Style::new().axis(Axis::Column).width(px(760.0)))
                                .fill(color("#26263a"))
                                .radius(8.0)
                                .border((1.0, color("#3a3a55")))
                                .children((0..3).map(move |part| {
                                    UiBox::new()
                                        .style(Style::new().axis(Axis::Column).width(px(740.0)))
                                        .fill(color(if part == 1 { "#1b1b2b" } else { "#2c2c44" }))
                                        .radius(4.0)
                                        .clip(part == 1)
                                        .child(
                                            UiText::builder()
                                                .arg(format!(
                                                    "Part {index}.{part} {}",
                                                    PARAGRAPH.repeat(1 + (index + part) % 2)
                                                ))
                                                .wrap(true)
                                                .style(Style::new().width(px(700.0)))
                                                .build(),
                                        )
                                        .build()
                                }))
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

fn line(name: &str, samples: &mut [f64]) -> String {
    samples.sort_by(f64::total_cmp);
    format!(
        "{name} p50={:.1} p90={:.1} max={:.1}",
        percentile(samples, 0.5),
        percentile(samples, 0.9),
        samples[samples.len() - 1]
    )
}

#[test]
#[ignore]
fn emit_phase_report() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    mount(&mut runtime, 200);
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    let mut lowering = Lowering::new(fonts, runs.clone());
    let mut planner = DamagePlanner::new();
    let size = Size::new(1200.0, 800.0);
    let extent = Extent::new(1200, 800);
    let (mut build, mut damage, mut lower, mut plan_us) = (vec![], vec![], vec![], vec![]);
    let (mut commands, mut passes, mut items, mut glyphs) = (0, 0, 0, 0);
    for step in 0..600 {
        runtime
            .tree_mut()
            .scroll_to(scroll, Point::new(0.0, 3.0 + step as f32 * 5.0));
        let started = WallInstant::now();
        runtime.build_frame().unwrap();
        build.push(started.elapsed().as_secs_f64() * 1e6);
        let started = WallInstant::now();
        planner.plan(runtime.scene(), &runs, size, 1.0, true);
        planner.commit();
        damage.push(started.elapsed().as_secs_f64() * 1e6);
        let started = WallInstant::now();
        let (out, _, report): (Vec<PaintCommand>, _, _) =
            lowering
                .at(Instant::ZERO)
                .commands(runtime.scene(), size, 1.0, Instant::ZERO);
        lower.push(started.elapsed().as_secs_f64() * 1e6);
        let started = WallInstant::now();
        let plan = plan_paint_with(extent, color("#1e1e2e"), &out, PlanLimits::default()).unwrap();
        plan_us.push(started.elapsed().as_secs_f64() * 1e6);
        commands = out.len();
        lowering.retain_frame(out);
        passes = plan.passes.len();
        items = report.items;
        glyphs = report.glyphs;
    }
    println!(
        "perf-emit items={items} commands={commands} passes={passes} glyphs={glyphs}\n  {}\n  {}\n  {}\n  {}",
        line("build_us", &mut build),
        line("damage_us", &mut damage),
        line("lower_us", &mut lower),
        line("plan_us", &mut plan_us)
    );
}

#[test]
#[ignore]
fn animated_frame_report() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut runtime = Runtime::new(fonts.clone(), runs.clone());
    runtime.set_viewport(1200.0, 800.0, 1.0);
    mount(&mut runtime, 200);
    runtime.build_frame().unwrap();
    let mut lowering = Lowering::new(fonts, runs.clone());
    let mut planner = DamagePlanner::new();
    let size = Size::new(1200.0, 800.0);
    let (mut warm, mut culled, mut damage) = (vec![], vec![], vec![]);
    let (mut full_items, mut full_commands, mut small_commands, mut small_culled) = (0, 0, 0, 0);
    planner.plan(runtime.scene(), &runs, size, 1.0, true);
    planner.commit();
    for step in 0..400 {
        let started = WallInstant::now();
        planner.plan(runtime.scene(), &runs, size, 1.0, true);
        damage.push(started.elapsed().as_secs_f64() * 1e6);
        lowering.set_cull(None);
        let started = WallInstant::now();
        let (out, _, report) =
            lowering
                .at(Instant::ZERO)
                .commands(runtime.scene(), size, 1.0, Instant::ZERO);
        warm.push(started.elapsed().as_secs_f64() * 1e6);
        full_items = report.items;
        full_commands = out.len();
        lowering.retain_frame(out);
        lowering.set_cull(Some(aqua_core::Rect {
            x: 400.0 + (step % 8) as f32,
            y: 300.0,
            w: 9.0,
            h: 9.0,
        }));
        let started = WallInstant::now();
        let (out, _, report) =
            lowering
                .at(Instant::ZERO)
                .commands(runtime.scene(), size, 1.0, Instant::ZERO);
        culled.push(started.elapsed().as_secs_f64() * 1e6);
        small_commands = out.len();
        small_culled = report.culled;
        lowering.retain_frame(out);
    }
    println!(
        "perf-animated items={full_items} full_commands={full_commands} small_commands={small_commands} culled={small_culled}\n  {}\n  {}\n  {}",
        line("full_lower_us", &mut warm),
        line("culled_lower_us", &mut culled),
        line("damage_us", &mut damage)
    );
}
