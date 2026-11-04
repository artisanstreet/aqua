#![cfg(windows)]

use aqua::render::lower::Lowering;
use aqua::ui::Text;
use aqua::UiApp;
use aqua_core::scene::TextRasterMode;
use aqua_core::Size;
use aqua_text::{SharedFonts, SharedRuns};
use aqua_transport::LowerContext;

const SIZE: Size = Size { w: 640.0, h: 480.0 };
const WORDS: [&str; 20] = [
    "render",
    "thread",
    "atlas",
    "growth",
    "streaming",
    "Markdown",
    "tokens",
    "arrive",
    "shape",
    "cache",
    "Because",
    "Zebra",
    "quick",
    "jumped",
    "(code)",
    "key=value;",
    "0123456789",
    "Fox?",
    "end.",
    "and",
];

fn paragraph(seed: usize) -> String {
    let letter = (b'a' + (seed % 26) as u8) as char;
    let fresh = match seed % 2 {
        0 => letter.to_string().repeat(3),
        _ => letter.to_ascii_uppercase().to_string().repeat(2) + &seed.to_string(),
    };
    let mut words: Vec<&str> = (0..24)
        .map(|n| WORDS[(seed * 5 + n * 7 + n * n) % WORDS.len()])
        .collect();
    words.push(&fresh);
    words.join(" ")
}

fn window(first: usize) -> String {
    (first..first + 10)
        .map(paragraph)
        .collect::<Vec<_>>()
        .join("\n")
}

fn context() -> LowerContext {
    LowerContext {
        size: SIZE,
        scale: 1.0,
        now: aqua_protocol::Instant::ZERO,
    }
}

#[test]
fn scrolling_through_ascii_text_never_grows_the_slug_store_or_rasterizes() {
    let fonts = SharedFonts::new();
    let runs = SharedRuns::new();
    let mut lowering = Lowering::new(fonts.clone(), runs.clone());
    lowering.set_text_raster(TextRasterMode::Outline);
    let mut grew = Vec::new();
    for first in 0..60 {
        let mut app = UiApp::new(fonts.clone(), runs.clone());
        let text = window(first);
        app.mount(move |_| {
            Text::new(text.clone())
                .font_size(14.0)
                .multiline(true)
                .wrap(true)
                .build()
        })
        .unwrap();
        app.set_viewport(SIZE.w, SIZE.h, 1.0);
        app.build_frame().unwrap();
        aqua_transport::Lowering::lower(&mut lowering, app.scene(), &context());
        let report = lowering.last_report().clone();
        assert!(report.glyphs > 0);
        if first > 0
            && (report.slug_created > 0
                || report.atlas_created > 0
                || report.glyph_rasterized > 0
                || report.slug_upload_bytes > 0)
        {
            grew.push((first, report));
        }
    }
    assert!(grew.is_empty(), "{grew:?}");
}
