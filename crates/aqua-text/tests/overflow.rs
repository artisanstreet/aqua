use aqua_text::overflow::{single_line_overflow, EllipsisPosition, OverflowCluster, OverflowMode};

fn clusters(text: &str, advance: f32) -> Vec<OverflowCluster> {
    text.char_indices()
        .map(|(start, ch)| OverflowCluster {
            bytes: start..start + ch.len_utf8(),
            advance,
        })
        .collect()
}

#[test]
fn end_and_start_keep_source_boundaries() {
    let text = "abcde";
    let shaped = clusters(text, 10.0);
    let end = single_line_overflow(text, &shaped, 35.0, 5.0, OverflowMode::EndEllipsis);
    assert_eq!(end.source, 0..2);
    assert_eq!(end.ellipsis, Some(EllipsisPosition::End));
    let start = single_line_overflow(text, &shaped, 35.0, 5.0, OverflowMode::StartEllipsis);
    assert_eq!(start.source, 3..5);
    assert_eq!(start.ellipsis, Some(EllipsisPosition::Start));
}

#[test]
fn end_trims_punctuation_without_splitting_clusters() {
    let text = "ab, cd";
    let shaped = vec![
        OverflowCluster {
            bytes: 0..2,
            advance: 20.0,
        },
        OverflowCluster {
            bytes: 2..3,
            advance: 10.0,
        },
        OverflowCluster {
            bytes: 3..4,
            advance: 10.0,
        },
        OverflowCluster {
            bytes: 4..6,
            advance: 20.0,
        },
    ];
    let result = single_line_overflow(text, &shaped, 45.0, 5.0, OverflowMode::EndEllipsis);
    assert_eq!(&text[result.source], "ab");
}

#[test]
fn fits_and_clip_leave_original_intact() {
    let text = "éclair";
    let shaped = clusters(text, 4.0);
    for mode in [
        OverflowMode::Clip,
        OverflowMode::StartEllipsis,
        OverflowMode::EndEllipsis,
    ] {
        let width = if mode == OverflowMode::Clip {
            1.0
        } else {
            24.0
        };
        let result = single_line_overflow(text, &shaped, width, 4.0, mode);
        assert_eq!(result.source, 0..text.len());
        assert_eq!(result.ellipsis, None);
    }
}

#[cfg(windows)]
#[test]
fn shaped_overflow_keeps_full_source_and_maps_visible_glyphs() {
    use aqua_text::style::LineMode;
    use aqua_text::{FontManager, TextLayout, TextStyle};
    let text = "A long filename ending.txt";
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_line_mode(LineMode::Single);
    layout.set_text(text, &TextStyle::default());
    let natural = layout.layout(&mut fonts);
    let width = natural.size.0 * 0.6;
    let visible =
        layout.layout_single_line_overflow(&mut fonts, width, OverflowMode::StartEllipsis);
    assert_eq!(layout.source_len(), text.len());
    let glyphs = &visible.lines[0].glyphs;
    assert!(glyphs.iter().any(|glyph| glyph.src_byte_start > 0));
    assert!(glyphs.iter().all(|glyph| glyph.src_byte_end <= text.len()));
    assert!(visible.size.0 < natural.size.0);
}

#[test]
fn source_character_spacing_changes_the_cut_boundary() {
    use aqua_text::overflow::single_line_overflow_with_spacing;
    let text = "abcdef";
    let advances = clusters(text, 10.0);
    let without = single_line_overflow_with_spacing(
        text,
        &advances,
        37.0,
        5.0,
        0.0,
        OverflowMode::EndEllipsis,
    );
    let spaced = single_line_overflow_with_spacing(
        text,
        &advances,
        37.0,
        5.0,
        3.0,
        OverflowMode::EndEllipsis,
    );
    assert_eq!(without.source, 0..3);
    assert_eq!(spaced.source, 0..2);
}

#[test]
fn shaped_text_that_fits_is_not_truncated() {
    use aqua_text::{FontManager, TextLayout, TextStyle};
    let mut fonts = FontManager::new();
    let mut layout = TextLayout::new();
    layout.set_line_mode(aqua_text::LineMode::Single);
    layout.set_text("AVATAR", &TextStyle::default());
    let natural = layout.layout(&mut fonts);
    let width = natural.size.0;
    let result = layout.layout_single_line_overflow(&mut fonts, width, OverflowMode::EndEllipsis);
    assert_eq!(layout.display_text(), "AVATAR");
    assert_eq!(result.size.0, width);
}
