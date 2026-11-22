fn main() {
    #[cfg(target_os = "linux")]
    linux::main();
    #[cfg(target_os = "macos")]
    mac::main();
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    eprintln!("system_fonts: skipped; fontconfig and Core Text discovery are Linux and macOS only");
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

#[cfg(target_os = "macos")]
mod mac {
    use std::time::Instant;

    use aqua_fonts_system::testing::core_text::{self, ChosenFace};
    use aqua_fonts_system::{FontDatabase, Generic};
    use aqua_text::raster::{place_glyph, GlyphImage, Rasterizer};
    use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

    const HIRAGANA: char = 'あ';
    const EMOJI: char = '😀';

    struct System {
        fonts: FontManager,
        database: FontDatabase,
    }

    impl System {
        fn chosen_family(&self, chosen: &ChosenFace) -> String {
            chosen
                .resolve(&self.database)
                .unwrap_or_else(|| panic!("{chosen:?} is a face Aqua loaded"))
                .families[0]
                .clone()
        }

        fn family_of(&mut self, text: &str, style: &TextStyle, wanted: char) -> String {
            let mut layout = TextLayout::new();
            layout.set_text(text, style);
            let laid = layout.layout(&mut self.fonts);
            let glyph = laid
                .lines
                .iter()
                .flat_map(|line| line.glyphs.iter())
                .find(|glyph| text[glyph.src_byte_start..].starts_with(wanted))
                .unwrap_or_else(|| panic!("{wanted:?} was laid out in {text:?}"));
            let post_script_name = self
                .fonts
                .postscript_name(glyph.font_id)
                .expect("a laid glyph has a face")
                .to_owned();
            self.database
                .faces()
                .find(|face| face.post_script_name == post_script_name)
                .unwrap_or_else(|| panic!("{post_script_name} is in the system database"))
                .families[0]
                .clone()
        }
    }

    fn monospace() -> TextStyle {
        TextStyle {
            families: vec![FontFamily::Monospace],
            ..TextStyle::default()
        }
    }

    fn configured_sans_serif_is_the_system_ui_font(system: &mut System) {
        let interface = core_text::system_ui_font().expect("Core Text names a system UI font");
        let family = system.chosen_family(&interface);
        assert_eq!(
            system
                .database
                .configured_families(Generic::SansSerif)
                .first(),
            Some(&family)
        );
        assert_eq!(
            system.family_of("Hello", &TextStyle::default(), 'H'),
            family
        );
    }

    fn monospace_follows_the_user_fixed_pitch_font(system: &mut System) {
        let fixed = core_text::user_fixed_pitch_font().expect("Core Text names a fixed-pitch font");
        let family = system.chosen_family(&fixed);
        assert_eq!(system.family_of("code", &monospace(), 'c'), family);
    }

    fn a_missing_hiragana_glyph_falls_back_to_the_family_core_text_chooses(system: &mut System) {
        let chosen =
            core_text::font_for(&HIRAGANA.to_string()).expect("Core Text has a face for hiragana");
        let family = system.chosen_family(&chosen);
        let text = format!("a{HIRAGANA}");
        assert_eq!(
            system.family_of(&text, &TextStyle::default(), HIRAGANA),
            family,
            "hiragana falls back as Core Text's cascade does"
        );
    }

    fn emoji_goes_to_apple_color_emoji(system: &mut System) {
        let text = format!("a{EMOJI}");
        assert_eq!(
            system.family_of(&text, &TextStyle::default(), EMOJI),
            "Apple Color Emoji"
        );
    }

    fn a_label_renders_from_core_text_fonts_without_font_directories(system: &mut System) {
        let style = TextStyle {
            size_px: 24.0,
            ..TextStyle::default()
        };
        let mut layout = TextLayout::new();
        layout.set_text("Press", &style);
        let laid = layout.layout(&mut system.fonts);
        let line = &laid.lines[0];
        assert_eq!(line.glyphs.len(), 5);
        let mut rasterizer = Rasterizer::new();
        let inked: usize = line
            .glyphs
            .iter()
            .map(|glyph| {
                assert_ne!(glyph.glyph_id, 0, "every letter has a real glyph");
                let placement = place_glyph(glyph, 0.0, line.baseline_y, true);
                match rasterizer.rasterize(&mut system.fonts, &placement.key) {
                    Some(GlyphImage::Mask(bitmap) | GlyphImage::Color(bitmap)) => {
                        bitmap.data.iter().filter(|value| **value > 0).count()
                    }
                    None => 0,
                }
            })
            .sum();
        assert!(inked > 150, "the label is inked: {inked} pixels");
    }

    type Case = (&'static str, fn(&mut System));

    pub fn main() {
        unsafe {
            std::env::remove_var("AQUA_FONT_DIRS");
        }
        let started = Instant::now();
        let database = FontDatabase::system();
        let elapsed = started.elapsed();
        println!(
            "system_database_load_time_is_reported: {} faces in {:.1} ms",
            database.face_count(),
            elapsed.as_secs_f64() * 1000.0
        );
        for generic in Generic::ALL {
            println!(
                "configured {generic:?}: {:?}",
                database.configured_families(generic)
            );
        }
        let mut system = System {
            fonts: FontManager::new(),
            database,
        };
        let cases: [Case; 5] = [
            (
                "configured_sans_serif_is_the_system_ui_font",
                configured_sans_serif_is_the_system_ui_font,
            ),
            (
                "monospace_follows_the_user_fixed_pitch_font",
                monospace_follows_the_user_fixed_pitch_font,
            ),
            (
                "a_missing_hiragana_glyph_falls_back_to_the_family_core_text_chooses",
                a_missing_hiragana_glyph_falls_back_to_the_family_core_text_chooses,
            ),
            (
                "emoji_goes_to_apple_color_emoji",
                emoji_goes_to_apple_color_emoji,
            ),
            (
                "a_label_renders_from_core_text_fonts_without_font_directories",
                a_label_renders_from_core_text_fonts_without_font_directories,
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
            case(&mut system);
            println!("test {name} ... ok");
        }
    }
}
