fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(not(target_os = "linux"))]
    eprintln!("system_fonts: skipped; fontconfig discovery is Linux only");
}

#[cfg(target_os = "linux")]
mod linux {
    use aqua_fonts_system::testing::{FixtureFont, FontconfigFixture};
    use aqua_fonts_system::Generic;
    use aqua_text::{FontFamily, FontManager, FontWeight, TextLayout, TextStyle};

    const HIRAGANA: char = 'あ';
    const EMOJI: char = '😀';

    fn fixture() -> FontconfigFixture {
        let fixture = FontconfigFixture::new("aqua-text")
            .expect("a scratch directory")
            .prefer(
                Generic::SansSerif,
                &["Fixture Sans", "Fixture CJK B", "Fixture CJK A"],
            )
            .prefer(Generic::Monospace, &["Fixture Mono"])
            .prefer(Generic::Emoji, &["Fixture Emoji"]);
        let fonts = [
            ("sans.ttf", FixtureFont::new("Fixture Sans").ascii()),
            (
                "sans-bold.ttf",
                FixtureFont::new("Fixture Sans").weight(700).ascii(),
            ),
            ("inter.ttf", FixtureFont::new("Inter").ascii()),
            ("mono/mono.ttf", FixtureFont::new("Fixture Mono").ascii()),
            (
                "cjk/a.ttf",
                FixtureFont::new("Fixture CJK A").covering([HIRAGANA, EMOJI]),
            ),
            (
                "cjk/b.ttf",
                FixtureFont::new("Fixture CJK B").covering([HIRAGANA, EMOJI]),
            ),
            (
                "emoji/emoji.ttf",
                FixtureFont::new("Fixture Emoji").covering([EMOJI]),
            ),
        ];
        for (path, font) in &fonts {
            fixture
                .install(path, font)
                .expect("a fixture font is written");
        }
        fixture
    }

    fn faces(fonts: &mut FontManager, text: &str, style: &TextStyle) -> Vec<(char, String)> {
        let mut layout = TextLayout::new();
        layout.set_text(text, style);
        let laid = layout.layout(fonts);
        laid.lines
            .iter()
            .flat_map(|line| line.glyphs.iter())
            .map(|glyph| {
                let character = text[glyph.src_byte_start..]
                    .chars()
                    .next()
                    .expect("a glyph maps to source text");
                let face = fonts
                    .postscript_name(glyph.font_id)
                    .expect("a laid glyph has a face")
                    .to_owned();
                (character, face)
            })
            .collect()
    }

    fn face_of(fonts: &mut FontManager, text: &str, style: &TextStyle, wanted: char) -> String {
        faces(fonts, text, style)
            .into_iter()
            .find(|(character, _)| *character == wanted)
            .map(|(_, face)| face)
            .unwrap_or_else(|| panic!("{wanted:?} was laid out in {text:?}"))
    }

    fn sans_serif_text_uses_the_configured_family_rather_than_a_built_in_favourite(
        fonts: &mut FontManager,
    ) {
        let laid = faces(fonts, "Hello", &TextStyle::default());
        assert_eq!(laid.len(), 5);
        assert!(
            laid.iter().all(|(_, face)| face == "FixtureSans-Regular"),
            "sans-serif follows the fontconfig alias, not the built-in Inter: {laid:?}"
        );
    }

    fn bold_sans_serif_text_uses_the_configured_familys_bold_face(fonts: &mut FontManager) {
        let style = TextStyle {
            weight: FontWeight(700),
            ..TextStyle::default()
        };
        assert_eq!(face_of(fonts, "Bold", &style, 'B'), "FixtureSans-Bold");
    }

    fn monospace_text_uses_the_configured_monospace_family(fonts: &mut FontManager) {
        let style = TextStyle {
            families: vec![FontFamily::Monospace],
            ..TextStyle::default()
        };
        assert_eq!(face_of(fonts, "code", &style, 'c'), "FixtureMono-Regular");
    }

    fn a_character_the_primary_lacks_falls_back_in_fontconfig_order(fonts: &mut FontManager) {
        let text = format!("a{HIRAGANA}");
        let style = TextStyle::default();
        assert_eq!(face_of(fonts, &text, &style, 'a'), "FixtureSans-Regular");
        assert_eq!(
            face_of(fonts, &text, &style, HIRAGANA),
            "FixtureCJKB-Regular",
            "fontconfig prefers CJK B over CJK A for sans-serif"
        );
    }

    fn an_emoji_falls_back_to_the_configured_emoji_family(fonts: &mut FontManager) {
        let text = format!("a{EMOJI}");
        assert_eq!(
            face_of(fonts, &text, &TextStyle::default(), EMOJI),
            "FixtureEmoji-Regular",
            "the emoji alias wins over sans-serif fallbacks that also cover the emoji"
        );
    }

    type Case = (&'static str, fn(&mut FontManager));

    pub fn main() {
        let fixture = fixture();
        let config = fixture
            .write_config()
            .expect("the fontconfig file is written");
        unsafe {
            std::env::set_var("FONTCONFIG_FILE", &config);
            std::env::remove_var("FONTCONFIG_PATH");
            std::env::remove_var("FONTCONFIG_SYSROOT");
            std::env::remove_var("AQUA_FONT_DIRS");
        }
        let mut fonts = FontManager::new();
        let cases: [Case; 5] = [
            (
                "sans_serif_text_uses_the_configured_family_rather_than_a_built_in_favourite",
                sans_serif_text_uses_the_configured_family_rather_than_a_built_in_favourite,
            ),
            (
                "bold_sans_serif_text_uses_the_configured_familys_bold_face",
                bold_sans_serif_text_uses_the_configured_familys_bold_face,
            ),
            (
                "monospace_text_uses_the_configured_monospace_family",
                monospace_text_uses_the_configured_monospace_family,
            ),
            (
                "a_character_the_primary_lacks_falls_back_in_fontconfig_order",
                a_character_the_primary_lacks_falls_back_in_fontconfig_order,
            ),
            (
                "an_emoji_falls_back_to_the_configured_emoji_family",
                an_emoji_falls_back_to_the_configured_emoji_family,
            ),
        ];
        let filter = std::env::args().skip(1).find(|arg| !arg.starts_with('-'));
        for (name, case) in cases {
            if filter
                .as_deref()
                .is_some_and(|wanted| !name.contains(wanted))
            {
                continue;
            }
            case(&mut fonts);
            println!("test {name} ... ok");
        }
    }
}
