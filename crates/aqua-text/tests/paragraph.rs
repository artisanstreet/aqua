use aqua_text::{FontManager, LineMode, TextAlign, TextLayout, TextStyle, WrapMode};

fn layout(
    text: &str,
    width: Option<f32>,
    align: TextAlign,
) -> (TextLayout, std::sync::Arc<aqua_text::LaidOut>) {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_wrap(WrapMode::Word);
    layout.set_align(align);
    layout.set_max_width(width);
    layout.set_text(text, &TextStyle::default());
    let laid = layout.layout(&mut fonts);
    (layout, laid)
}

#[cfg(windows)]
#[test]
fn hebrew_run_inside_latin_paragraph_is_reordered_visually() {
    let text = "abc \u{5d0}\u{5d1}\u{5d2} def";
    let (_, laid) = layout(text, None, TextAlign::Start);
    let glyphs = &laid.lines[0].glyphs;
    let x_of = |byte: usize| {
        glyphs
            .iter()
            .find(|glyph| glyph.display_byte_start == byte)
            .map(|glyph| glyph.x)
            .unwrap()
    };
    let first = text.find('\u{5d0}').unwrap();
    let last = text.find('\u{5d2}').unwrap();
    assert!(x_of(last) < x_of(first));
    assert!(x_of(0) < x_of(last));
    assert!(x_of(first) < x_of(text.find("def").unwrap()));
    assert!(glyphs.iter().any(|glyph| glyph.bidi_level % 2 == 1));
}

#[cfg(windows)]
#[test]
fn alignment_moves_lines_inside_the_constraint() {
    let text = "alpha beta\ngamma";
    let (_, start) = layout(text, Some(300.0), TextAlign::Start);
    let (_, center) = layout(text, Some(300.0), TextAlign::Center);
    let (_, right) = layout(text, Some(300.0), TextAlign::Right);
    let first_x = |laid: &aqua_text::LaidOut, line: usize| laid.lines[line].glyphs[0].x;
    assert_eq!(first_x(&start, 0), 0.0);
    for line in 0..2 {
        let width = start.lines[line].width;
        assert!((first_x(&center, line) - (300.0 - width) / 2.0).abs() < 0.01);
        assert!((first_x(&right, line) - (300.0 - width)).abs() < 0.01);
    }
}

#[cfg(windows)]
#[test]
fn justify_fills_wrapped_lines_but_not_the_last() {
    let text = "alpha beta gamma delta epsilon zeta eta theta iota kappa";
    let (_, laid) = layout(text, Some(160.0), TextAlign::Justify);
    assert!(laid.lines.len() > 2);
    for line in &laid.lines[..laid.lines.len() - 1] {
        let right = line.glyphs.iter().map(|g| g.x + g.w).fold(0.0, f32::max);
        let trailing: f32 = line
            .glyphs
            .iter()
            .rev()
            .take_while(|g| {
                text[g.display_byte_start..g.display_byte_end]
                    .trim()
                    .is_empty()
            })
            .map(|g| g.w)
            .sum();
        assert!(
            (right - trailing - 160.0).abs() < 1.0,
            "line right edge {right} trailing {trailing}"
        );
    }
    let last = laid.lines.last().unwrap();
    assert!(last.width < 160.0);
}

#[cfg(windows)]
#[test]
fn tabs_land_on_multiples_of_eight_spaces() {
    let (_, laid) = layout("a\tb\n\tc", None, TextAlign::Start);
    let tab_stop = {
        let (_, spaces) = layout("        ", None, TextAlign::Start);
        spaces.lines[0].glyphs.iter().map(|g| g.w).sum::<f32>()
    };
    let b = laid.lines[0]
        .glyphs
        .iter()
        .find(|g| g.display_byte_start == 2)
        .unwrap();
    assert!((b.x - tab_stop).abs() < 0.01);
    let c = laid.lines[1]
        .glyphs
        .iter()
        .find(|g| g.display_byte_start == 5)
        .unwrap();
    assert!((c.x - tab_stop).abs() < 0.01);
}

#[cfg(windows)]
#[test]
fn changing_only_the_width_does_not_reshape() {
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Multi);
    layout.set_text("alpha beta gamma delta epsilon zeta", &TextStyle::default());
    layout.set_max_width(Some(400.0));
    let wide = layout.layout(&mut fonts);
    assert_eq!(layout.reshape_count(), 1);
    layout.set_max_width(Some(90.0));
    let narrow = layout.layout(&mut fonts);
    assert_eq!(layout.reshape_count(), 1);
    assert!(narrow.lines.len() > wide.lines.len());
    layout.set_align(TextAlign::Right);
    layout.layout(&mut fonts);
    assert_eq!(layout.reshape_count(), 1);
    layout.set_scale(2.0);
    layout.layout(&mut fonts);
    assert_eq!(layout.reshape_count(), 2);
}

#[test]
fn trailing_newline_yields_an_empty_final_line_with_line_height() {
    let (_, laid) = layout("one\n", None, TextAlign::Start);
    assert_eq!(laid.lines.len(), 2);
    assert!(laid.lines[1].glyphs.is_empty());
    assert!(laid.lines[1].height > 0.0);
    assert!(laid.lines[1].y_top >= laid.lines[0].y_top + laid.lines[0].height - 0.001);
}
