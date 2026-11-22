use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use crate::Generic;

const UNITS: u16 = 1000;
const ADVANCE: u16 = 600;

#[derive(Clone, Debug)]
pub struct FixtureFont {
    family: String,
    weight: u16,
    codepoints: Vec<u32>,
}

impl FixtureFont {
    pub fn new(family: &str) -> FixtureFont {
        FixtureFont {
            family: family.to_owned(),
            weight: 400,
            codepoints: Vec::new(),
        }
    }

    pub fn weight(mut self, weight: u16) -> FixtureFont {
        self.weight = weight;
        self
    }

    pub fn covering(mut self, characters: impl IntoIterator<Item = char>) -> FixtureFont {
        self.codepoints
            .extend(characters.into_iter().map(|character| character as u32));
        self
    }

    pub fn ascii(self) -> FixtureFont {
        self.covering((0x20u8..0x7f).map(char::from))
    }

    pub fn postscript_name(&self) -> String {
        let family: String = self.family.chars().filter(|c| !c.is_whitespace()).collect();
        format!("{family}-{}", self.subfamily())
    }

    fn subfamily(&self) -> &'static str {
        match self.weight {
            600.. => "Bold",
            _ => "Regular",
        }
    }

    pub fn build(&self) -> Vec<u8> {
        let mut codepoints = self.codepoints.clone();
        codepoints.sort_unstable();
        codepoints.dedup();
        let glyphs = codepoints.len() as u16 + 1;
        let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
            (b"OS/2", self.os2(&codepoints)),
            (b"cmap", cmap(&codepoints)),
            (b"glyf", glyf(glyphs)),
            (b"head", head()),
            (b"hhea", hhea(glyphs)),
            (b"hmtx", hmtx(glyphs)),
            (b"loca", loca(glyphs)),
            (b"maxp", maxp(glyphs)),
            (b"name", self.name()),
            (b"post", post()),
        ];
        sfnt(&tables)
    }

    fn name(&self) -> Vec<u8> {
        let full = format!("{} {}", self.family, self.subfamily());
        let postscript = self.postscript_name();
        let strings: [(u16, &str); 4] = [
            (1, &self.family),
            (2, self.subfamily()),
            (4, &full),
            (6, &postscript),
        ];
        let mut table = Vec::new();
        push16(&mut table, 0);
        push16(&mut table, strings.len() as u16);
        push16(&mut table, 6 + 12 * strings.len() as u16);
        let mut storage = Vec::new();
        for (id, text) in strings {
            let encoded: Vec<u8> = text.encode_utf16().flat_map(u16::to_be_bytes).collect();
            for value in [
                3u16,
                1,
                0x409,
                id,
                encoded.len() as u16,
                storage.len() as u16,
            ] {
                push16(&mut table, value);
            }
            storage.extend_from_slice(&encoded);
        }
        table.extend_from_slice(&storage);
        table
    }

    fn os2(&self, codepoints: &[u32]) -> Vec<u8> {
        let first = codepoints.first().copied().unwrap_or(0x20).min(0xffff) as u16;
        let last = codepoints.last().copied().unwrap_or(0x20).min(0xffff) as u16;
        let selection: u16 = match self.weight {
            600.. => 0x20,
            _ => 0x40,
        };
        let mut table = vec![0u8; 96];
        table[0..2].copy_from_slice(&4u16.to_be_bytes());
        table[2..4].copy_from_slice(&(ADVANCE as i16).to_be_bytes());
        table[4..6].copy_from_slice(&self.weight.to_be_bytes());
        table[6..8].copy_from_slice(&5u16.to_be_bytes());
        table[62..64].copy_from_slice(&selection.to_be_bytes());
        table[64..66].copy_from_slice(&first.to_be_bytes());
        table[66..68].copy_from_slice(&last.to_be_bytes());
        table[68..70].copy_from_slice(&800i16.to_be_bytes());
        table[70..72].copy_from_slice(&(-200i16).to_be_bytes());
        table[74..76].copy_from_slice(&800u16.to_be_bytes());
        table[76..78].copy_from_slice(&200u16.to_be_bytes());
        table[86..88].copy_from_slice(&500u16.to_be_bytes());
        table[88..90].copy_from_slice(&700u16.to_be_bytes());
        table
    }
}

