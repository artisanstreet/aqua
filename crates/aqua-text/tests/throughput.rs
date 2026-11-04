use std::time::Instant;

use aqua_text::{FontManager, LineMode, TextLayout, TextStyle, WrapMode};

const LINE: &str = "fn layout(&mut self, fonts: &mut FontManager) -> Arc<LaidOut> { let value = 42; // shaped text }";

#[test]
#[ignore]
fn shaping_and_wrapping_throughput() {
    let mut fonts = FontManager::new();
    let text = (0..2000)
        .map(|i| format!("{i}: {LINE}"))
        .collect::<Vec<_>>()
        .join("\n");
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::WordOrGlyph);
    layout.set_text(&text, &TextStyle::default());
    layout.set_max_width(Some(600.0));
    let start = Instant::now();
    let first = layout.layout(&mut fonts);
    let cold = start.elapsed();
    layout.set_max_width(Some(500.0));
    let start = Instant::now();
    let second = layout.layout(&mut fonts);
    let rewrap = start.elapsed();
    println!(
        "throughput paragraphs=2000 cold_ms={:.1} rewrap_ms={:.1} lines_first={} lines_second={} reshapes={}",
        cold.as_secs_f64() * 1e3,
        rewrap.as_secs_f64() * 1e3,
        first.lines.len(),
        second.lines.len(),
        layout.reshape_count()
    );
}

#[test]
#[ignore]
fn startup_and_first_layout_cost() {
    let start = Instant::now();
    let mut fonts = FontManager::new();
    let discovery = start.elapsed();
    let mut layout = TextLayout::new();
    layout.set_text("Hello, Aqua", &TextStyle::default());
    let start = Instant::now();
    layout.layout(&mut fonts);
    let first = start.elapsed();
    let mut again = TextLayout::new();
    again.set_text("Hello, Aqua again", &TextStyle::default());
    let start = Instant::now();
    again.layout(&mut fonts);
    let second = start.elapsed();
    println!(
        "startup faces={} discovery_ms={:.1} first_layout_ms={:.2} second_layout_ms={:.3}",
        fonts.face_count(),
        discovery.as_secs_f64() * 1e3,
        first.as_secs_f64() * 1e3,
        second.as_secs_f64() * 1e3
    );
}
