use std::time::Instant;

use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::collection::List;
use aqua_ui::element::{ui, Element};
use aqua_ui::style::{px, Axis, Style};
use aqua_ui::tree::NodeKind;

fn transcript_row(index: usize) -> Element {
    let paragraphs = 2 + index % 5;
    ui::Box::new()
        .style(Style::new().axis(Axis::Column).width(px(700.0)))
        .child(
            ui::Box::new()
                .style(Style::new().axis(Axis::Row).height(px(24.0)))
                .child(
                    ui::Box::new()
                        .style(Style::new().width(px(20.0)).height(px(20.0)))
                        .build(),
                )
                .child(
                    ui::Text::builder()
                        .arg(format!("Assistant {index}"))
                        .build(),
                )
                .build(),
        )
        .children((0..paragraphs).map(|paragraph| {
            ui::Box::new()
                .style(Style::new().axis(Axis::Column))
                .child(
                    ui::Text::builder()
                        .arg(format!(
                            "Row {index} paragraph {paragraph}. {}",
                            "Aqua keeps retained layout between frames. ".repeat(3 + paragraph)
                        ))
                        .wrap(true)
                        .build(),
                )
                .child(
                    ui::Box::new()
                        .style(Style::new().axis(Axis::Row).height(px(18.0)))
                        .child(ui::Text::builder().arg("copy").build())
                        .child(ui::Text::builder().arg("retry").build())
                        .build(),
                )
                .build()
        }))
        .build()
}

fn transcript(rows: usize) -> Element {
    List::new()
        .items(0..rows)
        .key(|index| *index)
        .row(|index| transcript_row(*index))
        .virtualize()
        .estimated_row_height(px(180.0))
        .overscan(0)
        .overscan_pixels(1600.0)
        .style(Style::new().width(px(760.0)).height(px(900.0)))
        .build()
        .with_key("transcript")
}

fn percentile(sorted: &[f64], fraction: f64) -> f64 {
    sorted[((sorted.len() as f64 - 1.0) * fraction).round() as usize]
}

#[test]
#[ignore]
fn collection_scroll_build_report() {
    let fonts = SharedFonts::new();
    let mut runtime = Runtime::new(fonts, SharedRuns::new());
    runtime.set_viewport(760.0, 900.0, 1.25);
    runtime.mount(|_| transcript(600)).unwrap();
    runtime.build_frame().unwrap();
    let scroll = runtime
        .tree()
        .traverse()
        .into_iter()
        .find(|node| matches!(runtime.tree().get(*node).unwrap().kind, NodeKind::Scroll(_)))
        .unwrap();
    let mut all = Vec::new();
    let mut events = Vec::new();
    let mut quiet = Vec::new();
    let mut nodes = 0;
    let mut previous = runtime.tree().scene_version();
    let mut offset = 0.0f32;
    for _ in 0..600 {
        offset += 70.0;
        runtime
            .tree_mut()
            .scroll_to(scroll, aqua_core::Point::new(0.0, offset));
        let started = Instant::now();
        runtime.build_frame().unwrap();
        let micros = started.elapsed().as_secs_f64() * 1e6;
        let version = runtime.tree().scene_version();
        all.push(micros);
        match version != previous {
            true => events.push(micros),
            false => quiet.push(micros),
        }
        previous = version;
        nodes = nodes.max(runtime.tree().traverse().len());
    }
    let total: f64 = all.iter().sum();
    let event_total: f64 = events.iter().sum();
    let mut sorted = all.clone();
    sorted.sort_by(f64::total_cmp);
    quiet.sort_by(f64::total_cmp);
    events.sort_by(f64::total_cmp);
    println!(
        "scroll-build steps=600 total_ms={:.1} mean={:.0}us p50={:.0} p99={:.0} events={} event_mean={:.0}us event_p50={:.0} quiet_mean={:.0}us event_share={:.0}% max_nodes={nodes}",
        total / 1000.0,
        total / 600.0,
        percentile(&sorted, 0.5),
        percentile(&sorted, 0.99),
        events.len(),
        event_total / events.len().max(1) as f64,
        events.get(events.len() / 2).copied().unwrap_or(0.0),
        quiet.iter().sum::<f64>() / quiet.len().max(1) as f64,
        100.0 * event_total / total,
    );
}
