#![cfg(windows)]

use aqua_oracle_dwrite::{FontSource, Oracle};
use aqua_text::raster::{place_glyph, GlyphImage, RasterKey, Rasterizer};
use aqua_text::{FontFamily, FontManager, TextLayout, TextStyle};

const UNITS: u16 = 1000;
const SMALL_PPEM: u16 = 16;
const LARGE_PPEM: u16 = 64;
const SMALL_ORIGIN: (i16, i16) = (1, -2);
const LARGE_ORIGIN: (i16, i16) = (4, -8);

fn be16(value: u16) -> [u8; 2] {
    value.to_be_bytes()
}

fn bei16(value: i16) -> [u8; 2] {
    value.to_be_bytes()
}

fn utf16(text: &str) -> Vec<u8> {
    text.encode_utf16().flat_map(u16::to_be_bytes).collect()
}

fn quadrants(side: u32) -> Vec<u8> {
    let mut pixels = Vec::new();
    for y in 0..side {
        for x in 0..side {
            let pixel: [u8; 4] = match (x < side / 2, y < side / 2) {
                (true, true) => [255, 0, 0, 255],
                (false, true) => [0, 255, 0, 255],
                (true, false) => [0, 0, 255, 255],
                (false, false) => [255, 255, 255, 0],
            };
            pixels.extend_from_slice(&pixel);
        }
    }
    let mut out = Vec::new();
    let mut encoder = png::Encoder::new(&mut out, side, side);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&pixels)
        .unwrap();
    out
}

fn head() -> Vec<u8> {
    let mut t = vec![0u8; 54];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[12..16].copy_from_slice(&0x5F0F_3CF5u32.to_be_bytes());
    t[18..20].copy_from_slice(&be16(UNITS));
    t[44..46].copy_from_slice(&be16(8));
    t[46..48].copy_from_slice(&be16(2));
    t
}

fn hhea() -> Vec<u8> {
    let mut t = vec![0u8; 36];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[4..6].copy_from_slice(&bei16(800));
    t[6..8].copy_from_slice(&bei16(-200));
    t[10..12].copy_from_slice(&be16(UNITS));
    t[18..20].copy_from_slice(&bei16(1));
    t[34..36].copy_from_slice(&be16(3));
    t
}

fn maxp() -> Vec<u8> {
    let mut t = vec![0u8; 32];
    t[0..4].copy_from_slice(&0x0001_0000u32.to_be_bytes());
    t[4..6].copy_from_slice(&be16(3));
    t
}

fn hmtx() -> Vec<u8> {
    (0..3).flat_map(|_| [be16(UNITS), be16(0)].concat()).collect()
}

fn cmap() -> Vec<u8> {
    let mut sub = Vec::new();
    sub.extend_from_slice(&be16(4));
    sub.extend_from_slice(&be16(32));
    sub.extend_from_slice(&be16(0));
    sub.extend_from_slice(&be16(4));
    sub.extend_from_slice(&be16(4));
    sub.extend_from_slice(&be16(1));
    sub.extend_from_slice(&be16(0));
    for end in [0x42u16, 0xFFFF] {
        sub.extend_from_slice(&be16(end));
    }
    sub.extend_from_slice(&be16(0));
    for start in [0x41u16, 0xFFFF] {
        sub.extend_from_slice(&be16(start));
    }
    for delta in [1i32 - 0x41, 1] {
        sub.extend_from_slice(&bei16(delta as i16));
    }
    sub.extend_from_slice(&be16(0));
    sub.extend_from_slice(&be16(0));
    let mut t = Vec::new();
    t.extend_from_slice(&be16(0));
    t.extend_from_slice(&be16(1));
    t.extend_from_slice(&be16(3));
    t.extend_from_slice(&be16(1));
    t.extend_from_slice(&12u32.to_be_bytes());
    t.extend_from_slice(&sub);
    t
}

fn name() -> Vec<u8> {
    let strings = [
        (1u16, "AquaSbix"),
        (2, "Regular"),
        (4, "AquaSbix Regular"),
        (6, "AquaSbix-Regular"),
    ];
    let mut t = Vec::new();
    t.extend_from_slice(&be16(0));
    t.extend_from_slice(&be16(strings.len() as u16));
    t.extend_from_slice(&be16(6 + 12 * strings.len() as u16));
    let mut offset = 0u16;
    let mut storage = Vec::new();
    for (id, text) in strings {
        let encoded = utf16(text);
        for value in [3u16, 1, 0x409, id, encoded.len() as u16, offset] {
            t.extend_from_slice(&be16(value));
        }
        offset += encoded.len() as u16;
        storage.extend_from_slice(&encoded);
    }
    t.extend_from_slice(&storage);
    t
}

