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
}
