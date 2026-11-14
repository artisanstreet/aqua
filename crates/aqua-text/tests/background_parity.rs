#![cfg(windows)]

use aqua_text::{FontManager, LaidOut, StyledSpan, TextBackground, TextLayout, WrapMode};

fn source_backgrounds(shaped: &LaidOut, spans: &[StyledSpan]) -> Vec<TextBackground> {
    let mut runs = spans.iter();
    let mut run_end = 0;
    let mut active = None;
    let mut result = Vec::new();
    for (index, line) in shaped.lines.iter().enumerate() {
        for glyph in &line.glyphs {
            if glyph.src_byte_start >= run_end {
                let mut style = runs.next();
                while let Some(run) = style {
                    if glyph.src_byte_start < run_end + run.text.len() {
                        break;
                    }
                    run_end += run.text.len();
                    style = runs.next();
                }
                let mut finished = None;
                if let Some(run) = style {
                    if active
                        .as_ref()
                        .is_some_and(|(_, color)| Some(*color) != run.background)
                    {
                        finished = active.take();
                    }
                    if let Some(color) = run.background {
                        active.get_or_insert((glyph.x, color));
                    }
                    run_end += run.text.len();
                } else {
                    finished = active.take();
                }
                if let Some((mut x, fill)) = finished {
                    let width = glyph.x - x;
                    if width == 0.0 {
                        x -= glyph.font_max_width.unwrap() / 2.0;
                    }
                    result.push(TextBackground {
                        bounds: aqua_text::Rect {
                            x,
                            y: line.y_top,
                            w: width,
                            h: line.height,
                        },
                        fill,
                    });
                }
            }
        }
        if let Some((mut x, fill)) = active.take() {
            let next = shaped
                .lines
                .get(index + 1)
                .filter(|next| next.paragraph == line.paragraph)
                .and_then(|next| next.glyphs.first());
            let mut width = line.width - x;
            if width == 0.0 {
                let glyph = next.or_else(|| line.glyphs.last()).unwrap();
                let half = glyph.font_max_width.unwrap() / 2.0;
                x -= half;
                width = half;
            }
            result.push(TextBackground {
                bounds: aqua_text::Rect {
                    x,
                    y: line.y_top,
                    w: width,
                    h: line.height,
                },
                fill,
            });
            if next.is_some_and(|glyph| glyph.src_byte_start < run_end) {
                active = Some((0.0, fill));
            }
        }
    }
    result
}

#[test]
fn backgrounds_follow_pinned_reference_painter_for_clusters_bidi_and_wraps() {
    let red = [0.2, 0.3, 0.4, 0.5];
    let blue = [0.6, 0.7, 0.8, 0.9];
    let fixtures = [
        vec![
            StyledSpan::plain("a"),
            StyledSpan::plain("\u{301}").gpui_background(red),
            StyledSpan::plain("bc").gpui_background(blue),
        ],
        vec![
            StyledSpan::plain("אב").gpui_background(red),
            StyledSpan::plain("גד").gpui_background(blue),
        ],
        vec![
            StyledSpan::plain("first "),
            StyledSpan::plain("a long colored run crossing wrapped lines").gpui_background(red),
            StyledSpan::plain(" end").gpui_background(blue),
        ],
    ];
    let mut fonts = FontManager::new();
    for spans in fixtures {
        let mut layout = TextLayout::new();
        layout.set_spans(spans.clone());
        layout.set_wrap(WrapMode::WordOrGlyph);
        layout.set_max_width(Some(70.0));
        let shaped = layout.layout(&mut fonts);
        assert!(shaped.lines.iter().any(|line| !line.glyphs.is_empty()));
        assert_eq!(
            layout.gpui_backgrounds().unwrap(),
            source_backgrounds(&shaped, &spans)
        );
    }
}