fn os2() -> Vec<u8> {
    let mut t = vec![0u8; 86];
    t[0..2].copy_from_slice(&be16(1));
    t[4..6].copy_from_slice(&be16(400));
    t[6..8].copy_from_slice(&be16(5));
    t[62..64].copy_from_slice(&be16(0x40));
    t[64..66].copy_from_slice(&be16(0x41));
    t[66..68].copy_from_slice(&be16(0x42));
    t[68..70].copy_from_slice(&bei16(800));
    t[70..72].copy_from_slice(&bei16(-200));
    t[74..76].copy_from_slice(&be16(800));
    t[76..78].copy_from_slice(&be16(200));
    t
}

fn post() -> Vec<u8> {
    let mut t = vec![0u8; 32];
    t[0..4].copy_from_slice(&0x0003_0000u32.to_be_bytes());
    t
}

fn strike(ppem: u16, origin: (i16, i16), png: &[u8]) -> Vec<u8> {
    let header = 4 + 4 * 4;
    let length = 8 + png.len() as u32;
    let mut t = Vec::new();
    t.extend_from_slice(&be16(ppem));
    t.extend_from_slice(&be16(72));
    for offset in [header, header, header + length, header + length] {
        t.extend_from_slice(&offset.to_be_bytes());
    }
    t.extend_from_slice(&bei16(origin.0));
    t.extend_from_slice(&bei16(origin.1));
    t.extend_from_slice(b"png ");
    t.extend_from_slice(png);
    t
}

fn sbix() -> Vec<u8> {
    let small = strike(SMALL_PPEM, SMALL_ORIGIN, &quadrants(16));
    let large = strike(LARGE_PPEM, LARGE_ORIGIN, &quadrants(64));
    let first = 8 + 8;
    let mut t = Vec::new();
    t.extend_from_slice(&be16(1));
    t.extend_from_slice(&be16(1));
    t.extend_from_slice(&2u32.to_be_bytes());
    t.extend_from_slice(&(first as u32).to_be_bytes());
    t.extend_from_slice(&((first + small.len()) as u32).to_be_bytes());
    t.extend_from_slice(&small);
    t.extend_from_slice(&large);
    t
}

fn font() -> Vec<u8> {
    let tables: Vec<(&[u8; 4], Vec<u8>)> = vec![
        (b"OS/2", os2()),
        (b"cmap", cmap()),
        (b"glyf", Vec::new()),
        (b"head", head()),
        (b"hhea", hhea()),
        (b"hmtx", hmtx()),
        (b"loca", vec![0u8; 8]),
        (b"maxp", maxp()),
        (b"name", name()),
        (b"post", post()),
        (b"sbix", sbix()),
    ];
    let count = tables.len() as u16;
    let mut out = 0x0001_0000u32.to_be_bytes().to_vec();
    out.extend_from_slice(&be16(count));
    let search = 16 * (1u16 << (15 - count.leading_zeros() as u16));
    out.extend_from_slice(&be16(search));
    out.extend_from_slice(&be16(15 - count.leading_zeros() as u16));
    out.extend_from_slice(&be16(count * 16 - search));
    let mut offset = 12 + 16 * tables.len();
    let mut body = Vec::new();
    for (tag, data) in &tables {
        out.extend_from_slice(*tag);
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(&(offset as u32).to_be_bytes());
        out.extend_from_slice(&(data.len() as u32).to_be_bytes());
        body.extend_from_slice(data);
        let padding = (4 - data.len() % 4) % 4;
        body.extend_from_slice(&vec![0u8; padding]);
        offset += data.len() + padding;
    }
    out.extend_from_slice(&body);
    out
}

fn style(size: f32) -> TextStyle {
    TextStyle {
        families: vec![FontFamily::Named("AquaSbix".to_owned())],
        size_px: size,
        ..TextStyle::default()
    }
}

fn key(fonts: &mut FontManager, size: f32) -> RasterKey {
    let mut layout = TextLayout::new();
    layout.set_text("A", &style(size));
    let laid = layout.layout(fonts);
    place_glyph(&laid.lines[0].glyphs[0], 0.0, laid.lines[0].baseline_y, true).key
}

