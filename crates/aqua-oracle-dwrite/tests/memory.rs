#![cfg(windows)]

use aqua_media::residency::{Residency, ResidencyConfig};
use aqua_text::raster::place_glyph;
use aqua_text::residency::GlyphResolver;
use aqua_text::{FontFamily, FontManager, LineMode, TextLayout, TextStyle, WrapMode};
use windows::Win32::System::ProcessStatus::{GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;

fn private_bytes() -> usize {
    let mut counters = PROCESS_MEMORY_COUNTERS_EX::default();
    unsafe {
        GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut counters as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
        )
        .unwrap();
    }
    counters.PrivateUsage
}

#[test]
fn text_stack_private_memory_stays_inside_its_budget() {
    let baseline = private_bytes();
    let mut fonts = FontManager::new();
    let after_discovery = private_bytes();
    let style = TextStyle {
        families: vec![FontFamily::Named("Segoe UI".to_owned())],
        size_px: 14.0,
        line_height_px: 20.0,
        ..TextStyle::default()
    };
    let text: String = (0..400)
        .map(|index| format!("{index:04} let value = shape(\"Aqua text\", width); // \u{4f60}\u{597d} \u{1f600} \u{5d0}\u{5d1}\n"))
        .collect();
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::WordOrGlyph);
    layout.set_max_width(Some(700.0));
    layout.set_text(&text, &style);
    let laid = layout.layout(&mut fonts);
    let after_layout = private_bytes();
    let mut residency = Residency::new(ResidencyConfig::default());
    let mut resolver = GlyphResolver::new();
    residency.begin_frame();
    for line in &laid.lines {
        for glyph in &line.glyphs {
            let placement = place_glyph(glyph, 0.0, line.baseline_y, true);
            let _ = resolver.resolve(&mut residency, &mut fonts, &placement.key);
        }
    }
    let uploaded = residency.finish_frame().upload_bytes() as usize;
    let after_raster = private_bytes();
    println!(
        "memory discovery={:.2}MB layout={:.2}MB raster={:.2}MB atlas_uploads={:.2}MB glyphs={} lines={} resident={}",
        (after_discovery - baseline) as f64 / 1e6,
        (after_layout - after_discovery) as f64 / 1e6,
        (after_raster - after_layout) as f64 / 1e6,
        uploaded as f64 / 1e6,
        laid.lines
            .iter()
            .map(|line| line.glyphs.len())
            .sum::<usize>(),
        laid.lines.len(),
        residency.stats().masks.entries + residency.stats().colors.entries,
    );
    assert!(after_raster - baseline < 20 * 1024 * 1024);
}
