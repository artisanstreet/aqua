use std::time::Instant;

use aqua_core::Size;
use aqua_runtime::Runtime;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_ui::element::ui::{Box as UiBox, Text as UiText};
use aqua_ui::style::{px, Axis, Style};

mod support;

const PARAGRAPH: &str = "Aqua keeps retained text, layout and scene state between frames so that an unchanged transcript costs nothing, while a streaming message only reshapes the run that changed.";

fn mount_transcript(runtime: &mut Runtime, rows: usize) {
    runtime
        .mount(|_| {
            UiBox::new()
                .style(Style::new().axis(Axis::Column).width(px(760.0)))
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
                .build()
        })
        .unwrap();
}

#[test]
#[ignore]
fn render_full_report() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    for rows in [100, 1000] {
        let mut runtime = Runtime::new(fonts.clone(), runs.clone());
        runtime.set_viewport(1200.0, 800.0, 1.0);
        mount_transcript(&mut runtime, rows);
        runtime.build_frame().unwrap();
        for mut painter in support::hardware(fonts.clone(), runs.clone()) {
            let mut micros = Vec::new();
            for _ in 0..10 {
                let started = Instant::now();
                painter.paint(runtime.scene(), Size::new(1200.0, 800.0), 1.0);
                micros.push(started.elapsed().as_secs_f64() * 1e6);
            }
            micros.sort_by(f64::total_cmp);
            println!(
                "perf-render {} rows={rows} p50_us={:.0} max_us={:.0} commands={} glyphs={}",
                painter.label,
                micros[micros.len() / 2],
                micros[micros.len() - 1],
                painter.report.commands,
                painter.report.glyphs,
            );
        }
    }
}
