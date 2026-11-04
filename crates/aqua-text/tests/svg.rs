use aqua_text::svg::outline_text;
use aqua_text::FontManager;

#[test]
fn documents_without_text_pass_through_untouched() {
    let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="8" height="8"><rect width="8" height="8"/></svg>"#;
    let mut fonts = FontManager::new_without_system_fonts();
    assert!(matches!(
        outline_text(svg, &mut fonts),
        std::borrow::Cow::Borrowed(_)
    ));
}

#[cfg(windows)]
mod with_fonts {
    use aqua_media::decode::{decode_svg, DecodedImage};
    use aqua_text::svg::outline_text;
    use aqua_text::FontManager;

    fn render(svg: &str, fonts: &mut FontManager) -> DecodedImage {
        let outlined = outline_text(svg.as_bytes(), fonts);
        decode_svg(&outlined, 64 * 1024 * 1024).expect("decode")
    }

    fn ink_columns(image: &DecodedImage) -> (u32, u32, usize) {
        let width = image.width() as usize;
        let inked: Vec<usize> = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, px)| px[3] > 32)
            .map(|(index, _)| index % width)
            .collect();
        (
            inked.iter().copied().min().unwrap_or(0) as u32,
            inked.iter().copied().max().unwrap_or(0) as u32,
            inked.len(),
        )
    }

    const START: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40"><text x="100" y="28" font-family="Arial" font-size="24" fill="black">Hello</text></svg>"#;
    const MIDDLE: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40"><text x="100" y="28" font-family="Arial" font-size="24" text-anchor="middle" fill="black">Hello</text></svg>"#;
    const END: &str = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40"><text x="100" y="28" font-family="Arial" font-size="24" text-anchor="end" fill="black">Hello</text></svg>"#;

    #[test]
    fn text_becomes_outlines_that_resvg_paints() {
        let mut fonts = FontManager::new();
        let outlined = outline_text(START.as_bytes(), &mut fonts);
        let outlined = std::str::from_utf8(&outlined).unwrap();
        assert!(!outlined.contains("<text") && outlined.contains("<path"));
        assert!(ink_columns(&render(START, &mut fonts)).2 > 100);
    }

    #[test]
    fn anchors_move_the_ink_around_the_anchor_point() {
        let mut fonts = FontManager::new();
        let (start_min, start_max, _) = ink_columns(&render(START, &mut fonts));
        let (mid_min, mid_max, _) = ink_columns(&render(MIDDLE, &mut fonts));
        let (end_min, end_max, _) = ink_columns(&render(END, &mut fonts));
        let width = start_max - start_min;
        assert!((195..=215).contains(&start_min));
        assert!((380..=420).contains(&(mid_min + mid_max)));
        assert!((185..=215).contains(&end_max));
        assert!(mid_max - mid_min + 4 >= width && end_max - end_min + 4 >= width);
    }

    #[test]
    fn tspans_take_their_own_fill_and_position() {
        let svg = r#"<svg xmlns="http://www.w3.org/2000/svg" width="200" height="40"><text y="28" font-family="Arial" font-size="24"><tspan x="10" fill="red">AB</tspan><tspan x="110" fill="blue">CD</tspan></text></svg>"#;
        let mut fonts = FontManager::new();
        let outlined = outline_text(svg.as_bytes(), &mut fonts);
        let outlined = std::str::from_utf8(&outlined).unwrap();
        assert_eq!(outlined.matches("<path").count(), 2);
        assert!(outlined.contains("fill=\"red\"") && outlined.contains("fill=\"blue\""));
    }

    fn ink_rows(image: &DecodedImage) -> (u32, u32) {
        let width = image.width() as usize;
        let rows: Vec<usize> = image
            .pixels()
            .as_chunks::<4>()
            .0
            .iter()
            .enumerate()
            .filter(|(_, px)| px[3] > 32)
            .map(|(index, _)| index / width)
            .collect();
        (
            rows.iter().copied().min().unwrap_or(0) as u32,
            rows.iter().copied().max().unwrap_or(0) as u32,
        )
    }

    fn doc(body: &str) -> String {
        format!(
            r#"<svg xmlns="http://www.w3.org/2000/svg" xmlns:xlink="http://www.w3.org/1999/xlink" width="200" height="120">{body}</svg>"#
        )
    }

    #[test]
    fn style_sheets_set_size_and_paint() {
        let sheet = doc(
            r##"<style>text.big { font-size: 40px; fill: #ff0000 } text { font-family: Arial }</style><text class="big" x="10" y="60">Hi</text>"##,
        );
        let direct = doc(
            r##"<text x="10" y="60" font-family="Arial" font-size="40" fill="#ff0000">Hi</text>"##,
        );
        let mut fonts = FontManager::new();
        let outlined = outline_text(sheet.as_bytes(), &mut fonts);
        assert!(std::str::from_utf8(&outlined).unwrap().contains("#ff0000"));
        assert_eq!(
            ink_columns(&render(&sheet, &mut fonts)),
            ink_columns(&render(&direct, &mut fonts))
        );
    }

    #[test]
    fn em_and_percent_font_sizes_resolve_through_ancestors() {
        let em = doc(r#"<g font-size="20"><text x="10" y="60" font-family="Arial" font-size="2em">Hi</text></g>"#);
        let percent = doc(r#"<g font-size="20"><text x="10" y="60" font-family="Arial" font-size="200%">Hi</text></g>"#);
        let pixels = doc(r#"<text x="10" y="60" font-family="Arial" font-size="40">Hi</text>"#);
        let mut fonts = FontManager::new();
        let expected = ink_columns(&render(&pixels, &mut fonts));
        assert!(expected.2 > 100);
        assert_eq!(ink_columns(&render(&em, &mut fonts)), expected);
        assert_eq!(ink_columns(&render(&percent, &mut fonts)), expected);
    }

    #[test]
    fn text_on_a_straight_path_matches_plain_text() {
        let plain = doc(r#"<text x="10" y="60" font-family="Arial" font-size="24">Hello</text>"#);
        let along = doc(
            r##"<defs><path id="p" d="M10 60 L190 60"/></defs><text font-family="Arial" font-size="24"><textPath xlink:href="#p">Hello</textPath></text>"##,
        );
        let mut fonts = FontManager::new();
        let expected = ink_columns(&render(&plain, &mut fonts));
        let got = ink_columns(&render(&along, &mut fonts));
        assert!(expected.2 > 100);
        assert!(got.0.abs_diff(expected.0) <= 2 && got.1.abs_diff(expected.1) <= 4);
        assert!(got.2.abs_diff(expected.2) * 20 < expected.2);
    }

    #[test]
    fn text_on_a_curve_rotates_and_start_offset_with_anchor_shift_it() {
        let flat = doc(r#"<text x="10" y="100" font-family="Arial" font-size="24">Hello World</text>"#);
        let arc = doc(
            r##"<path id="p" d="M10 100 A 90 90 0 0 1 190 100" fill="none"/><text font-family="Arial" font-size="24"><textPath href="#p">Hello World</textPath></text>"##,
        );
        let mut fonts = FontManager::new();
        let (flat_top, flat_bottom) = ink_rows(&render(&flat, &mut fonts));
        let (arc_top, arc_bottom) = ink_rows(&render(&arc, &mut fonts));
        assert!(arc_bottom - arc_top > (flat_bottom - flat_top) * 2);
        let centered = doc(
            r##"<path id="p" d="M10 100 L190 100"/><text font-family="Arial" font-size="24" text-anchor="middle"><textPath href="#p" startOffset="50%">Hello</textPath></text>"##,
        );
        let (min, max, _) = ink_columns(&render(&centered, &mut fonts));
        assert!((190..=210).contains(&((min + max) / 2)));
    }
}