fn push16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn push32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn head() -> Vec<u8> {
    let mut table = vec![0u8; 54];
    table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    table[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    table[18..20].copy_from_slice(&UNITS.to_be_bytes());
    table[36..38].copy_from_slice(&100i16.to_be_bytes());
    table[40..42].copy_from_slice(&500i16.to_be_bytes());
    table[42..44].copy_from_slice(&700i16.to_be_bytes());
    table[46..48].copy_from_slice(&8u16.to_be_bytes());
    table[48..50].copy_from_slice(&2i16.to_be_bytes());
    table
}

fn hhea(glyphs: u16) -> Vec<u8> {
    let mut table = vec![0u8; 36];
    table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    table[4..6].copy_from_slice(&800i16.to_be_bytes());
    table[6..8].copy_from_slice(&(-200i16).to_be_bytes());
    table[10..12].copy_from_slice(&ADVANCE.to_be_bytes());
    table[18..20].copy_from_slice(&1i16.to_be_bytes());
    table[34..36].copy_from_slice(&glyphs.to_be_bytes());
    table
}

fn maxp(glyphs: u16) -> Vec<u8> {
    let mut table = vec![0u8; 32];
    table[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    table[4..6].copy_from_slice(&glyphs.to_be_bytes());
    table[6..8].copy_from_slice(&4u16.to_be_bytes());
    table[8..10].copy_from_slice(&1u16.to_be_bytes());
    table[14..16].copy_from_slice(&1u16.to_be_bytes());
    table
}

fn hmtx(glyphs: u16) -> Vec<u8> {
    let mut table = Vec::new();
    for _ in 0..glyphs {
        push16(&mut table, ADVANCE);
        push16(&mut table, 100);
    }
    table
}

fn square() -> Vec<u8> {
    let mut glyph = Vec::new();
    for value in [1i16, 100, 0, 500, 700] {
        glyph.extend_from_slice(&value.to_be_bytes());
    }
    push16(&mut glyph, 3);
    push16(&mut glyph, 0);
    glyph.extend_from_slice(&[1, 1, 1, 1]);
    for x in [100i16, 0, 400, 0] {
        glyph.extend_from_slice(&x.to_be_bytes());
    }
    for y in [0i16, 700, 0, -700] {
        glyph.extend_from_slice(&y.to_be_bytes());
    }
    glyph
}

fn padded_square() -> Vec<u8> {
    let mut glyph = square();
    glyph.resize(glyph.len().next_multiple_of(4), 0);
    glyph
}

fn glyf(glyphs: u16) -> Vec<u8> {
    padded_square().repeat(usize::from(glyphs) - 1)
}

fn loca(glyphs: u16) -> Vec<u8> {
    let step = padded_square().len() as u16 / 2;
    let mut table = Vec::new();
    push16(&mut table, 0);
    for glyph in 0..glyphs {
        push16(&mut table, glyph * step);
    }
    table
}

fn post() -> Vec<u8> {
    let mut table = vec![0u8; 32];
    table[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    table
}

fn cmap(codepoints: &[u32]) -> Vec<u8> {
    let mut groups: Vec<(u32, u32)> = Vec::new();
    for &codepoint in codepoints {
        match groups.last_mut() {
            Some((_, end)) if *end + 1 == codepoint => *end = codepoint,
            _ => groups.push((codepoint, codepoint)),
        }
    }
    let mut subtable = Vec::new();
    push16(&mut subtable, 12);
    push16(&mut subtable, 0);
    push32(&mut subtable, 16 + 12 * groups.len() as u32);
    push32(&mut subtable, 0);
    push32(&mut subtable, groups.len() as u32);
    let mut next_glyph = 1;
    for (start, end) in groups {
        push32(&mut subtable, start);
        push32(&mut subtable, end);
        push32(&mut subtable, next_glyph);
        next_glyph += end - start + 1;
    }
    let mut table = Vec::new();
    push16(&mut table, 0);
    push16(&mut table, 1);
    push16(&mut table, 3);
    push16(&mut table, 10);
    push32(&mut table, 12);
    table.extend_from_slice(&subtable);
    table
}

fn checksum(data: &[u8]) -> u32 {
    data.chunks(4).fold(0u32, |sum, chunk| {
        let mut word = [0u8; 4];
        word[..chunk.len()].copy_from_slice(chunk);
        sum.wrapping_add(u32::from_be_bytes(word))
    })
}

fn sfnt(tables: &[(&[u8; 4], Vec<u8>)]) -> Vec<u8> {
    let count = tables.len() as u16;
    let selector = 15 - count.leading_zeros() as u16;
    let search = 16 * (1u16 << selector);
    let mut out = Vec::new();
    push32(&mut out, 0x0001_0000);
    push16(&mut out, count);
    push16(&mut out, search);
    push16(&mut out, selector);
    push16(&mut out, count * 16 - search);
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in tables {
        out.extend_from_slice(*tag);
        push32(&mut out, checksum(data));
        push32(&mut out, offset as u32);
        push32(&mut out, data.len() as u32);
        body.extend_from_slice(data);
        let padded = data.len().next_multiple_of(4);
        body.resize(body.len() + padded - data.len(), 0);
        offset += padded;
    }
    out.extend_from_slice(&body);
    out
}

fn generic_name(generic: Generic) -> &'static str {
    match generic {
        Generic::SansSerif => "sans-serif",
        Generic::Serif => "serif",
        Generic::Monospace => "monospace",
        Generic::Cursive => "cursive",
        Generic::Fantasy => "fantasy",
        Generic::Emoji => "emoji",
    }
}

fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

pub struct FontconfigFixture {
    root: PathBuf,
    fonts: PathBuf,
    aliases: Vec<(Generic, Vec<String>)>,
}

impl FontconfigFixture {
    pub fn new(label: &str) -> std::io::Result<FontconfigFixture> {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "aqua-fontconfig-{label}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let fonts = root.join("fonts");
        std::fs::create_dir_all(&fonts)?;
        Ok(FontconfigFixture {
            root,
            fonts,
            aliases: Vec::new(),
        })
    }

    pub fn install(
        &self,
        relative: impl AsRef<Path>,
        font: &FixtureFont,
    ) -> std::io::Result<PathBuf> {
        let path = self.fonts.join(relative);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&path, font.build())?;
        Ok(path)
    }

    pub fn prefer(mut self, generic: Generic, families: &[&str]) -> FontconfigFixture {
        self.aliases.push((
            generic,
            families.iter().map(|family| (*family).to_owned()).collect(),
        ));
        self
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn write_config(&self) -> std::io::Result<PathBuf> {
        let mut config = String::from(
            "<?xml version=\"1.0\"?>\n<!DOCTYPE fontconfig SYSTEM \"urn:fontconfig:fonts.dtd\">\n<fontconfig>\n",
        );
        config.push_str(&format!(
            "  <dir>{}</dir>\n  <cachedir>{}</cachedir>\n",
            escape(&self.fonts.to_string_lossy()),
            escape(&self.root.join("cache").to_string_lossy())
        ));
        for (generic, families) in &self.aliases {
            config.push_str(&format!(
                "  <alias binding=\"same\">\n    <family>{}</family>\n    <prefer>\n",
                generic_name(*generic)
            ));
            for family in families {
                config.push_str(&format!("      <family>{}</family>\n", escape(family)));
            }
            config.push_str("    </prefer>\n  </alias>\n");
        }
        config.push_str("</fontconfig>\n");
        let path = self.root.join("fonts.conf");
        std::fs::write(&path, config)?;
        Ok(path)
    }
}

impl Drop for FontconfigFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[cfg(target_os = "macos")]
pub mod core_text {
    use std::path::PathBuf;

    use objc2_core_foundation::{CFRange, CFString};
    use objc2_core_text::CTFontUIFontType;

    use crate::coretext::{face_of, interface_font};
    use crate::discovery::FaceRef;
    use crate::{FaceInfo, FontDatabase, Source};

    #[derive(Clone, Debug, PartialEq, Eq)]
    pub struct ChosenFace {
        pub path: PathBuf,
        pub post_script_name: String,
    }

    impl ChosenFace {
        fn of(face: FaceRef) -> Option<ChosenFace> {
            match face {
                FaceRef::Named {
                    path,
                    post_script_name,
                } => Some(ChosenFace {
                    path,
                    post_script_name,
                }),
                FaceRef::Indexed { .. } => None,
            }
        }

        pub fn resolve<'a>(&self, database: &'a FontDatabase) -> Option<&'a FaceInfo> {
            let in_file: Vec<&FaceInfo> = database
                .faces()
                .filter(|face| matches!(&face.source, Source::File(path) if *path == self.path))
                .collect();
            crate::locate(
                &in_file,
                &FaceRef::Named {
                    path: self.path.clone(),
                    post_script_name: self.post_script_name.clone(),
                },
            )
        }
    }

    pub fn system_ui_font() -> Option<ChosenFace> {
        interface_font(CTFontUIFontType::System)
            .and_then(|font| face_of(&font))
            .and_then(ChosenFace::of)
    }

    pub fn user_fixed_pitch_font() -> Option<ChosenFace> {
        interface_font(CTFontUIFontType::UserFixedPitch)
            .and_then(|font| face_of(&font))
            .and_then(ChosenFace::of)
    }

    pub fn font_for(text: &str) -> Option<ChosenFace> {
        let system = interface_font(CTFontUIFontType::System)?;
        let string = CFString::from_str(text);
        let range = CFRange {
            location: 0,
            length: text.encode_utf16().count() as isize,
        };
        let chosen = unsafe { system.for_string(&string, range) };
        face_of(&chosen).and_then(ChosenFace::of)
    }
}