fn texel(bitmap: &aqua_text::raster::Bitmap, x: u32, y: u32) -> [u8; 4] {
    let at = ((y * bitmap.width + x) * 4) as usize;
    [bitmap.data[at], bitmap.data[at + 1], bitmap.data[at + 2], bitmap.data[at + 3]]
}

fn fonts_with_sbix() -> FontManager {
    let mut fonts = FontManager::new_without_system_fonts();
    fonts.load_font_bytes(font());
    fonts
}

#[test]
fn a_complete_sbix_font_rasterizes_through_the_whole_text_stack() {
    let mut fonts = fonts_with_sbix();
    let mut rasterizer = Rasterizer::new();
    for (size, ppem, origin) in [
        (SMALL_PPEM, SMALL_PPEM, SMALL_ORIGIN),
        (LARGE_PPEM, LARGE_PPEM, LARGE_ORIGIN),
    ] {
        let key = RasterKey {
            subpixel: 0,
            ..key(&mut fonts, f32::from(size))
        };
        let GlyphImage::Color(bitmap) = rasterizer.rasterize(&mut fonts, &key).expect("sbix glyph")
        else {
            panic!("sbix glyph must be a color bitmap");
        };
        assert_eq!((bitmap.width, bitmap.height), (u32::from(ppem), u32::from(ppem)));
        assert_eq!(bitmap.left, i32::from(origin.0));
        assert_eq!(bitmap.top, i32::from(origin.1) + i32::from(ppem));
        let quarter = u32::from(ppem) / 4;
        let three_quarters = u32::from(ppem) * 3 / 4;
        assert_eq!(texel(&bitmap, quarter, quarter), [255, 0, 0, 255]);
        assert_eq!(texel(&bitmap, three_quarters, quarter), [0, 255, 0, 255]);
        assert_eq!(texel(&bitmap, quarter, three_quarters), [0, 0, 255, 255]);
        assert_eq!(texel(&bitmap, three_quarters, three_quarters)[3], 0);
    }
}

#[test]
fn intermediate_sizes_scale_from_the_nearest_strike_and_missing_glyphs_have_no_image() {
    let mut fonts = fonts_with_sbix();
    let mut rasterizer = Rasterizer::new();
    let key_24 = RasterKey {
        subpixel: 0,
        ..key(&mut fonts, 24.0)
    };
    let GlyphImage::Color(bitmap) = rasterizer.rasterize(&mut fonts, &key_24).expect("scaled strike")
    else {
        panic!("expected a color bitmap");
    };
    assert_eq!((bitmap.width, bitmap.height), (24, 24));
    assert_eq!(texel(&bitmap, 6, 6), [255, 0, 0, 255]);
    assert_eq!(texel(&bitmap, 18, 6), [0, 255, 0, 255]);
    let mut layout = TextLayout::new();
    layout.set_text("B", &style(16.0));
    let laid = layout.layout(&mut fonts);
    let blank = place_glyph(&laid.lines[0].glyphs[0], 0.0, laid.lines[0].baseline_y, true).key;
    assert!(rasterizer
        .rasterize(&mut fonts, &blank)
        .is_none_or(|image| image.bitmap().is_empty() || !matches!(image, GlyphImage::Color(_))));
}

#[test]
fn directwrite_reads_the_same_strike_with_matching_size_and_origin() {
    let mut fonts = fonts_with_sbix();
    let mut layout = TextLayout::new();
    layout.set_text("A", &style(16.0));
    let laid = layout.layout(&mut fonts);
    let glyph = laid.lines[0].glyphs[0].clone();
    let entry = fonts
        .entry_with(glyph.font_id, glyph.font_weight, glyph.font_stretch, false)
        .expect("face");
    let source = FontSource {
        data: entry.data(),
        face_index: entry.face_index(),
        weight: entry.weight(),
    };
    let mut oracle = Oracle::new().unwrap();
    const PNG: u32 = 0x10;
    let reported = oracle.strike_image(source, glyph.glyph_id, u32::from(SMALL_PPEM), PNG);
    let Ok(Some(image)) = reported else {
        panic!("directwrite refused the sbix strike: {reported:?}");
    };
    assert_eq!(image.pixels_per_em, u32::from(SMALL_PPEM));
    assert_eq!(image.size, (16, 16));
    assert_eq!(
        image.left_origin,
        (-(SMALL_ORIGIN.0 as f32), (SMALL_ORIGIN.1 + SMALL_PPEM as i16) as f32)
    );
}
